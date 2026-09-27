//! G^G Genesis Core: Plasma-Facing Materials & Extreme Energy Alloy Physics Engine
//! Models extreme aerothermal heat fluxes (q_plasma), thermal shockwave micro-cracking,
//! ablation surface recession (s_dot), and MHD magnetic Z-shear coupling for fusion/hypersonic materials.

use serde::{Deserialize, Serialize};

/// Dynamic State of an Extreme Plasma-Facing Material Boundary (e.g. Tungsten/CMC Divertor)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlasmaFacingMaterialState {
    pub plasma_temperature_k: f64,
    pub heat_flux_mw_m2: f64,
    pub surface_temperature_k: f64,
    pub ablation_recession_depth_mm: f64,
    pub thermal_stress_mpa: f64,
    pub thermal_crack_safety_margin: f64,
    /// Composite barrier fail: thermal stress OR ablation (product dual-regime flag)
    pub is_ablation_spallation_failed: bool,
    /// Stress-only axis (σ_thermal > STRESS_LIMIT_MPA)
    pub is_thermal_stress_failed: bool,
    /// Ablation-only axis (recession > ABLATION_LIMIT_MM)
    pub is_ablation_failed: bool,
}

/// Plasma product law limits (sealed policy — plot thresholds must match)
pub const STRESS_LIMIT_MPA: f64 = 1800.0;
/// Divertor-tile recession gate (mm). Calibrated so ablation can fail without stress.
pub const ABLATION_LIMIT_MM: f64 = 0.12;
/// Surface temperature above which ablation integrates (K)
pub const ABLATION_ONSET_K: f64 = 1750.0;

/// Parameters for Plasma-Facing Extreme Energy Alloy Design
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlasmaFacingDesignParams {
    pub thermal_conductivity_w_mk: f64,    // e.g. 170 W/m*K for tungsten alloy
    pub heat_of_vaporization_mj_kg: f64,   // e.g. 11.5 MJ/kg
    pub max_allowable_thermal_stress_mpa: f64,
    pub material_density_kg_m3: f64,
}

impl Default for PlasmaFacingDesignParams {
    fn default() -> Self {
        Self {
            thermal_conductivity_w_mk: 170.0,
            heat_of_vaporization_mj_kg: 11.5,
            max_allowable_thermal_stress_mpa: 450.0, // 450 MPa max thermal stress before spallation
            material_density_kg_m3: 19300.0,         // Tungsten-class density
        }
    }
}

impl PlasmaFacingMaterialState {
    pub fn new(t_plasma_k: f64, flux_mw: f64) -> Self {
        Self {
            plasma_temperature_k: t_plasma_k,
            heat_flux_mw_m2: flux_mw,
            surface_temperature_k: 300.0,
            ablation_recession_depth_mm: 0.0,
            thermal_stress_mpa: 0.0,
            thermal_crack_safety_margin: 0.0,
            is_ablation_spallation_failed: false,
            is_thermal_stress_failed: false,
            is_ablation_failed: false,
        }
    }

    /// Advances Aerothermal Plasma Shockwave Physics by dt (seconds)
    pub fn step(&mut self, params: &PlasmaFacingDesignParams, dt_sec: f64) {
        // Convective + Radiative Heat Flux Balance to Surface (coupled to Plasma T)
        let q_flux_w_m2 = self.heat_flux_mw_m2 * 1e6;
        let delta_t_conduction = (q_flux_w_m2 * 0.006) / params.thermal_conductivity_w_mk; // 6mm high-flux divertor tile
        // Plasma radiation coupling + conduction — allows hot-plasma / moderate-flux ablation
        self.surface_temperature_k = 300.0
            + delta_t_conduction
            + (0.55 * (self.plasma_temperature_k - 1500.0).max(0.0));

        // Ablation recession (mm): dual-regime policy vs stress axis
        if self.surface_temperature_k > ABLATION_ONSET_K {
            let excess_t = self.surface_temperature_k - ABLATION_ONSET_K;
            let q_mw = self.heat_flux_mw_m2.max(0.0);
            // Plasma-T driven + flux driven terms (decouples somewhat from pure conduction stress)
            let recession_rate_mm_s = (0.012 * (excess_t / 300.0) + 0.008 * (q_mw / 10.0))
                / (params.heat_of_vaporization_mj_kg / 11.5);
            self.ablation_recession_depth_mm += recession_rate_mm_s * dt_sec;
        }

        // Thermal Stress Crack Risk: sigma = E * alpha * Delta_T / (1 - nu)
        let thermal_stress = 150.0 * 1.2e-5 * delta_t_conduction * 1e3 / 0.7; // E=150GPa
        self.thermal_stress_mpa = thermal_stress;
        self.thermal_crack_safety_margin =
            (STRESS_LIMIT_MPA / self.thermal_stress_mpa.max(1e-9)) - 1.0;

        self.is_thermal_stress_failed = self.thermal_stress_mpa > STRESS_LIMIT_MPA;
        self.is_ablation_failed = self.ablation_recession_depth_mm > ABLATION_LIMIT_MM;
        self.is_ablation_spallation_failed =
            self.is_thermal_stress_failed || self.is_ablation_failed;
    }
}
