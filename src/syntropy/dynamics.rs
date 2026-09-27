//! Discrete dynamics plants. Sample domain: tactile grasp at host dt=0.001.
//!
//! Grasp taxel loading is copied from `src/bin/grasp_loop_trace.rs` (lines 71–79).
//! The step itself is `physics::dexterous::evaluate_grasp_dynamics` — the same
//! function the Topic 4 Monte Carlo and the C FFI export call.
//!
//! Mycelial and terran adapters are reduced-order but call the disk equations
//! (`MycelialMesh::step_signal` / `step_nutrients`, `SoilProfile::effective_yield_stress`).

use crate::physics::dexterous::{
    evaluate_grasp_dynamics, evaluate_hand_tendon_dynamics, hand_contact_q_sum_max, C_GraspState,
    C_HandTendonResult, C_HandTendonState, C_TactileArray, Taxel, GRASP_CLAMP_N as DEX_CLAMP,
    N_HAND_FINGERS, TENDON_MOMENT_ARM_M, TENDON_REST_LENGTH_M, TENDON_STRAIN_WARN,
    THUMB_OPPOSITION_RAD,
};
use crate::physics::mycelial::{HyphalEdge, MycelialMesh, MycelialNode};
use crate::physics::terran::{SoilProfile, SoilType};

use super::sheet_music::{ActuationChannel, ChannelKind};

/// Grasp / chassis body-time [s]. Host dt = 0.001.
pub const GRASP_DT: f64 = 0.001;
pub const GRASP_CLAMP_N: f64 = DEX_CLAMP as f64;
pub const GRASP_STATE_DIM: usize = 8;
/// Mycelial Kirchhoff clock. 10 Hz.
pub const MYCELIAL_DT: f64 = 0.1;

pub const GRASP_FORCE: usize = 0;
pub const GRASP_MARGIN: usize = 1;
pub const GRASP_SLIP_V: usize = 2;
pub const GRASP_MASS: usize = 3;
pub const GRASP_MU_S: usize = 4;
pub const GRASP_MU_D: usize = 5;
pub const GRASP_SLIP_W: usize = 6;
pub const GRASP_REFLEX: usize = 7;

/// Discrete plant: `x_{k+1} = f(x_k, u_k, dt)`.
pub trait DiscreteDynamics {
    fn state_dim(&self) -> usize;
    fn actuation_dim(&self) -> usize {
        self.channels().len()
    }
    fn channels(&self) -> &[ActuationChannel];
    fn actuation_bounds(&self) -> &[(f64, f64)];
    fn step(&self, state: &mut [f64], actuation: &[f64], dt: f64);
    /// Optional analytic `∂f/∂x`, `∂f/∂u` (row-major). Default: finite differences.
    fn jacobian(
        &self,
        _state: &[f64],
        _actuation: &[f64],
        _dt: f64,
    ) -> Option<(Vec<f64>, Vec<f64>)> {
        None
    }
}

const GRASP_CHANNELS: [ActuationChannel; 1] = [ActuationChannel {
    name: "grip_force",
    unit: "N",
    kind: ChannelKind::Force,
}];

const GRASP_BOUNDS: [(f64, f64); 1] = [(0.0, GRASP_CLAMP_N)];

/// Tactile grasp force plant.
///
/// Actuation `u[0]` is applied pad force [N], spread across 16 taxels:
/// `n = F/16`, `shear = (m g + disturbance)/16`, then the same spatial shear
/// pattern as `grasp_loop_trace.rs`. Inner reflex may raise `state.normal_force`;
/// the next tick still applies the next `u` (outer commander).
#[derive(Clone, Debug)]
pub struct GraspForceDynamics {
    /// Extra shear [N] on the object, same role as `disturbances` in the trace bin.
    pub disturbance_n: f64,
    channels: [ActuationChannel; 1],
    bounds: [(f64, f64); 1],
}

impl Default for GraspForceDynamics {
    fn default() -> Self {
        Self {
            disturbance_n: 0.0,
            channels: GRASP_CHANNELS,
            bounds: GRASP_BOUNDS,
        }
    }
}

impl GraspForceDynamics {
    pub fn new(disturbance_n: f64) -> Self {
        Self {
            disturbance_n,
            ..Self::default()
        }
    }

    /// Resting grasp state. Margin starts at 0 until the first step observes it.
    pub fn idle_state(mass_kg: f64, mu: f64, force_n: f64) -> Vec<f64> {
        Self::catching_state(mass_kg, mu, force_n, 0.0)
    }

    /// Grasp already in motion: same idle cone, nonzero linear slip [m/s].
    pub fn catching_state(mass_kg: f64, mu: f64, force_n: f64, slip_m_s: f64) -> Vec<f64> {
        let mu = mu.clamp(0.05, 1.5);
        vec![
            force_n.clamp(0.0, GRASP_CLAMP_N),
            0.0,
            slip_m_s.max(0.0),
            mass_kg.max(0.01),
            mu,
            (mu * 0.8).clamp(0.04, 1.2),
            0.0,
            0.0,
        ]
    }

    /// Taxel construction: `src/bin/grasp_loop_trace.rs` 71–79 (disturbance untimed).
    fn load_taxels(&self, force_n: f32, mass_kg: f32) -> C_TactileArray {
        let n = force_n / 16.0;
        let shear = (mass_kg * 9.81 + self.disturbance_n as f32) / 16.0;
        let mut taxels = [Taxel {
            normal: n,
            shear_x: 0.0,
            shear_y: 0.0,
        }; 16];
        for i in 0..16 {
            taxels[i].shear_x = shear * (1.0 + 0.1 * (i as f32 % 4.0));
            taxels[i].shear_y = shear * 0.15 * (i as f32 / 4.0);
        }
        C_TactileArray { taxels }
    }
}

impl DiscreteDynamics for GraspForceDynamics {
    fn state_dim(&self) -> usize {
        GRASP_STATE_DIM
    }

    fn channels(&self) -> &[ActuationChannel] {
        &self.channels
    }

    fn actuation_bounds(&self) -> &[(f64, f64)] {
        &self.bounds
    }

    fn step(&self, state: &mut [f64], actuation: &[f64], dt: f64) {
        debug_assert!(state.len() >= GRASP_STATE_DIM);
        debug_assert!(!actuation.is_empty());
        let force_cmd = actuation[0].clamp(0.0, GRASP_CLAMP_N);
        let mass = state[GRASP_MASS].max(0.01);
        let mu_s = state[GRASP_MU_S].clamp(0.05, 1.5) as f32;
        let mut grasp = C_GraspState {
            normal_force: force_cmd as f32,
            slip_velocity: state[GRASP_SLIP_V] as f32,
            slip_angular_velocity: state[GRASP_SLIP_W] as f32,
            object_mass: mass as f32,
            static_friction_coeff: mu_s,
            dynamic_friction_coeff: state[GRASP_MU_D].clamp(0.04, 1.2) as f32,
            reflex_active: state[GRASP_REFLEX] > 0.5,
        };
        let sensor = self.load_taxels(force_cmd as f32, mass as f32);
        let res = evaluate_grasp_dynamics(&sensor, &mut grasp, dt.max(1e-9) as f32);
        state[GRASP_FORCE] = grasp.normal_force as f64;
        state[GRASP_MARGIN] = res.margin as f64;
        state[GRASP_SLIP_V] = grasp.slip_velocity as f64;
        state[GRASP_MASS] = mass;
        state[GRASP_MU_S] = grasp.static_friction_coeff as f64;
        state[GRASP_MU_D] = grasp.dynamic_friction_coeff as f64;
        state[GRASP_SLIP_W] = grasp.slip_angular_velocity as f64;
        state[GRASP_REFLEX] = if grasp.reflex_active { 1.0 } else { 0.0 };
    }
}

const MYCELIAL_CHANNELS: [ActuationChannel; 2] = [
    ActuationChannel {
        name: "hyphal_health_rate",
        unit: "1/s",
        kind: ChannelKind::Concentration,
    },
    ActuationChannel {
        name: "source_nutrient_rate",
        unit: "1/s",
        kind: ChannelKind::Nutrient,
    },
];

const MYCELIAL_BOUNDS: [(f64, f64); 2] = [(-0.5, 0.5), (0.0, 2.0)];

/// Three-node Kirchhoff line (source–mid–sink) on [`MycelialMesh`].
///
/// Geometry is fixed (no RNG). Edges are 50 m so conductance = health
/// (`HyphalEdge::conductance`, `physics/mycelial.rs`). Clock 10 Hz.
///
/// State: signal[3], nutrient[3], edge_health[2], delivered_nutrient.
/// `delivered_nutrient`:= `MycelialMesh::delivery_ratio` (sink/source signal).
#[derive(Clone, Debug)]
pub struct MycelialLineDynamics {
    channels: [ActuationChannel; 2],
    bounds: [(f64, f64); 2],
}

impl Default for MycelialLineDynamics {
    fn default() -> Self {
        Self {
            channels: MYCELIAL_CHANNELS,
            bounds: MYCELIAL_BOUNDS,
        }
    }
}

impl MycelialLineDynamics {
    pub const STATE_DIM: usize = 9;
    pub const DELIVERED: usize = 8;

    pub fn idle_state(edge_health: f64) -> Vec<f64> {
        let h = edge_health.clamp(0.05, 1.0);
        vec![
            1.0, 0.0, 0.0, // signal source, mid, sink
            0.8, 0.0, 0.0, // nutrient
            h, h,   // two edges
            0.0, // delivered (observed after first step)
        ]
    }

    fn mesh_from_state(&self, state: &[f64]) -> MycelialMesh {
        let h0 = state[6].clamp(0.0, 1.0);
        let h1 = state[7].clamp(0.0, 1.0);
        MycelialMesh {
            nodes: vec![
                MycelialNode {
                    position: [0.0, 0.0],
                    health_index: h0,
                    nutrient_density: state[3],
                    is_source: true,
                    is_sink: false,
                    signal_level: state[0],
                },
                MycelialNode {
                    position: [50.0, 0.0],
                    health_index: (h0 + h1) * 0.5,
                    nutrient_density: state[4],
                    is_source: false,
                    is_sink: false,
                    signal_level: state[1],
                },
                MycelialNode {
                    position: [100.0, 0.0],
                    health_index: h1,
                    nutrient_density: state[5],
                    is_source: false,
                    is_sink: true,
                    signal_level: state[2],
                },
            ],
            edges: vec![
                HyphalEdge {
                    from: 0,
                    to: 1,
                    length: 50.0,
                    health: h0,
                    alive: h0 > 0.01,
                },
                HyphalEdge {
                    from: 1,
                    to: 2,
                    length: 50.0,
                    health: h1,
                    alive: h1 > 0.01,
                },
            ],
            propagation_rate: 0.05,
            decay_rate: 0.01,
            network_radius: 100.0,
        }
    }

    fn pack(&self, mesh: &MycelialMesh, state: &mut [f64]) {
        for i in 0..3 {
            state[i] = mesh.nodes[i].signal_level;
            state[3 + i] = mesh.nodes[i].nutrient_density;
        }
        state[6] = mesh.edges[0].health;
        state[7] = mesh.edges[1].health;
        state[8] = mesh.delivery_ratio();
    }
}

impl DiscreteDynamics for MycelialLineDynamics {
    fn state_dim(&self) -> usize {
        Self::STATE_DIM
    }

    fn channels(&self) -> &[ActuationChannel] {
        &self.channels
    }

    fn actuation_bounds(&self) -> &[(f64, f64)] {
        &self.bounds
    }

    fn step(&self, state: &mut [f64], actuation: &[f64], dt: f64) {
        debug_assert!(state.len() >= Self::STATE_DIM);
        let dt = dt.max(1e-9);
        let dh = actuation.first().copied().unwrap_or(0.0) * dt;
        let dn = actuation.get(1).copied().unwrap_or(0.0) * dt;
        state[6] = (state[6] + dh).clamp(0.0, 1.0);
        state[7] = (state[7] + dh).clamp(0.0, 1.0);
        state[3] = (state[3] + dn).clamp(0.0, 2.0);
        let mut mesh = self.mesh_from_state(state);
        mesh.step_signal(dt);
        mesh.step_nutrients(dt);
        self.pack(&mesh, state);
    }
}

const GLOMALIN_CHANNELS: [ActuationChannel; 1] = [ActuationChannel {
    name: "glomalin_exudation",
    unit: "mg/g/s",
    kind: ChannelKind::Concentration,
}];

const GLOMALIN_BOUNDS: [(f64, f64); 1] = [(-0.05, 0.20)];

/// Reduced-order soil plant: exudation integrates `glomalin_mg_g`, yield stress
/// is the algebraic disk law [`SoilProfile::effective_yield_stress`]
/// (`physics/terran.rs` 158–171). Not a dt = 0.001 s Reflex organ — soil is slow.
/// Clock is caller-chosen; tests use `dt = 1 s`.
#[derive(Clone, Debug)]
pub struct GlomalinExudationDynamics {
    pub soil_type: SoilType,
    pub moisture: f64,
    channels: [ActuationChannel; 1],
    bounds: [(f64, f64); 1],
}

impl GlomalinExudationDynamics {
    pub const STATE_DIM: usize = 2;
    pub const GLOMALIN: usize = 0;
    pub const YIELD_STRESS: usize = 1;

    pub fn andisol(moisture: f64) -> Self {
        Self {
            soil_type: SoilType::Andisol,
            moisture,
            channels: GLOMALIN_CHANNELS,
            bounds: GLOMALIN_BOUNDS,
        }
    }

    pub fn idle_state(&self, glomalin_mg_g: f64) -> Vec<f64> {
        let g = glomalin_mg_g.max(0.0);
        let y = self.yield_stress_pa(g);
        vec![g, y]
    }

    fn yield_stress_pa(&self, glomalin_mg_g: f64) -> f64 {
        let soil = SoilProfile {
            soil_type: self.soil_type,
            moisture: self.moisture,
            glomalin_mg_g,
            compaction: 0.0,
            depth_layers: 8,
        };
        soil.effective_yield_stress()
    }
}

impl DiscreteDynamics for GlomalinExudationDynamics {
    fn state_dim(&self) -> usize {
        Self::STATE_DIM
    }

    fn channels(&self) -> &[ActuationChannel] {
        &self.channels
    }

    fn actuation_bounds(&self) -> &[(f64, f64)] {
        &self.bounds
    }

    fn step(&self, state: &mut [f64], actuation: &[f64], dt: f64) {
        debug_assert!(state.len() >= Self::STATE_DIM);
        let u = actuation.first().copied().unwrap_or(0.0);
        let g = (state[0] + u * dt).clamp(0.0, 20.0);
        state[0] = g;
        state[1] = self.yield_stress_pa(g);
    }
}

/// Full kinematic hand. 5 fingers × {MCP, PIP, DIP} + tendon, wrapping
/// [`evaluate_hand_tendon_dynamics`]. Actuation is `commanded_close_rad`
/// and optionally `opposition_rad` (pinch). Clock is [`GRASP_DT`].
///
/// Layout (47 × f64). Joints are finger-major, thumb = index 0:
/// `q_mcp[5] · q_pip[5] · q_dip[5] · qdot_mcp[5] · qdot_pip[5] · qdot_dip[5]`
/// then tendon / cone / attractor slots below.
pub const HAND_STATE_DIM: usize = 47;
pub const HAND_Q_MCP: usize = 0;
pub const HAND_Q_PIP: usize = 5;
pub const HAND_Q_DIP: usize = 10;
pub const HAND_QDOT_MCP: usize = 15;
pub const HAND_QDOT_PIP: usize = 20;
pub const HAND_QDOT_DIP: usize = 25;
pub const HAND_STRETCH: usize = 30;
pub const HAND_TENSION: usize = 31;
pub const HAND_OPPOSITION: usize = 32;
pub const HAND_SPAN: usize = 33;
pub const HAND_CLOSE: usize = 34;
pub const HAND_PAD_N: usize = 35;
pub const HAND_FORCE: usize = 36;
pub const HAND_SLIP_V: usize = 37;
pub const HAND_SLIP_W: usize = 38;
pub const HAND_MASS: usize = 39;
pub const HAND_MU_S: usize = 40;
pub const HAND_MU_D: usize = 41;
pub const HAND_REFLEX: usize = 42;
pub const HAND_MARGIN: usize = 43;
pub const HAND_STRAIN: usize = 44;
/// `blocked_stretch / TENDON_REST_LENGTH` — the overstretch observable.
pub const HAND_BLOCKED_STRAIN: usize = 45;
pub const HAND_OVERSTRETCH: usize = 46;

const HAND_CLOSE_LO: f64 = 0.05;
const HAND_CLOSE_HI: f64 = 1.55;
const HAND_OPP_LO: f64 = 0.35;
const HAND_OPP_HI: f64 = 1.35;

const HAND_CHANNELS_BOTH: [ActuationChannel; 2] = [
    ActuationChannel {
        name: "commanded_close_rad",
        unit: "rad",
        kind: ChannelKind::Angle,
    },
    ActuationChannel {
        name: "opposition_rad",
        unit: "rad",
        kind: ChannelKind::Angle,
    },
];

const HAND_BOUNDS_BOTH: [(f64, f64); 2] =
    [(HAND_CLOSE_LO, HAND_CLOSE_HI), (HAND_OPP_LO, HAND_OPP_HI)];

fn copy5(state: &[f64], off: usize) -> [f32; N_HAND_FINGERS] {
    let mut a = [0.0f32; N_HAND_FINGERS];
    for (i, slot) in a.iter_mut().enumerate() {
        *slot = state.get(off + i).copied().unwrap_or(0.0) as f32;
    }
    a
}

fn write5(state: &mut [f64], off: usize, q: &[f32; N_HAND_FINGERS]) {
    for i in 0..N_HAND_FINGERS {
        if off + i < state.len() {
            state[off + i] = q[i] as f64;
        }
    }
}

/// Blocked-chain strain: `blocked_stretch / rest_length`. Same gate as
/// `tendon_overstretch` in `evaluate_hand_tendon_dynamics`.
pub fn hand_blocked_strain(close_rad: f64, span_m: f64) -> f64 {
    let q_sum_max = hand_contact_q_sum_max(span_m as f32) as f64;
    let excess_q = (close_rad * 2.18 - q_sum_max).max(0.0);
    let blocked = TENDON_MOMENT_ARM_M as f64 * excess_q;
    blocked / TENDON_REST_LENGTH_M as f64
}

/// 5-finger serial-chain plant. Default actuation is close + opposition.
#[derive(Clone, Debug)]
pub struct HandTendonDynamics {
    n_u: usize,
    channels: [ActuationChannel; 2],
    bounds: [(f64, f64); 2],
}

impl Default for HandTendonDynamics {
    fn default() -> Self {
        Self::close_and_opposition()
    }
}

impl HandTendonDynamics {
    pub const STATE_DIM: usize = HAND_STATE_DIM;

    /// `u = [commanded_close_rad, opposition_rad]`.
    pub fn close_and_opposition() -> Self {
        Self {
            n_u: 2,
            channels: HAND_CHANNELS_BOTH,
            bounds: HAND_BOUNDS_BOTH,
        }
    }

    /// Close only. Opposition stays whatever the state already holds.
    pub fn close_only() -> Self {
        Self {
            n_u: 1,
            channels: HAND_CHANNELS_BOTH,
            bounds: HAND_BOUNDS_BOTH,
        }
    }

    pub fn unpack(state: &[f64]) -> C_HandTendonState {
        let mu = state
            .get(HAND_MU_S)
            .copied()
            .unwrap_or(0.55)
            .clamp(0.05, 1.5);
        C_HandTendonState {
            q_mcp: copy5(state, HAND_Q_MCP),
            q_pip: copy5(state, HAND_Q_PIP),
            q_dip: copy5(state, HAND_Q_DIP),
            qdot_mcp: copy5(state, HAND_QDOT_MCP),
            qdot_pip: copy5(state, HAND_QDOT_PIP),
            qdot_dip: copy5(state, HAND_QDOT_DIP),
            tendon_stretch_m: state.get(HAND_STRETCH).copied().unwrap_or(0.0) as f32,
            tendon_tension_n: state.get(HAND_TENSION).copied().unwrap_or(0.0) as f32,
            opposition_rad: state
                .get(HAND_OPPOSITION)
                .copied()
                .unwrap_or(THUMB_OPPOSITION_RAD as f64) as f32,
            object_span_m: state.get(HAND_SPAN).copied().unwrap_or(0.04) as f32,
            commanded_close_rad: state.get(HAND_CLOSE).copied().unwrap_or(0.4) as f32,
            pad_normal_n: state.get(HAND_PAD_N).copied().unwrap_or(0.0) as f32,
            normal_force: state.get(HAND_FORCE).copied().unwrap_or(0.0) as f32,
            slip_velocity: state.get(HAND_SLIP_V).copied().unwrap_or(0.0) as f32,
            slip_angular_velocity: state.get(HAND_SLIP_W).copied().unwrap_or(0.0) as f32,
            object_mass: state.get(HAND_MASS).copied().unwrap_or(0.4).max(0.01) as f32,
            static_friction_coeff: mu as f32,
            dynamic_friction_coeff: state
                .get(HAND_MU_D)
                .copied()
                .unwrap_or(mu * 0.8)
                .clamp(0.04, 1.2) as f32,
            reflex_active: state.get(HAND_REFLEX).copied().unwrap_or(0.0) > 0.5,
        }
    }

    pub fn pack(hand: &C_HandTendonState, res: &C_HandTendonResult) -> Vec<f64> {
        let mut s = vec![0.0; HAND_STATE_DIM];
        Self::write(&mut s, hand, res);
        s
    }

    fn write(state: &mut [f64], hand: &C_HandTendonState, res: &C_HandTendonResult) {
        debug_assert!(state.len() >= HAND_STATE_DIM);
        write5(state, HAND_Q_MCP, &hand.q_mcp);
        write5(state, HAND_Q_PIP, &hand.q_pip);
        write5(state, HAND_Q_DIP, &hand.q_dip);
        write5(state, HAND_QDOT_MCP, &hand.qdot_mcp);
        write5(state, HAND_QDOT_PIP, &hand.qdot_pip);
        write5(state, HAND_QDOT_DIP, &hand.qdot_dip);
        state[HAND_STRETCH] = hand.tendon_stretch_m as f64;
        state[HAND_TENSION] = hand.tendon_tension_n as f64;
        state[HAND_OPPOSITION] = hand.opposition_rad as f64;
        state[HAND_SPAN] = hand.object_span_m as f64;
        state[HAND_CLOSE] = hand.commanded_close_rad as f64;
        state[HAND_PAD_N] = hand.pad_normal_n as f64;
        state[HAND_FORCE] = hand.normal_force as f64;
        state[HAND_SLIP_V] = hand.slip_velocity as f64;
        state[HAND_SLIP_W] = hand.slip_angular_velocity as f64;
        state[HAND_MASS] = hand.object_mass as f64;
        state[HAND_MU_S] = hand.static_friction_coeff as f64;
        state[HAND_MU_D] = hand.dynamic_friction_coeff as f64;
        state[HAND_REFLEX] = if hand.reflex_active { 1.0 } else { 0.0 };
        state[HAND_MARGIN] = res.margin as f64;
        state[HAND_STRAIN] = res.strain as f64;
        let blocked =
            hand_blocked_strain(hand.commanded_close_rad as f64, hand.object_span_m as f64);
        state[HAND_BLOCKED_STRAIN] = blocked;
        state[HAND_OVERSTRETCH] = if res.tendon_overstretch || blocked > TENDON_STRAIN_WARN as f64 {
            1.0
        } else {
            0.0
        };
    }

    /// Weak wrap on a named object. Joints start near rest; slip may already be on.
    pub fn idle_state(
        mass_kg: f64,
        mu: f64,
        span_m: f64,
        close_rad: f64,
        opposition_rad: f64,
        slip_m_s: f64,
    ) -> Vec<f64> {
        let mu = mu.clamp(0.05, 1.5);
        let close = close_rad.clamp(HAND_CLOSE_LO, HAND_CLOSE_HI);
        let opp = opposition_rad.clamp(HAND_OPP_LO, HAND_OPP_HI);
        let hand = C_HandTendonState {
            q_mcp: [0.06; N_HAND_FINGERS],
            q_pip: [0.04; N_HAND_FINGERS],
            q_dip: [0.03; N_HAND_FINGERS],
            qdot_mcp: [0.0; N_HAND_FINGERS],
            qdot_pip: [0.0; N_HAND_FINGERS],
            qdot_dip: [0.0; N_HAND_FINGERS],
            tendon_stretch_m: 0.0,
            tendon_tension_n: 0.0,
            opposition_rad: opp as f32,
            object_span_m: span_m.clamp(0.012, 0.090) as f32,
            commanded_close_rad: close as f32,
            pad_normal_n: 0.0,
            normal_force: 0.0,
            slip_velocity: slip_m_s.max(0.0) as f32,
            slip_angular_velocity: 0.0,
            object_mass: mass_kg.max(0.01) as f32,
            static_friction_coeff: mu as f32,
            dynamic_friction_coeff: (mu * 0.8).clamp(0.04, 1.2) as f32,
            reflex_active: false,
        };
        let res = C_HandTendonResult {
            tendon_overstretch: false,
            pad_slip: slip_m_s > 0.005,
            commanded_force: 0.0,
            margin: 0.0,
            tendon_tension_n: 0.0,
            pad_normal_n: 0.0,
            stretch_m: 0.0,
            strain: 0.0,
        };
        Self::pack(&hand, &res)
    }
}

impl DiscreteDynamics for HandTendonDynamics {
    fn state_dim(&self) -> usize {
        HAND_STATE_DIM
    }

    fn channels(&self) -> &[ActuationChannel] {
        &self.channels[..self.n_u]
    }

    fn actuation_bounds(&self) -> &[(f64, f64)] {
        &self.bounds[..self.n_u]
    }

    fn step(&self, state: &mut [f64], actuation: &[f64], dt: f64) {
        debug_assert!(state.len() >= HAND_STATE_DIM);
        let mut hand = Self::unpack(state);
        if let Some(close) = actuation.first() {
            hand.commanded_close_rad = close.clamp(HAND_CLOSE_LO, HAND_CLOSE_HI) as f32;
        }
        if self.n_u > 1 {
            if let Some(opp) = actuation.get(1) {
                hand.opposition_rad = opp.clamp(HAND_OPP_LO, HAND_OPP_HI) as f32;
            }
        }
        let res = evaluate_hand_tendon_dynamics(&mut hand, dt.max(1e-9) as f32);
        Self::write(state, &hand, &res);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grasp_step_writes_margin_from_real_evaluator() {
        let plant = GraspForceDynamics::default();
        let mut x = GraspForceDynamics::idle_state(0.4, 0.6, 20.0);
        plant.step(&mut x, &[20.0], GRASP_DT);
        assert!(
            x[GRASP_MARGIN] > 0.0,
            "20 N on 0.4 kg at μ=0.6 must have cone margin"
        );
        assert!(x[GRASP_FORCE] > 0.0);
    }

    #[test]
    fn higher_grip_raises_margin_in_stick() {
        let plant = GraspForceDynamics::default();
        let mut lo = GraspForceDynamics::idle_state(0.4, 0.6, 12.0);
        let mut hi = GraspForceDynamics::idle_state(0.4, 0.6, 30.0);
        plant.step(&mut lo, &[12.0], GRASP_DT);
        plant.step(&mut hi, &[30.0], GRASP_DT);
        assert!(
            hi[GRASP_MARGIN] > lo[GRASP_MARGIN],
            "hi {} lo {}",
            hi[GRASP_MARGIN],
            lo[GRASP_MARGIN]
        );
    }

    #[test]
    fn mycelial_health_raises_delivery() {
        let plant = MycelialLineDynamics::default();
        let mut weak = MycelialLineDynamics::idle_state(0.15);
        let mut strong = MycelialLineDynamics::idle_state(0.95);
        for _ in 0..80 {
            plant.step(&mut weak, &[0.0, 0.0], MYCELIAL_DT);
            plant.step(&mut strong, &[0.0, 0.0], MYCELIAL_DT);
        }
        assert!(
            strong[MycelialLineDynamics::DELIVERED] > weak[MycelialLineDynamics::DELIVERED],
            "strong {} weak {}",
            strong[MycelialLineDynamics::DELIVERED],
            weak[MycelialLineDynamics::DELIVERED]
        );
    }

    #[test]
    fn glomalin_maps_to_terran_yield_stress() {
        let plant = GlomalinExudationDynamics::andisol(0.20);
        let mut x = plant.idle_state(2.0);
        let expected = plant.yield_stress_pa(2.0);
        assert!((x[1] - expected).abs() < 1e-9);
        plant.step(&mut x, &[0.10], 1.0);
        assert!((x[0] - 2.1).abs() < 1e-12);
        assert!((x[1] - plant.yield_stress_pa(2.1)).abs() < 1e-9);
    }

    #[test]
    fn hand_step_writes_five_finger_joints() {
        let plant = HandTendonDynamics::close_and_opposition();
        let mut x = HandTendonDynamics::idle_state(0.4, 0.55, 0.035, 0.80, 1.047, 0.0);
        for _ in 0..40 {
            plant.step(&mut x, &[0.80, 1.047], GRASP_DT);
        }
        assert_eq!(x.len(), HAND_STATE_DIM);
        let thumb = x[HAND_Q_MCP];
        let index = x[HAND_Q_MCP + 1];
        assert!(thumb > 0.05 && thumb < 1.6, "thumb q_mcp {thumb}");
        assert!(index > 0.05 && index < 1.6, "index q_mcp {index}");
        assert!(
            (thumb - index).abs() > 1e-4,
            "thumb scale/opposition must split MCP from the fingers"
        );
        for f in 0..N_HAND_FINGERS {
            let q = x[HAND_Q_MCP + f] + x[HAND_Q_PIP + f] + x[HAND_Q_DIP + f];
            assert!(q > 0.05, "finger {f} chain {q}");
        }
        assert!(x[HAND_PAD_N] > 0.0);
        assert!(x[HAND_MARGIN] >= 0.0);
    }

    #[test]
    fn hand_pack_roundtrip_preserves_span_and_mass() {
        let x0 = HandTendonDynamics::idle_state(0.62, 0.40, 0.028, 0.42, 0.80, 0.03);
        let hand = HandTendonDynamics::unpack(&x0);
        assert!((hand.object_mass as f64 - 0.62).abs() < 1e-5);
        assert!((hand.object_span_m as f64 - 0.028).abs() < 1e-5);
        assert!((hand.commanded_close_rad as f64 - 0.42).abs() < 1e-5);
        assert_eq!(hand.q_mcp.len(), N_HAND_FINGERS);
    }
}
