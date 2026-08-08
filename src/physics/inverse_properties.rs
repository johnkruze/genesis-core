//! G^G Genesis Core: Multi-Objective Inverse Materials Property Physics Engine
//! Models backward property engineering from target loads (F_target), mass limits,
//! S-N fatigue endurance curves (N_cycles), and moment-of-inertia tensor invariants.

use serde::{Deserialize, Serialize};

/// Dynamic State of an Inverse Property Optimization Trajectory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InversePropertyState {
    pub target_load_kn: f64,
    pub target_fatigue_cycles: f64,
    pub achieved_mass_kg: f64,
    pub fatigue_endurance_limit_mpa: f64,
    pub von_mises_stress_mpa: f64,
    pub structural_mass_efficiency_index: f64,
    pub safety_margin: f64,
    pub is_multi_objective_satisfied: bool,
}

/// Parameters for Multi-Objective Property Invariants
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InversePropertyParams {
    pub max_allowable_mass_kg: f64,
    pub min_required_safety_margin: f64,
    pub base_density_kg_m3: f64,
}

impl Default for InversePropertyParams {
    fn default() -> Self {
        Self {
            max_allowable_mass_kg: 2.5,
            min_required_safety_margin: 1.0,
            base_density_kg_m3: 2700.0, // Aluminum-class lightweight baseline
        }
    }
}

impl InversePropertyState {
    pub fn new(target_f_kn: f64, fatigue_n: f64) -> Self {
        Self {
            target_load_kn: target_f_kn,
            target_fatigue_cycles: fatigue_n,
            achieved_mass_kg: 0.0,
            fatigue_endurance_limit_mpa: 0.0,
            von_mises_stress_mpa: 0.0,
            structural_mass_efficiency_index: 0.0,
            safety_margin: 0.0,
            is_multi_objective_satisfied: false,
        }
    }

    /// Advances Multi-Objective Inverse Physics Property Solver
    ///
    /// Alignment is principal-path efficiency:
    /// - higher alignment → lighter topology (mass factor down) and lower effective von Mises
    /// - lower alignment → heavier section and higher working stress
    /// Contract (unchanged): pass iff safety_margin ≥ min_required AND mass ≤ max_allowable.
    pub fn step(&mut self, params: &InversePropertyParams, alignment_score: f64) {
        // S-N Curve Fatigue Limit: sigma_e = sigma_ut * (N)^(-b) * C_surface
        let base_ut_mpa = 550.0; // 550 MPa UTS
        let exponent_b = 0.085;
        let fatigue_limit_mpa = base_ut_mpa * (self.target_fatigue_cycles).powf(-exponent_b) * 0.85;
        self.fatigue_endurance_limit_mpa = fatigue_limit_mpa;

        let align = alignment_score.clamp(0.0, 1.0);

        // Nominal section at 62 MPa working stress (fatigue-class aluminum envelope)
        let baseline_area_m2 = (self.target_load_kn * 1000.0) / (62.0 * 1e6);
        // Topology mass factor: misaligned load paths need more material (1.20 → 0.76)
        let mass_factor = 1.20 - 0.44 * align;
        let required_area_m2 = baseline_area_m2 * mass_factor;
        // Member length 0.48 m — mass dual-regime vs 2.5 kg budget
        self.achieved_mass_kg = required_area_m2 * 0.48 * params.base_density_kg_m3;

        let raw_stress_mpa = (self.target_load_kn * 1000.0 / required_area_m2.max(1e-12)) / 1e6;
        // Alignment reduces effective von Mises (principal-path relief): 1.22 → 0.78
        // Misalignment can drive safety_margin below the 1.0 contract
        let stress_alignment = 1.22 - 0.44 * align;
        self.von_mises_stress_mpa = raw_stress_mpa * stress_alignment;
        self.safety_margin = (fatigue_limit_mpa / self.von_mises_stress_mpa.max(1e-9)) - 1.0;

        // Structural Mass Efficiency Index: eta = (sigma_limit / density) / mass
        self.structural_mass_efficiency_index =
            (fatigue_limit_mpa / params.base_density_kg_m3) / self.achieved_mass_kg.max(1e-9);

        self.is_multi_objective_satisfied = self.safety_margin
            >= params.min_required_safety_margin
            && self.achieved_mass_kg <= params.max_allowable_mass_kg;
    }
}
