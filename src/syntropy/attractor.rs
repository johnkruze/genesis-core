//! Strange Attractor — a desired physical end-state in SI / domain units.
//!
//! Domain-agnostic at the trait. Concrete attractors name observables the forward
//! plants already expose (grasp margin, mycelial `delivery_ratio`, terran
//! `effective_yield_stress`).

/// Desired physical end-state. Every target is a measurable scalar or vector.
pub trait StrangeAttractor {
    fn name(&self) -> &'static str;
    /// Observables extracted from the plant state vector (SI / domain units).
    fn observe(&self, state: &[f64]) -> Vec<f64>;
    fn target(&self) -> &[f64];
    fn weights(&self) -> &[f64];
    /// Weighted squared distance from `observe(state)` to `target`.
    fn cost(&self, state: &[f64]) -> f64 {
        let y = self.observe(state);
        let t = self.target();
        let w = self.weights();
        let n = y.len().min(t.len()).min(w.len());
        let mut j = 0.0;
        for i in 0..n {
            let d = y[i] - t[i];
            j += w[i] * d * d;
        }
        j
    }
}

/// Grasp hold: friction-cone margin and linear slip (SOMA body-28 `pos` slots).
///
/// `state[1] = margin` (0 = slipping, 1 = secure), `state[2] = slip_velocity` [m/s].
/// Both are written by [`crate::physics::dexterous::evaluate_grasp_dynamics`].
#[derive(Clone, Debug)]
pub struct GraspHoldAttractor {
    pub target_margin: f64,
    pub target_slip_m_s: f64,
    target: [f64; 2],
    weights: [f64; 2],
}

impl GraspHoldAttractor {
    pub fn new(target_margin: f64, target_slip_m_s: f64) -> Self {
        Self {
            target_margin,
            target_slip_m_s,
            target: [target_margin, target_slip_m_s],
            weights: [1.0, 25.0],
        }
    }

    /// Secure hold: margin 0.40, slip arrested.
    pub fn secure() -> Self {
        Self::new(0.40, 0.0)
    }
}

impl StrangeAttractor for GraspHoldAttractor {
    fn name(&self) -> &'static str {
        "grasp_hold"
    }

    fn observe(&self, state: &[f64]) -> Vec<f64> {
        let margin = state.get(1).copied().unwrap_or(0.0);
        let slip = state.get(2).copied().unwrap_or(0.0);
        vec![margin, slip]
    }

    fn target(&self) -> &[f64] {
        &self.target
    }

    fn weights(&self) -> &[f64] {
        &self.weights
    }
}

/// One named scalar in a plant state vector.
///
/// Sketched constellation (indices documented on the matching plant):
/// - mycelial `delivered_nutrient`:= [`crate::physics::mycelial::MycelialMesh::delivery_ratio`]
/// (sink signal / source signal). Live orb field: `C_MycelialState.delivered_nutrient`.
/// - soil `glomalin_yield_stress`:= [`crate::physics::terran::SoilProfile::effective_yield_stress`]
/// [Pa] — `(base + glomalin_mg_g * coeff) * moisture_factor`.
#[derive(Clone, Debug)]
pub struct ScalarAttractor {
    name: &'static str,
    pub index: usize,
    pub target_value: f64,
    target: [f64; 1],
    weights: [f64; 1],
}

impl ScalarAttractor {
    pub fn new(name: &'static str, index: usize, target_value: f64, weight: f64) -> Self {
        Self {
            name,
            index,
            target_value,
            target: [target_value],
            weights: [weight],
        }
    }

    /// Maximize / gate delivered nutrient on [`super::MycelialLineDynamics`] (state[8]).
    pub fn mycelial_delivered_nutrient(target: f64) -> Self {
        Self::new("mycelial_delivered_nutrient", 8, target, 1.0)
    }

    /// Target effective yield stress [Pa] on [`super::GlomalinExudationDynamics`] (state[1]).
    pub fn soil_glomalin_yield_stress_pa(target_pa: f64) -> Self {
        Self::new(
            "soil_glomalin_yield_stress",
            1,
            target_pa,
            1.0 / (1.0e4 * 1.0e4),
        )
    }
}

impl StrangeAttractor for ScalarAttractor {
    fn name(&self) -> &'static str {
        self.name
    }

    fn observe(&self, state: &[f64]) -> Vec<f64> {
        vec![state.get(self.index).copied().unwrap_or(0.0)]
    }

    fn target(&self) -> &[f64] {
        &self.target
    }

    fn weights(&self) -> &[f64] {
        &self.weights
    }
}

/// Kinematic wrap: named object span, safe blocked-tendon strain, arrested slip.
///
/// Observables from [`super::dynamics::HandTendonDynamics`]:
/// span, blocked_strain (`blocked_stretch / L`), slip_velocity, overstretch flag,
/// friction-cone margin. Overstretch is the blocked-chain gate
/// (`TENDON_STRAIN_WARN = 0.055`), not total working strain.
#[derive(Clone, Debug)]
pub struct KinematicGraspAttractor {
    pub target_span_m: f64,
    pub target_strain: f64,
    pub target_slip_m_s: f64,
    pub target_margin: f64,
    target: [f64; 5],
    weights: [f64; 5],
}

impl KinematicGraspAttractor {
    pub fn new(target_span_m: f64, target_strain: f64, target_slip_m_s: f64) -> Self {
        Self::with_margin(target_span_m, target_strain, target_slip_m_s, 0.25)
    }

    pub fn with_margin(
        target_span_m: f64,
        target_strain: f64,
        target_slip_m_s: f64,
        target_margin: f64,
    ) -> Self {
        let strain = target_strain.clamp(0.0, crate::physics::dexterous::TENDON_STRAIN_WARN as f64);
        Self {
            target_span_m,
            target_strain: strain,
            target_slip_m_s,
            target_margin,
            target: [target_span_m, strain, target_slip_m_s, 0.0, target_margin],
            // span identity · blocked strain · slip · overstretch flag · margin
            weights: [4.0, 80.0, 30.0, 12.0, 3.0],
        }
    }

    /// Wrap `span_m` with blocked strain at 0 (no overstretch) and slip arrested.
    pub fn wrap(span_m: f64) -> Self {
        Self::new(span_m, 0.0, 0.0)
    }
}

impl StrangeAttractor for KinematicGraspAttractor {
    fn name(&self) -> &'static str {
        "kinematic_grasp"
    }

    fn observe(&self, state: &[f64]) -> Vec<f64> {
        use super::dynamics::{
            HAND_BLOCKED_STRAIN, HAND_MARGIN, HAND_OVERSTRETCH, HAND_SLIP_V, HAND_SPAN,
        };
        vec![
            state.get(HAND_SPAN).copied().unwrap_or(0.0),
            state.get(HAND_BLOCKED_STRAIN).copied().unwrap_or(0.0),
            state.get(HAND_SLIP_V).copied().unwrap_or(0.0),
            state.get(HAND_OVERSTRETCH).copied().unwrap_or(0.0),
            state.get(HAND_MARGIN).copied().unwrap_or(0.0),
        ]
    }

    fn target(&self) -> &[f64] {
        &self.target
    }

    fn weights(&self) -> &[f64] {
        &self.weights
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grasp_hold_cost_is_weighted_squares() {
        let a = GraspHoldAttractor::new(0.4, 0.0);
        // state: force, margin, slip,...
        let mut s = vec![0.0; 8];
        s[1] = 0.4;
        s[2] = 0.0;
        assert!(a.cost(&s) < 1e-18);
        s[1] = 0.2;
        let j = a.cost(&s);
        assert!((j - 0.04).abs() < 1e-12);
    }

    #[test]
    fn kinematic_grasp_penalizes_overstretch_and_slip() {
        use crate::syntropy::dynamics::{
            HAND_BLOCKED_STRAIN, HAND_MARGIN, HAND_OVERSTRETCH, HAND_SLIP_V, HAND_SPAN,
            HAND_STATE_DIM,
        };
        let a = KinematicGraspAttractor::wrap(0.028);
        let mut s = vec![0.0; HAND_STATE_DIM];
        s[HAND_SPAN] = 0.028;
        s[HAND_BLOCKED_STRAIN] = 0.0;
        s[HAND_SLIP_V] = 0.0;
        s[HAND_OVERSTRETCH] = 0.0;
        s[HAND_MARGIN] = 0.25;
        assert!(a.cost(&s) < 1e-18);
        s[HAND_OVERSTRETCH] = 1.0;
        s[HAND_SLIP_V] = 0.10;
        let j = a.cost(&s);
        assert!(j > 12.0, "flag + slip must cost, got {j}");
    }
}
