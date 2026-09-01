//! Tesseract — nonlinear IMU organ.
//!
//! Drive-mode Duffing on a proof-mass, Coriolis scale factor on the sense axis,
//! adaptive hold on one periodic attractor. Physics package and control in `step()`.
//!
//! Linear plateau (inherit Cluster D; do not rewrite it):
//!
//! \[
//! \ddot x + 2\zeta\omega_n\dot x + \omega_n^2 x = a(t)
//! \]
//!
//! Duffing leap (the cubic):
//!
//! \[
//! \ddot x + 2\zeta\omega_n\dot x + \omega_n^2 x + \alpha x^3 = a_{\mathrm{inertial}}(t) + u_{\mathrm{control}}(t)
//! \]
//!
//! - \(\alpha > 0\): spring-hardening
//! - \(\alpha < 0\): spring-softening
//!
//! Coriolis scale factor (sense axis, not a term in the drive ODE):
//!
//! \[
//! F_c = (2 m v) \times \Omega \qquad\Rightarrow\qquad |F_c| = 2 m |v| |\Omega|
//! \]
//!
//! Linear-plateau velocity is ~1 m/s. High-velocity tether is ~70 m/s.
//! AlN dilatational speed ~182 m/s and diamond ~400 m/s are published
//! wave speeds cited in comments — not SKUs, not products.
//!
//! Clock is \(\omega_n\). Integrator is the same semi-implicit Euler as
//! `DynamicOscillator` so \(\alpha = 0\), \(u = 0\) is Cluster D, not a fork.
//!
//! Bias floor is a constant bias (bias instability), not a zero-mean walk.
//! Reduced-order Allan-style floor: \(\tfrac12 |b| t^2\) (accel) or \(|b| t\) (rate).

use serde::{Deserialize, Serialize};

/// Restore stiffness [1/s²]. Analog of ankle \(k_p > m g h\): \(k_p > \omega_n^2\) holds the orbit.
pub const HOLD_KP_SUFFICIENT: f64 = 1.2e6;
/// Far below \(\omega_n^2\) a kick crosses the fold.
pub const HOLD_KP_INSUFFICIENT: f64 = 12.0;
/// Derivative gain [1/s].
pub const HOLD_KD: f64 = 1.6e3;
/// Control saturation [m/s²].
pub const HOLD_U_SAT_M_S2: f64 = 800.0;

/// Linear-plateau proof-mass velocity [m/s].
pub const V_LINEAR_M_S: f64 = 1.0;
/// High-velocity tether [m/s]. Scale factor \(\propto m v\).
pub const V_TETHER_M_S: f64 = 70.0;

/// Accel bias → position floor, or rate bias → angle floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BiasKind {
    Accel,
    Rate,
}

/// Drive-mode resonator + sense-axis scale factor + hold.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Tesseract {
    pub displacement_m: f64,
    pub velocity_m_s: f64,
    pub mass_kg: f64,
    pub omega_n_rad_s: f64,
    pub zeta: f64,
    /// Duffing cubic [1/m² s²]. Zero recovers `DynamicOscillator`.
    pub alpha: f64,
    /// Adaptive accel [m/s²], set by `control_hold_attractor`.
    pub control_u: f64,
    /// Fold amplitude [m] that splits high/low periodic branches.
    pub fold_m: f64,
    /// Last classified branch: +1 high, −1 low, 0 unset.
    pub branch: i8,
}

/// One tick of the organ.
#[derive(Debug, Clone, Copy)]
pub struct TesseractTick {
    pub displacement_m: f64,
    pub velocity_m_s: f64,
    /// Instantaneous Coriolis force \(2 m v \Omega\) [N].
    pub scale_factor_n: f64,
    /// Euler lag residual of the drive ODE [m/s²].
    pub residual: f64,
    /// True when the high/low branch flipped this tick.
    pub hopped: bool,
    pub branch: i8,
}

impl Tesseract {
    pub fn new(natural_freq_hz: f64, damping_ratio: f64, mass_kg: f64, alpha: f64) -> Self {
        let omega_n = natural_freq_hz * 2.0 * std::f64::consts::PI;
        Self {
            displacement_m: 0.0,
            velocity_m_s: 0.0,
            mass_kg: mass_kg.max(1e-12),
            omega_n_rad_s: omega_n.max(1e-3),
            zeta: damping_ratio.max(0.0),
            alpha,
            control_u: 0.0,
            fold_m: 0.0,
            branch: 0,
        }
    }

    /// Instantaneous envelope \(r = \sqrt{x^2 + (v/\omega_n)^2}\).
    pub fn envelope_m(&self) -> f64 {
        let w = self.omega_n_rad_s.max(1e-9);
        self.displacement_m.hypot(self.velocity_m_s / w)
    }

    /// Advance the Duffing drive mode. \(\Omega\) enters only through Coriolis scale.
    /// Callers who carry a constant accel-bias fold it in: `step(a + bias, …)`.
    pub fn step(&mut self, inertial_accel: f64, omega_ext_rad_s: f64, dt: f64) -> TesseractTick {
        let dt = dt.max(1e-12);
        let w = self.omega_n_rad_s;
        let x = self.displacement_m;
        let v = self.velocity_m_s;
        let spring = -w * w * x;
        let damp = -2.0 * self.zeta * w * v;
        let cubic = -self.alpha * x * x * x;
        let total_accel = inertial_accel + self.control_u + spring + damp + cubic;

        self.velocity_m_s += total_accel * dt;
        self.displacement_m += self.velocity_m_s * dt;

        let actual = (self.velocity_m_s - v) / dt;
        let x_n = self.displacement_m;
        let v_n = self.velocity_m_s;
        let expected_new = inertial_accel + self.control_u
            - w * w * x_n
            - 2.0 * self.zeta * w * v_n
            - self.alpha * x_n * x_n * x_n;
        let residual = (actual - expected_new).abs();

        // Instantaneous well \(\dot x\). The farm records sense-axis \(F_c\)
        // via `coriolis_scale_n(mass, drive_v, Ω)` — not this tick field.
        let scale_factor_n = coriolis_scale_n(self.mass_kg, self.velocity_m_s, omega_ext_rad_s);

        let prev_branch = self.branch;
        let amp = self.envelope_m();
        let next_branch = if self.fold_m > 0.0 {
            branch_from_amplitude(amp, self.fold_m)
        } else {
            prev_branch
        };
        let hopped = is_phase_hop(prev_branch, next_branch);
        if next_branch != 0 {
            self.branch = next_branch;
        }

        TesseractTick {
            displacement_m: self.displacement_m,
            velocity_m_s: self.velocity_m_s,
            scale_factor_n,
            residual,
            hopped,
            branch: self.branch,
        }
    }

    /// Same Euler as `step`, with a constant accel-bias folded into the drive
    /// so the well feels it. Does not add a field; `b` is a caller argument.
    pub fn step_with_bias(
        &mut self,
        inertial_accel: f64,
        bias_m_s2: f64,
        omega_ext_rad_s: f64,
        dt: f64,
    ) -> TesseractTick {
        self.step(inertial_accel + bias_m_s2, omega_ext_rad_s, dt)
    }
}

/// \(|F_c| = 2 m |v| |\Omega|\). Scale factor \(\propto m v\).
///
/// `velocity_m_s` is **sense-axis** speed (tether or linear plateau),
/// not \(\dot x\) of the Duffing well. The well overwrites oscillator
/// velocity; the proof-mass Coriolis law uses the drive-axis speed.
#[inline]
pub fn coriolis_scale_n(mass_kg: f64, velocity_m_s: f64, omega_rad_s: f64) -> f64 {
    2.0 * mass_kg.abs() * velocity_m_s.abs() * omega_rad_s.abs()
}

/// Restore toward the commanded periodic branch. Saturates.
///
/// Named law (same job as `pd_ankle_torque_nm`):
/// \(u = \mathrm{clamp}(k_p (x_{\mathrm{cmd}} - x) + k_d (v_{\mathrm{cmd}} - v),\,\pm u_{\mathrm{sat}})\).
/// \(k_p > \omega_n^2\) holds; below it a kick phase-hops.
#[inline]
pub fn control_hold_attractor(
    displacement_m: f64,
    velocity_m_s: f64,
    x_cmd_m: f64,
    v_cmd_m_s: f64,
    k_p: f64,
    k_d: f64,
    u_sat_m_s2: f64,
) -> f64 {
    (k_p * (x_cmd_m - displacement_m) + k_d * (v_cmd_m_s - velocity_m_s))
        .clamp(-u_sat_m_s2.abs(), u_sat_m_s2.abs())
}

/// Reduced-order floor from a **constant** bias (bias instability), not a walk.
/// Accel: \(\tfrac12 |b| t^2\) [m]. Rate: \(|b| t\) [rad].
#[inline]
pub fn bias_floor_m_or_rad(bias: f64, t_s: f64, kind: BiasKind) -> f64 {
    let t = t_s.max(0.0);
    match kind {
        BiasKind::Accel => 0.5 * bias.abs() * t * t,
        BiasKind::Rate => bias.abs() * t,
    }
}

/// High branch if envelope ≥ fold, else low. Fold ≤ 0 leaves the branch unset.
#[inline]
pub fn branch_from_amplitude(amp_m: f64, fold_m: f64) -> i8 {
    if fold_m <= 0.0 {
        0
    } else if amp_m >= fold_m {
        1
    } else {
        -1
    }
}

/// Phase-hop: the resonator jumped between the two stable operating points.
#[inline]
pub fn is_phase_hop(prev_branch: i8, next_branch: i8) -> bool {
    prev_branch != 0 && next_branch != 0 && prev_branch != next_branch
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::resonance::DynamicOscillator;
    use crate::rng::Rng;

    const DT_D: f64 = 0.001;

    #[test]
    fn alpha_zero_matches_dynamic_oscillator() {
        let mut lin = DynamicOscillator::new(10.0, 0.05);
        let mut tes = Tesseract::new(10.0, 0.05, 1e-6, 0.0);
        tes.control_u = 0.0;
        let dt = DT_D;
        for tick in 0..1000 {
            let t = tick as f64 * dt;
            let a = (t * 10.0 * 2.0 * std::f64::consts::PI).sin() * 10.0;
            lin.step(a, dt);
            tes.step(a, 0.0, dt);
            assert!(
                (lin.displacement_m - tes.displacement_m).abs() < 1e-12,
                "tick {tick}: lin {} tes {}",
                lin.displacement_m,
                tes.displacement_m
            );
            assert!((lin.velocity_m_s - tes.velocity_m_s).abs() < 1e-12);
        }
    }

    #[test]
    fn hardening_reduces_amplitude_at_resonance() {
        // Same drive at ω = ω_n. α > 0 adds restoring cubic → smaller peak.
        let fn_hz = 10.0;
        let zeta = 0.05;
        let dt = DT_D;
        let steps = 2000;
        let drive = |t: f64| (t * fn_hz * 2.0 * std::f64::consts::PI).sin() * 10.0;

        let mut lin = Tesseract::new(fn_hz, zeta, 1e-6, 0.0);
        let mut hard = Tesseract::new(fn_hz, zeta, 1e-6, 4.0e9);
        let mut peak_lin = 0.0_f64;
        let mut peak_hard = 0.0_f64;
        for k in 0..steps {
            let t = k as f64 * dt;
            let a = drive(t);
            lin.step(a, 0.0, dt);
            hard.step(a, 0.0, dt);
            peak_lin = peak_lin.max(lin.displacement_m.abs());
            peak_hard = peak_hard.max(hard.displacement_m.abs());
        }
        assert!(
            peak_hard < peak_lin * 0.92,
            "hardening should cut amplitude: hard {peak_hard} lin {peak_lin}"
        );
        assert!(peak_lin > 0.01);
    }

    #[test]
    fn coriolis_scale_doubles_when_velocity_doubles() {
        let m = 1.5e-7;
        let omega = 0.4;
        let s1 = coriolis_scale_n(m, 1.0, omega);
        let s2 = coriolis_scale_n(m, 2.0, omega);
        assert!((s2 - 2.0 * s1).abs() < 1e-18);
        let tether = coriolis_scale_n(m, V_TETHER_M_S, omega);
        let plateau = coriolis_scale_n(m, V_LINEAR_M_S, omega);
        assert!((tether / plateau - 70.0).abs() < 1e-9);
    }

    fn hop_scenario(k_p: f64, k_d: f64) -> (f64, bool) {
        // Hardening Duffing, drive slightly above ω_n. Start on the high
        // branch, kick across the fold, then hold or fall.
        let fn_hz = 80.0;
        let zeta = 0.012;
        let alpha = 6.0e8;
        let dt = 1.0 / (fn_hz * 24.0);
        let w_d = 1.07 * fn_hz * 2.0 * std::f64::consts::PI;
        let f_drive = 55.0;
        let fold = 0.008;
        let steps_period = 24usize;
        let settle = 40 * steps_period;
        let hold = 50 * steps_period;

        let mut tes = Tesseract::new(fn_hz, zeta, 1e-7, alpha);
        tes.displacement_m = 0.022;
        tes.velocity_m_s = 0.0;
        tes.fold_m = fold;
        tes.branch = 1;

        for k in 0..settle {
            let t = k as f64 * dt;
            tes.control_u = 0.0;
            tes.step(f_drive * (w_d * t).sin(), 0.0, dt);
        }
        let a_cmd = tes.envelope_m().max(0.012);
        tes.displacement_m *= 0.28;
        tes.velocity_m_s *= 0.28;
        tes.branch = branch_from_amplitude(tes.envelope_m(), fold);

        for k in 0..hold {
            let t = (settle + k) as f64 * dt;
            let x_cmd = a_cmd * (w_d * t).sin();
            let v_cmd = a_cmd * w_d * (w_d * t).cos();
            tes.control_u = control_hold_attractor(
                tes.displacement_m,
                tes.velocity_m_s,
                x_cmd,
                v_cmd,
                k_p,
                k_d,
                HOLD_U_SAT_M_S2,
            );
            tes.step(f_drive * (w_d * t).sin(), 0.0, dt);
        }
        let amp = tes.envelope_m();
        let end_low = amp < fold;
        (amp, end_low)
    }

    #[test]
    fn sufficient_gain_holds_attractor() {
        let (amp, end_low) = hop_scenario(HOLD_KP_SUFFICIENT, HOLD_KD);
        assert!(
            !end_low && amp >= 0.008,
            "sufficient k_p must hold the high branch: amp {amp} end_low {end_low}"
        );
    }

    #[test]
    fn insufficient_gain_phase_hops() {
        let (amp, end_low) = hop_scenario(HOLD_KP_INSUFFICIENT, 0.0);
        assert!(
            end_low && amp < 0.008,
            "insufficient k_p must hop to the low branch: amp {amp} end_low {end_low}"
        );
    }

    #[test]
    fn constant_bias_hits_floor_walk_does_not() {
        // Marine lesson, reduced-order: constant bias is the cliff.
        let t_s = 8.0;
        let dt = 0.01;
        let bias = 0.02; // m/s²
        let lock_m = 0.50;
        let floor = bias_floor_m_or_rad(bias, t_s, BiasKind::Accel);
        // ½ · 0.02 · 64 = 0.64 m
        assert!((floor - 0.64).abs() < 1e-12);
        assert!(floor > lock_m);

        let mut pos_c = 0.0;
        let mut vel_c = 0.0;
        let steps = (t_s / dt) as usize;
        for _ in 0..steps {
            vel_c += bias * dt;
            pos_c += vel_c * dt;
        }
        assert!(pos_c.abs() > lock_m);

        let mut pos_w = 0.0;
        let mut vel_w = 0.0;
        let mut rng = Rng::new(0x7E55_0001);
        for _ in 0..steps {
            // Zero-mean walk of the same σ (accel white → VRW). Same |σ| as |bias|.
            let a = rng.gaussian(0.0, bias);
            vel_w += a * dt;
            pos_w += vel_w * dt;
        }
        assert!(
            pos_w.abs() < lock_m,
            "walk of the same σ must miss the lock-loss: pos {pos_w}"
        );
    }

    #[test]
    fn constant_bias_added_to_inertial_accel_moves_the_mass() {
        // Same drive, α = 0, u = 0. Bias folded into inertial_accel
        // displaces the proof-mass by the spring particular solution
        // x ≈ b / ω_n². (½ b t² is the nav floor, not the well.)
        let fn_hz = 10.0;
        let zeta = 0.05;
        let dt = DT_D;
        let steps = 1000;
        let bias = 0.40;
        let drive = |t: f64| (t * fn_hz * 2.0 * std::f64::consts::PI).sin() * 10.0;

        let mut clean = Tesseract::new(fn_hz, zeta, 1e-6, 0.0);
        let mut biased = Tesseract::new(fn_hz, zeta, 1e-6, 0.0);
        clean.control_u = 0.0;
        biased.control_u = 0.0;
        for k in 0..steps {
            let t = k as f64 * dt;
            let a = drive(t);
            clean.step(a, 0.0, dt);
            biased.step_with_bias(a, bias, 0.0, dt);
        }
        let dx = (biased.displacement_m - clean.displacement_m).abs();
        let expected = bias / (clean.omega_n_rad_s * clean.omega_n_rad_s);
        assert!(
            dx > 1e-6 && (dx - expected).abs() / expected < 0.15,
            "well must sit near b/ω_n²: dx {dx} expected {expected}"
        );
    }
}
