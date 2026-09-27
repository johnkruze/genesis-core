//! The Cultivator — shooting / finite-difference trajectory optimizer.
//!
//! Decision variables are piecewise-constant actuation knots expanded onto a
//! fixed horizon. Cost = attractor distance at the horizon + energy + smoothness.
//! Gradients are central differences through [`DiscreteDynamics::step`]. No `rand`
//! on this path. Analytic Jacobians are accepted when a plant provides them;
//! none of the sample adapters do yet (that would be the adjoint of the C domains).

use super::attractor::{GraspHoldAttractor, KinematicGraspAttractor, StrangeAttractor};
use super::dynamics::{
    DiscreteDynamics, GraspForceDynamics, HandTendonDynamics, GRASP_CLAMP_N, GRASP_DT,
};
use super::sheet_music::SheetMusic;
use crate::physics::dexterous::{hand_contact_q_sum_max, THUMB_OPPOSITION_RAD};

/// Optimizer hyperparameters. All deterministic.
#[derive(Clone, Debug)]
pub struct Cultivator {
    pub horizon: usize,
    pub dt: f64,
    pub max_iters: usize,
    pub step_size: f64,
    pub fd_eps: f64,
    pub lambda_energy: f64,
    pub lambda_smooth: f64,
    pub tol: f64,
    /// If `Some(k)`, optimize `k` holds per channel instead of every tick.
    pub n_knots: Option<usize>,
}

impl Default for Cultivator {
    fn default() -> Self {
        Self {
            horizon: 32,
            dt: GRASP_DT,
            max_iters: 80,
            step_size: 0.25,
            fd_eps: 1e-4,
            lambda_energy: 1e-4,
            lambda_smooth: 1e-2,
            tol: 1e-10,
            n_knots: Some(4),
        }
    }
}

/// Result of one invert: sheet music + terminal physics.
#[derive(Clone, Debug)]
pub struct InvertResult {
    pub sheet: SheetMusic,
    pub final_state: Vec<f64>,
    pub cost: f64,
    pub terminal_cost: f64,
    pub iters: usize,
}

impl Cultivator {
    fn n_knots(&self) -> usize {
        self.n_knots
            .unwrap_or(self.horizon)
            .clamp(1, self.horizon.max(1))
    }

    fn expand_knots(knots: &[f64], nu: usize, n_knots: usize, horizon: usize) -> Vec<f64> {
        let mut dense = vec![0.0; horizon * nu];
        if n_knots == 0 || nu == 0 {
            return dense;
        }
        for k in 0..horizon {
            let knot = ((k * n_knots) / horizon).min(n_knots - 1);
            for j in 0..nu {
                dense[k * nu + j] = knots[knot * nu + j];
            }
        }
        dense
    }

    fn clip_knots(knots: &mut [f64], nu: usize, bounds: &[(f64, f64)]) {
        let n = knots.len() / nu.max(1);
        for k in 0..n {
            for j in 0..nu {
                let (lo, hi) = bounds
                    .get(j)
                    .copied()
                    .unwrap_or((f64::NEG_INFINITY, f64::INFINITY));
                let i = k * nu + j;
                if i < knots.len() {
                    knots[i] = knots[i].clamp(lo, hi);
                }
            }
        }
    }

    fn rollout<D: DiscreteDynamics>(&self, dyns: &D, x0: &[f64], u_dense: &[f64]) -> Vec<f64> {
        let mut x = x0.to_vec();
        let nu = dyns.actuation_dim();
        for k in 0..self.horizon {
            let u = if nu == 0 {
                &[][..]
            } else {
                &u_dense[k * nu..(k + 1) * nu]
            };
            dyns.step(&mut x, u, self.dt);
        }
        x
    }

    fn regularizer(&self, u_dense: &[f64], nu: usize) -> f64 {
        let n = self.horizon;
        let mut e = 0.0;
        for k in 0..n {
            for j in 0..nu {
                let u = u_dense[k * nu + j];
                e += self.lambda_energy * u * u * self.dt;
            }
            if k + 1 < n {
                for j in 0..nu {
                    let du = u_dense[(k + 1) * nu + j] - u_dense[k * nu + j];
                    e += self.lambda_smooth * du * du * self.dt;
                }
            }
        }
        e
    }

    fn total_cost<D, A>(
        &self,
        dyns: &D,
        attr: &A,
        x0: &[f64],
        knots: &[f64],
    ) -> (f64, f64, Vec<f64>)
    where
        D: DiscreteDynamics,
        A: StrangeAttractor,
    {
        let nu = dyns.actuation_dim();
        let dense = Self::expand_knots(knots, nu, self.n_knots(), self.horizon);
        let x_n = self.rollout(dyns, x0, &dense);
        let terminal = attr.cost(&x_n);
        let j = terminal + self.regularizer(&dense, nu);
        (j, terminal, x_n)
    }

    fn finite_diff_grad<D, A>(&self, dyns: &D, attr: &A, x0: &[f64], knots: &[f64]) -> Vec<f64>
    where
        D: DiscreteDynamics,
        A: StrangeAttractor,
    {
        let eps = self.fd_eps;
        let mut g = vec![0.0; knots.len()];
        let nu = dyns.actuation_dim();
        let bounds = dyns.actuation_bounds();
        for i in 0..knots.len() {
            let mut up = knots.to_vec();
            let mut um = knots.to_vec();
            up[i] += eps;
            um[i] -= eps;
            Self::clip_knots(&mut up, nu, bounds);
            Self::clip_knots(&mut um, nu, bounds);
            let (jp, _, _) = self.total_cost(dyns, attr, x0, &up);
            let (jm, _, _) = self.total_cost(dyns, attr, x0, &um);
            let span = (up[i] - um[i]).abs().max(1e-18);
            g[i] = (jp - jm) / span;
        }
        g
    }

    /// Invert: work backward from `attr` through `dyns`, starting at `x0`.
    ///
    /// `u_init` is either one actuation sample (tiled) or a full knot vector.
    pub fn invert<D, A>(&self, dyns: &D, attr: &A, x0: &[f64], u_init: &[f64]) -> InvertResult
    where
        D: DiscreteDynamics,
        A: StrangeAttractor,
    {
        let nu = dyns.actuation_dim();
        let n_knots = self.n_knots();
        let mut knots = vec![0.0; n_knots * nu];
        if u_init.len() == nu {
            for k in 0..n_knots {
                for j in 0..nu {
                    knots[k * nu + j] = u_init[j];
                }
            }
        } else if u_init.len() == knots.len() {
            knots.copy_from_slice(u_init);
        } else if !u_init.is_empty() && nu > 0 {
            for (i, slot) in knots.iter_mut().enumerate() {
                *slot = u_init[i % u_init.len()];
            }
        }
        Self::clip_knots(&mut knots, nu, dyns.actuation_bounds());

        let (mut best_j, mut best_term, mut best_x) = self.total_cost(dyns, attr, x0, &knots);
        let mut best_knots = knots.clone();
        let mut alpha = self.step_size;
        let mut iters = 0usize;

        for iter in 0..self.max_iters {
            iters = iter + 1;
            let g = self.finite_diff_grad(dyns, attr, x0, &knots);
            let gnorm: f64 = g.iter().map(|v| v * v).sum::<f64>().sqrt();
            if gnorm < self.tol {
                break;
            }
            let mut improved = false;
            let mut trial_alpha = alpha;
            for _bt in 0..8 {
                let mut trial = knots.clone();
                for i in 0..trial.len() {
                    trial[i] -= trial_alpha * g[i];
                }
                Self::clip_knots(&mut trial, nu, dyns.actuation_bounds());
                let (j, term, x_n) = self.total_cost(dyns, attr, x0, &trial);
                if j < best_j {
                    knots = trial;
                    best_j = j;
                    best_term = term;
                    best_x = x_n;
                    best_knots = knots.clone();
                    alpha = (trial_alpha * 1.2).min(self.step_size * 4.0);
                    improved = true;
                    break;
                }
                trial_alpha *= 0.5;
            }
            if !improved {
                alpha *= 0.5;
                if alpha < 1e-12 {
                    break;
                }
            }
        }

        let dense = Self::expand_knots(&best_knots, nu, n_knots, self.horizon);
        // 1e-4 abs: sub-µε optimizer jitter is not a new hold.
        let sheet = SheetMusic::from_samples(dyns.channels(), self.dt, self.horizon, dense, 1e-4);
        InvertResult {
            sheet,
            final_state: best_x,
            cost: best_j,
            terminal_cost: best_term,
            iters,
        }
    }
}

/// Tiny invert of a grasp-hold attractor. `mass_kg`, `mu`, `force0_n` seed the plant.
pub fn invert_grasp_hold(
    mass_kg: f64,
    mu: f64,
    force0_n: f64,
    target_margin: f64,
    horizon: usize,
) -> InvertResult {
    let plant = GraspForceDynamics::default();
    let attr = GraspHoldAttractor::new(target_margin, 0.0);
    let x0 = GraspForceDynamics::idle_state(mass_kg, mu, force0_n);
    let cult = Cultivator {
        horizon,
        dt: GRASP_DT,
        n_knots: Some(horizon.min(8).max(1)),
        max_iters: 60,
        step_size: 12.0,
        fd_eps: 2e-2,
        lambda_energy: 1e-5,
        lambda_smooth: 5e-3,
        ..Cultivator::default()
    };
    cult.invert(&plant, &attr, &x0, &[force0_n.clamp(0.0, GRASP_CLAMP_N)])
}

/// Invert a 5-finger wrap. Default plant actuates close + opposition.
pub fn invert_kinematic_grasp(
    mass_kg: f64,
    mu: f64,
    span_m: f64,
    close0_rad: f64,
    opposition0_rad: f64,
    slip0_m_s: f64,
    horizon: usize,
) -> InvertResult {
    let plant = HandTendonDynamics::close_and_opposition();
    let attr = KinematicGraspAttractor::wrap(span_m);
    let x0 =
        HandTendonDynamics::idle_state(mass_kg, mu, span_m, close0_rad, opposition0_rad, slip0_m_s);
    let wrap_close = (hand_contact_q_sum_max(span_m as f32) as f64 / 2.18).clamp(0.05, 1.55);
    let wrap_opp = THUMB_OPPOSITION_RAD as f64;
    let cult = Cultivator {
        horizon,
        dt: GRASP_DT,
        n_knots: Some(horizon.min(4).max(1)),
        max_iters: 35,
        step_size: 0.12,
        fd_eps: 4e-3,
        lambda_energy: 5e-5,
        lambda_smooth: 8e-2,
        ..Cultivator::default()
    };
    cult.invert(&plant, &attr, &x0, &[wrap_close, wrap_opp])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntropy::attractor::ScalarAttractor;
    use crate::syntropy::dynamics::{
        GlomalinExudationDynamics, GraspForceDynamics, HandTendonDynamics, GRASP_MARGIN,
        GRASP_STATE_DIM, HAND_BLOCKED_STRAIN, HAND_MARGIN, HAND_OVERSTRETCH, HAND_Q_MCP,
        HAND_SLIP_V,
    };
    use crate::syntropy::sheet_music::{ActuationChannel, ChannelKind};

    /// 1-D Euler: x ← x + dt u. Analytic ∂J/∂u_k = 2 (x_N − x*) dt with J = (x_N − x*)².
    struct Euler1d {
        channels: [ActuationChannel; 1],
        bounds: [(f64, f64); 1],
    }

    impl Euler1d {
        fn new() -> Self {
            Self {
                channels: [ActuationChannel {
                    name: "u",
                    unit: "1/s",
                    kind: ChannelKind::Frequency,
                }],
                bounds: [(-20.0, 20.0)],
            }
        }
    }

    impl DiscreteDynamics for Euler1d {
        fn state_dim(&self) -> usize {
            1
        }
        fn channels(&self) -> &[ActuationChannel] {
            &self.channels
        }
        fn actuation_bounds(&self) -> &[(f64, f64)] {
            &self.bounds
        }
        fn step(&self, state: &mut [f64], actuation: &[f64], dt: f64) {
            state[0] += dt * actuation[0];
        }
    }

    #[test]
    fn finite_diff_matches_analytic_euler() {
        let plant = Euler1d::new();
        let attr = ScalarAttractor::new("x", 0, 1.0, 1.0);
        let cult = Cultivator {
            horizon: 4,
            dt: 0.1,
            n_knots: None,
            lambda_energy: 0.0,
            lambda_smooth: 0.0,
            fd_eps: 1e-6,
            ..Cultivator::default()
        };
        let x0 = [0.0];
        let knots = vec![1.0, 1.0, 1.0, 1.0];
        let g = cult.finite_diff_grad(&plant, &attr, &x0, &knots);
        // x_N = 0.4, target 1 → 2*(0.4-1)*0.1 = -0.12 each
        for gi in &g {
            assert!((gi + 0.12).abs() < 1e-6, "fd grad {gi} expected -0.12");
        }
    }

    #[test]
    fn euler_invert_reaches_target() {
        let plant = Euler1d::new();
        let attr = ScalarAttractor::new("x", 0, 1.0, 1.0);
        let cult = Cultivator {
            horizon: 8,
            dt: 0.1,
            n_knots: None,
            max_iters: 40,
            step_size: 2.0,
            lambda_energy: 0.0,
            lambda_smooth: 0.05,
            fd_eps: 1e-5,
            ..Cultivator::default()
        };
        let out = cult.invert(&plant, &attr, &[0.0], &[0.0]);
        assert!(
            (out.final_state[0] - 1.0).abs() < 0.05,
            "x_N = {}",
            out.final_state[0]
        );
        assert_sheet_roundtrip(&out.sheet);
    }

    fn assert_sheet_roundtrip(sheet: &crate::syntropy::sheet_music::SheetMusic) {
        let replay = sheet.reconstruct();
        assert_eq!(replay.len(), sheet.n_steps * sheet.channels.len());
        let again = crate::syntropy::sheet_music::SheetMusic::from_samples(
            &sheet.channels,
            sheet.dt_s,
            sheet.n_steps,
            replay.clone(),
            1e-4,
        );
        assert_eq!(again.reconstruct(), replay);
    }

    #[test]
    fn grasp_one_step_invertibility() {
        let plant = GraspForceDynamics::default();
        let mass = 0.4;
        let mu = 0.65;
        let f_true = 24.0;
        let mut x_true = GraspForceDynamics::idle_state(mass, mu, f_true);
        plant.step(&mut x_true, &[f_true], GRASP_DT);
        let target_margin = x_true[GRASP_MARGIN];
        assert!(target_margin > 0.05);

        // Margin-only: slip weight would trade the last Newtons. Init inside the
        // unclamped-margin cone so ∂margin/∂F is nonzero (f32 taxel flags).
        let attr = ScalarAttractor::new("grasp_margin", GRASP_MARGIN, target_margin, 16.0);
        let cult = Cultivator {
            horizon: 1,
            dt: GRASP_DT,
            n_knots: None,
            max_iters: 80,
            step_size: 40.0,
            fd_eps: 2e-2,
            lambda_energy: 0.0,
            lambda_smooth: 0.0,
            ..Cultivator::default()
        };
        let x0 = GraspForceDynamics::idle_state(mass, mu, 16.0);
        let out = cult.invert(&plant, &attr, &x0, &[16.0]);
        let f_hat = out.sheet.samples[0];
        assert!(
            (f_hat - f_true).abs() < 2.5,
            "recovered {f_hat} N, true {f_true} N, margin target {target_margin}, got {}",
            out.final_state[GRASP_MARGIN]
        );
        assert_eq!(out.final_state.len(), GRASP_STATE_DIM);
        assert!((out.final_state[GRASP_MARGIN] - target_margin).abs() < 0.08);
    }

    #[test]
    fn glomalin_invert_hits_yield_stress() {
        let plant = GlomalinExudationDynamics::andisol(0.20);
        let x0 = plant.idle_state(1.0);
        let target_pa = plant.idle_state(4.0)[1];
        let attr = ScalarAttractor::soil_glomalin_yield_stress_pa(target_pa);
        let cult = Cultivator {
            horizon: 8,
            dt: 1.0,
            n_knots: None,
            max_iters: 40,
            step_size: 0.05,
            fd_eps: 1e-5,
            lambda_energy: 1e-6,
            lambda_smooth: 1e-4,
            ..Cultivator::default()
        };
        let out = cult.invert(&plant, &attr, &x0, &[0.0]);
        assert!(
            (out.final_state[1] - target_pa).abs() / target_pa < 0.08,
            "yield {} target {}",
            out.final_state[1],
            target_pa
        );
        assert_sheet_roundtrip(&out.sheet);
    }

    #[test]
    fn invert_grasp_hold_emits_sheet_music() {
        let out = invert_grasp_hold(0.4, 0.65, 10.0, 0.35, 16);
        assert_eq!(out.sheet.n_steps, 16);
        assert!((out.sheet.dt_s - GRASP_DT).abs() < 1e-18);
        assert!(!out.sheet.events.is_empty());
        let text = out.sheet.format_score();
        assert!(text.contains("grip_force"));
        assert!(text.contains("N"));
        assert_eq!(out.sheet.reconstruct().len(), 16);
    }

    #[test]
    fn invert_kinematic_grasp_five_fingers_no_overstretch() {
        use crate::physics::dexterous::{N_HAND_FINGERS, TENDON_STRAIN_WARN};

        let mass = 0.35;
        let mu = 0.28;
        let span = 0.028;
        let close0 = 0.40;
        let opp0 = 0.80;
        let slip0 = 0.003;
        let horizon = 48;

        let plant = HandTendonDynamics::close_and_opposition();
        let mut open = HandTendonDynamics::idle_state(mass, mu, span, close0, opp0, slip0);
        for _ in 0..horizon {
            plant.step(&mut open, &[close0, opp0], GRASP_DT);
        }

        let out = invert_kinematic_grasp(mass, mu, span, close0, opp0, slip0, horizon);
        assert_eq!(out.sheet.n_steps, horizon);
        assert_eq!(out.sheet.channels.len(), 2);
        let text = out.sheet.format_score();
        assert!(text.contains("commanded_close_rad"));
        assert!(text.contains("opposition_rad"));
        assert_sheet_roundtrip(&out.sheet);

        for u in &out.sheet.samples {
            assert!(u.is_finite());
        }
        assert!(
            out.final_state[HAND_OVERSTRETCH] < 0.5,
            "overstretch flag {}",
            out.final_state[HAND_OVERSTRETCH]
        );
        assert!(
            out.final_state[HAND_BLOCKED_STRAIN] <= TENDON_STRAIN_WARN as f64 + 1e-4,
            "blocked strain {}",
            out.final_state[HAND_BLOCKED_STRAIN]
        );
        assert!(
            out.final_state[HAND_SLIP_V] < 0.02,
            "invert slip {}",
            out.final_state[HAND_SLIP_V]
        );
        assert!(
            out.final_state[HAND_SLIP_V] <= open[HAND_SLIP_V] + 1e-4,
            "invert slip {} vs open-loop {}",
            out.final_state[HAND_SLIP_V],
            open[HAND_SLIP_V]
        );
        let mut qmin = f64::MAX;
        let mut qmax = f64::MIN;
        for f in 0..N_HAND_FINGERS {
            let q = out.final_state[HAND_Q_MCP + f];
            assert!((0.0..=1.6).contains(&q), "finger {f} q_mcp {q}");
            qmin = qmin.min(q);
            qmax = qmax.max(q);
        }
        assert!(
            qmax - qmin > 1e-4,
            "five MCP angles must not collapse to one value"
        );
        assert!(out.final_state[HAND_MARGIN] >= 0.0);
        let peak_close = out
            .sheet
            .samples
            .chunks(2)
            .map(|c| c[0])
            .fold(0.0_f64, f64::max);
        assert!(peak_close <= 1.55 + 1e-9);
    }
}
