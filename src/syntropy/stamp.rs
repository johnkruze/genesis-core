//! Bridge: Physical Sheet Music → last-state `.soma.bin` (body 31, `pack_hand`).
//!
//! The Cultivator inverts a [`GraspHoldAttractor`] on the grasp force plant.
//! This module compresses the resulting choreography into the 64-byte hand
//! pinout (`LastStateFrame64` in `last_state.rs`) so a body can carry the score into
//! the Dark Window. Tendon/pad/stretch come from the same algebraic tendon
//! law as `evaluate_hand_tendon_dynamics`; margin and slip come from the
//! inverted grasp plant (the plant that saw the kinetic shear).

use crate::last_state::{self, LastStateFrame64, BODY_HAND};
use crate::physics::dexterous::{
    evaluate_hand_tendon_dynamics, hand_contact_q_sum_max, C_HandTendonState,
    GRASP_CLAMP_N as DEX_CLAMP, N_HAND_FINGERS, TENDON_MOMENT_ARM_M, TENDON_REST_LENGTH_M,
    TENDON_STIFFNESS_N_PER_M, TENDON_STRAIN_WARN, THUMB_OPPOSITION_RAD,
};
use crate::syntropy::attractor::{GraspHoldAttractor, StrangeAttractor};
use crate::syntropy::cultivator::{Cultivator, InvertResult};
use crate::syntropy::dynamics::{
    DiscreteDynamics, GraspForceDynamics, GRASP_CLAMP_N, GRASP_DT, GRASP_FORCE, GRASP_MARGIN,
    GRASP_SLIP_V, HAND_MARGIN, HAND_OPPOSITION, HAND_OVERSTRETCH, HAND_PAD_N, HAND_Q_MCP,
    HAND_SLIP_V, HAND_SPAN, HAND_STRETCH, HAND_TENSION,
};
use crate::syntropy::sheet_music::SheetMusic;

/// Survival horizon bounds [s]. The tape the hand carries into radio death.
pub const SHEAR_CATCH_HORIZON_MIN_S: f64 = 5.0;
pub const SHEAR_CATCH_HORIZON_MAX_S: f64 = 10.0;
/// Default: 8 s inside the 5–10 s window.
pub const SHEAR_CATCH_HORIZON_S: f64 = 8.0;
/// dt = 0.001 s catch window the Cultivator actually shoots. The last hold is then
/// sustained to `horizon_s`. Coarser dt latches `slip_velocity > 0.005` and
/// never recovers (grasp evaluator macro-slip branch).
pub const SHEAR_CATCH_CATCH_S: f64 = 1.0;
pub const SHEAR_CATCH_N_KNOTS: usize = 8;

pub const SHEAR_CATCH_MASS_KG: f64 = 0.72;
pub const SHEAR_CATCH_MU: f64 = 0.68;
pub const SHEAR_CATCH_FORCE0_N: f64 = 10.0;
/// Seed below the 5 mm/s macro-slip latch so stick recovery can still fire.
pub const SHEAR_CATCH_SLIP0_M_S: f64 = 0.003;
/// Extra pad shear [N] on top of `m g` — the kinetic load of a slipping catch.
pub const SHEAR_CATCH_DISTURBANCE_N: f64 = 8.0;
pub const SHEAR_CATCH_TARGET_MARGIN: f64 = 0.40;
pub const SHEAR_CATCH_SPAN_M: f64 = 0.040;

pub const HAND_SOMA_RESERVED: [u8; 8] = *b"HAND0001";

/// Named catch: heavy object already slipping, shear spike, 45 N clamp.
#[derive(Clone, Debug)]
pub struct ShearCatchScenario {
    pub horizon_s: f64,
    pub mass_kg: f64,
    pub mu: f64,
    pub force0_n: f64,
    pub slip0_m_s: f64,
    pub disturbance_n: f64,
    pub target_margin: f64,
    pub object_span_m: f64,
    pub opposition_rad: f64,
}

impl Default for ShearCatchScenario {
    fn default() -> Self {
        Self {
            horizon_s: SHEAR_CATCH_HORIZON_S,
            mass_kg: SHEAR_CATCH_MASS_KG,
            mu: SHEAR_CATCH_MU,
            force0_n: SHEAR_CATCH_FORCE0_N,
            slip0_m_s: SHEAR_CATCH_SLIP0_M_S,
            disturbance_n: SHEAR_CATCH_DISTURBANCE_N,
            target_margin: SHEAR_CATCH_TARGET_MARGIN,
            object_span_m: SHEAR_CATCH_SPAN_M,
            opposition_rad: THUMB_OPPOSITION_RAD as f64,
        }
    }
}

impl ShearCatchScenario {
    pub fn clamped_horizon_s(&self) -> f64 {
        self.horizon_s
            .clamp(SHEAR_CATCH_HORIZON_MIN_S, SHEAR_CATCH_HORIZON_MAX_S)
    }

    pub fn plant(&self) -> GraspForceDynamics {
        GraspForceDynamics::new(self.disturbance_n)
    }

    pub fn attractor(&self) -> GraspHoldAttractor {
        GraspHoldAttractor::new(self.target_margin, 0.0)
    }

    pub fn x0(&self) -> Vec<f64> {
        GraspForceDynamics::catching_state(self.mass_kg, self.mu, self.force0_n, self.slip0_m_s)
    }

    /// `m g` + kinetic shear the commander names (before taxel spatial gain).
    pub fn total_shear_n(&self) -> f64 {
        self.mass_kg * 9.81 + self.disturbance_n
    }

    /// GraspForceDynamics taxel pattern: Σshear / (mg+disturb) ≈ 18.75/16.
    pub fn pad_shear_n(&self) -> f64 {
        self.total_shear_n() * (18.75 / 16.0)
    }

    /// Force for attractor margin 0.40 under the taxel-gain cone, clamped at 45 N.
    pub fn hold_force_n(&self) -> f64 {
        let w = (1.0 - self.target_margin).max(0.05);
        (self.pad_shear_n() / (w * self.mu)).clamp(0.0, GRASP_CLAMP_N)
    }
}

/// Invert the shear-catch attractor at dt = 0.001 s, then sustain the last hold to the horizon.
pub fn invert_shear_catch(scenario: &ShearCatchScenario) -> InvertResult {
    let horizon_s = scenario.clamped_horizon_s();
    let plant = scenario.plant();
    let attr = scenario.attractor();
    let x0 = scenario.x0();
    let catch_s = SHEAR_CATCH_CATCH_S.min(horizon_s);
    let n_catch = ((catch_s / GRASP_DT).round() as usize).max(1);
    let cult = Cultivator {
        horizon: n_catch,
        dt: GRASP_DT,
        n_knots: Some(SHEAR_CATCH_N_KNOTS.min(n_catch).max(1)),
        max_iters: 40,
        step_size: 12.0,
        fd_eps: 2e-2,
        lambda_energy: 1e-5,
        lambda_smooth: 5e-3,
        ..Cultivator::default()
    };
    let mut out = cult.invert(&plant, &attr, &x0, &[scenario.hold_force_n()]);
    out.sheet = out.sheet.extend_last_hold(horizon_s);
    let x_n = rollout_terminal(&plant, &x0, &out.sheet);
    out.terminal_cost = attr.cost(&x_n);
    out.final_state = x_n;
    out
}

fn rollout_terminal(plant: &GraspForceDynamics, x0: &[f64], sheet: &SheetMusic) -> Vec<f64> {
    let mut x = x0.to_vec();
    let nu = plant.actuation_dim().max(1);
    let u = sheet.reconstruct();
    for k in 0..sheet.n_steps {
        let start = k * nu;
        let end = (start + nu).min(u.len());
        plant.step(&mut x, &u[start..end], sheet.dt_s);
    }
    x
}

/// Commanded close [rad] that produces `force_n` pad load under `opposition_rad`.
/// Inverse of `working_n = (close / 1.40) * 45 * sin(opposition)` in the hand evaluator.
pub fn close_rad_for_force(force_n: f64, opposition_rad: f64) -> f64 {
    let pinch = opposition_rad.sin().clamp(0.18, 1.0);
    (force_n.clamp(0.0, GRASP_CLAMP_N) / GRASP_CLAMP_N) * 1.40 / pinch
}

/// Tendon algebra from `evaluate_hand_tendon_dynamics` (stretch, tension, overstretch).
pub fn tendon_from_close(
    close_rad: f64,
    opposition_rad: f64,
    object_span_m: f64,
) -> (f64, f64, bool) {
    let pinch = opposition_rad.sin().clamp(0.18, 1.0);
    let q_sum_max = hand_contact_q_sum_max(object_span_m as f32) as f64;
    let excess_q = (close_rad * 2.18 - q_sum_max).max(0.0);
    let blocked = TENDON_MOMENT_ARM_M as f64 * excess_q;
    let working_n = (close_rad / 1.40 * DEX_CLAMP as f64 * pinch).clamp(0.0, GRASP_CLAMP_N);
    let working_stretch = working_n / TENDON_STIFFNESS_N_PER_M.max(1.0) as f64;
    let stretch = working_stretch + blocked;
    let tension = (TENDON_STIFFNESS_N_PER_M as f64 * stretch).max(0.0);
    let overstretch = blocked / TENDON_REST_LENGTH_M as f64 > TENDON_STRAIN_WARN as f64;
    (tension, stretch, overstretch)
}

fn settle_hand(force_n: f64, scenario: &ShearCatchScenario, slip_m_s: f64) -> C_HandTendonState {
    let close = close_rad_for_force(force_n, scenario.opposition_rad) as f32;
    let mu = scenario.mu.clamp(0.05, 1.5) as f32;
    let mut state = C_HandTendonState {
        q_mcp: [0.06; N_HAND_FINGERS],
        q_pip: [0.04; N_HAND_FINGERS],
        q_dip: [0.03; N_HAND_FINGERS],
        qdot_mcp: [0.0; N_HAND_FINGERS],
        qdot_pip: [0.0; N_HAND_FINGERS],
        qdot_dip: [0.0; N_HAND_FINGERS],
        tendon_stretch_m: 0.0,
        tendon_tension_n: 0.0,
        opposition_rad: scenario.opposition_rad as f32,
        object_span_m: scenario.object_span_m as f32,
        commanded_close_rad: close,
        pad_normal_n: 0.0,
        normal_force: force_n.clamp(0.0, GRASP_CLAMP_N) as f32,
        slip_velocity: slip_m_s.max(0.0) as f32,
        slip_angular_velocity: 0.0,
        object_mass: scenario.mass_kg.max(0.01) as f32,
        static_friction_coeff: mu,
        dynamic_friction_coeff: (mu * 0.8).clamp(0.04, 1.2),
        reflex_active: false,
    };
    for _ in 0..20 {
        let _ = evaluate_hand_tendon_dynamics(&mut state, GRASP_DT as f32);
    }
    state
}

fn pack_event_hand(
    t_s: f64,
    force_n: f64,
    margin: f64,
    slip_m_s: f64,
    scenario: &ShearCatchScenario,
) -> LastStateFrame64 {
    let force_n = force_n.clamp(0.0, GRASP_CLAMP_N);
    let close = close_rad_for_force(force_n, scenario.opposition_rad);
    let (tension_alg, stretch_alg, over_alg) =
        tendon_from_close(close, scenario.opposition_rad, scenario.object_span_m);
    let hand = settle_hand(force_n, scenario, slip_m_s);
    let tension = if hand.tendon_tension_n > 0.0 {
        hand.tendon_tension_n
    } else {
        tension_alg as f32
    };
    let stretch = if hand.tendon_stretch_m > 0.0 {
        hand.tendon_stretch_m
    } else {
        stretch_alg as f32
    };
    let overstretch = over_alg || {
        let blocked = TENDON_MOMENT_ARM_M as f64
            * (close * 2.18 - hand_contact_q_sum_max(scenario.object_span_m as f32) as f64)
                .max(0.0);
        blocked / TENDON_REST_LENGTH_M as f64 > TENDON_STRAIN_WARN as f64
    };
    let pad_slip = slip_m_s.abs() > 0.005 || margin < 0.05;
    LastStateFrame64::pack_hand(
        (t_s * 1000.0).round().clamp(0.0, u32::MAX as f64) as u32,
        tension,
        force_n as f32,
        stretch,
        scenario.opposition_rad as f32,
        hand.q_mcp[0],
        slip_m_s as f32,
        margin as f32,
        scenario.object_span_m as f32,
        overstretch,
        pad_slip,
    )
}

/// One last-state frame per Sheet Music hold (the compressed choreography).
pub fn stamp_hand_frames(
    sheet: &SheetMusic,
    scenario: &ShearCatchScenario,
) -> Vec<LastStateFrame64> {
    let plant = scenario.plant();
    let mut x = scenario.x0();
    let nu = plant.actuation_dim().max(1);
    let u = sheet.reconstruct();
    let mut ends: Vec<usize> = sheet
        .events
        .iter()
        .map(|e| (e.start_step + e.n_steps).min(sheet.n_steps))
        .collect();
    ends.sort_unstable();
    ends.dedup();
    if ends.last().copied() != Some(sheet.n_steps) {
        ends.push(sheet.n_steps);
    }
    let mut frames = Vec::with_capacity(ends.len());
    let mut next = 0usize;
    for k in 0..sheet.n_steps {
        let start = k * nu;
        let end = (start + nu).min(u.len());
        plant.step(&mut x, &u[start..end], sheet.dt_s);
        if next < ends.len() && k + 1 == ends[next] {
            let t_s = (k + 1) as f64 * sheet.dt_s;
            let force = x[GRASP_FORCE].clamp(0.0, GRASP_CLAMP_N);
            frames.push(pack_event_hand(
                t_s,
                force,
                x[GRASP_MARGIN],
                x[GRASP_SLIP_V],
                scenario,
            ));
            next += 1;
        }
    }
    frames
}

/// Body-31 frame from a packed [`crate::syntropy::dynamics::HandTendonDynamics`] state.
pub fn pack_hand_from_kinematic(t_s: f64, state: &[f64]) -> LastStateFrame64 {
    let slip = state.get(HAND_SLIP_V).copied().unwrap_or(0.0);
    LastStateFrame64::pack_hand(
        (t_s * 1000.0).round().clamp(0.0, u32::MAX as f64) as u32,
        state.get(HAND_TENSION).copied().unwrap_or(0.0) as f32,
        state.get(HAND_PAD_N).copied().unwrap_or(0.0) as f32,
        state.get(HAND_STRETCH).copied().unwrap_or(0.0) as f32,
        state.get(HAND_OPPOSITION).copied().unwrap_or(0.0) as f32,
        state.get(HAND_Q_MCP).copied().unwrap_or(0.0) as f32,
        slip as f32,
        state.get(HAND_MARGIN).copied().unwrap_or(0.0) as f32,
        state.get(HAND_SPAN).copied().unwrap_or(0.0) as f32,
        state.get(HAND_OVERSTRETCH).copied().unwrap_or(0.0) > 0.5,
        slip.abs() > 0.005,
    )
}

pub fn write_hand_soma(frames: &[LastStateFrame64]) -> Vec<u8> {
    let bytes: Vec<[u8; 64]> = frames.iter().map(|f| f.to_bytes()).collect();
    last_state::write_soma_file(BODY_HAND, HAND_SOMA_RESERVED, &bytes)
}

pub fn default_soma_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/hand_dark_window.soma.bin")
}

/// Invert, stamp, write. Returns (invert, frames, file bytes).
pub fn forge_dark_window(
    scenario: &ShearCatchScenario,
    path: &std::path::Path,
) -> std::io::Result<(InvertResult, Vec<LastStateFrame64>, Vec<u8>)> {
    let inverted = invert_shear_catch(scenario);
    let frames = stamp_hand_frames(&inverted.sheet, scenario);
    let bin = write_hand_soma(&frames);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, &bin)?;
    Ok((inverted, frames, bin))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::last_state::SPEC_VERSION;
    use sha2::{Digest, Sha256};

    #[test]
    fn close_rad_inverts_working_n() {
        let opp = THUMB_OPPOSITION_RAD as f64;
        let force = 36.9;
        let close = close_rad_for_force(force, opp);
        let pinch = opp.sin().clamp(0.18, 1.0);
        let back = (close / 1.40 * GRASP_CLAMP_N * pinch).clamp(0.0, GRASP_CLAMP_N);
        assert!(
            (back - force).abs() < 1e-6,
            "close {close} recovered {back} N, want {force}"
        );
    }

    #[test]
    fn shear_catch_is_holdable_under_clamp() {
        let s = ShearCatchScenario::default();
        let need = s.hold_force_n();
        assert!(
            need <= GRASP_CLAMP_N,
            "margin-0.40 force {need} N exceeds {GRASP_CLAMP_N} N clamp"
        );
        assert!(s.force0_n * s.mu < s.pad_shear_n());
        assert!(need > 30.0);
    }

    #[test]
    fn invert_stamps_hand_soma_within_clamp() {
        let scenario = ShearCatchScenario::default();
        let out = invert_shear_catch(&scenario);
        let duration = out.sheet.n_steps as f64 * out.sheet.dt_s;
        assert!(
            (duration - scenario.clamped_horizon_s()).abs() < 0.02,
            "duration {duration}"
        );
        assert!((out.sheet.dt_s - GRASP_DT).abs() < 1e-18);
        assert!(!out.sheet.events.is_empty());
        for u in &out.sheet.samples {
            assert!(
                *u >= 0.0 && *u <= GRASP_CLAMP_N + 1e-9,
                "sample {u} N outside 45 N clamp"
            );
        }
        let frames = stamp_hand_frames(&out.sheet, &scenario);
        assert!(!frames.is_empty());
        let last = frames.last().unwrap();
        assert!(last.pos[1] <= DEX_CLAMP + 1e-3);
        assert!(last.residual > 0.0);

        let bin = write_hand_soma(&frames);
        assert_eq!(&bin[0..4], b"SOMA");
        let spec = u16::from_le_bytes([bin[4], bin[5]]);
        assert_eq!(spec, SPEC_VERSION);
        let body = u16::from_le_bytes([bin[6], bin[7]]);
        assert_eq!(body, BODY_HAND);
        let nframes = u64::from_le_bytes(bin[16..24].try_into().unwrap());
        assert_eq!(nframes as usize, frames.len());
        assert_eq!(&bin[56..64], &HAND_SOMA_RESERVED);
        let digest = Sha256::digest(&bin[64..]);
        assert_eq!(&bin[24..56], digest.as_slice());
        assert_ne!(&bin[64..68], b"SOMA");
        assert_eq!(bin.len(), 64 + frames.len() * 64);

        let peak: f64 = out.sheet.samples.iter().copied().fold(0.0, f64::max);
        assert!(peak <= GRASP_CLAMP_N + 1e-9);
        assert!(
            out.final_state[GRASP_SLIP_V] < 0.02,
            "slip {} after invert",
            out.final_state[GRASP_SLIP_V]
        );
        assert!(
            out.final_state[GRASP_MARGIN] > 0.15,
            "terminal margin {}",
            out.final_state[GRASP_MARGIN]
        );
    }
}
