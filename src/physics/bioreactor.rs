//! G^G Bioreactor Impeller Tip Shear & kLa Mass Transfer Model
//!
//! Sub-system physics modeling 3D Non-Newtonian impeller shear stress,
//! volumetric gas-liquid mass transfer rates (kLa), and cell viability.

use serde::{Deserialize, Serialize};

/// State of an industrial bioreactor vessel simulation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BioreactorVesselState {
    pub vessel_volume_liters: f64,
    pub impeller_speed_rpm: f64,
    pub fluid_viscosity_pascal_sec: f64, // Non-Newtonian dynamic viscosity
    pub max_shear_stress_pa: f64,        // Peak shear stress at impeller tip
    pub kla_mass_transfer_hr: f64,       // Volumetric oxygen transfer coefficient kLa (1/hr)
    pub cellular_viability_pct: f64,     // Percentage of unruptured cells
    pub product_yield_g_l: f64,          // Biomanufacturing product concentration
    pub is_shear_damaged: bool,
}

/// Parameters for bioreactor fluid mechanics and bio-synthesis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BioreactorDesignParams {
    pub impeller_diameter_m: f64,
    pub critical_cell_shear_limit_pa: f64, // Max shear stress cells can withstand before lysis
    pub substrate_feed_rate_g_l_hr: f64,   // Nutrient feed rate
    pub vessel_aspect_ratio: f64,           // Height / Diameter
}

impl Default for BioreactorDesignParams {
    fn default() -> Self {
        Self {
            impeller_diameter_m: 0.85,
            critical_cell_shear_limit_pa: 45.0, // e.g. 45 Pa shear limit for mammalian/bacterial cells
            substrate_feed_rate_g_l_hr: 2.5,
            vessel_aspect_ratio: 3.0,
        }
    }
}

impl BioreactorVesselState {
    pub fn new(volume_l: f64, rpm: f64, viscosity: f64) -> Self {
        Self {
            vessel_volume_liters: volume_l,
            impeller_speed_rpm: rpm,
            fluid_viscosity_pascal_sec: viscosity,
            max_shear_stress_pa: 0.0,
            kla_mass_transfer_hr: 0.0,
            cellular_viability_pct: 100.0,
            product_yield_g_l: 0.0,
            is_shear_damaged: false,
        }
    }

    /// Advances Bioreactor Fluid Dynamics & Synthesis Physics by dt (hours)
    pub fn step(&mut self, params: &BioreactorDesignParams, dt_hr: f64) {
        let tip_speed_m_s =
            std::f64::consts::PI * params.impeller_diameter_m * (self.impeller_speed_rpm / 60.0);

        // Impeller tip shear: tau ≈ mu * (tip_speed / gap) — reduced-order tip gradient
        let velocity_gradient = tip_speed_m_s / (params.impeller_diameter_m * 0.1);
        self.max_shear_stress_pa = self.fluid_viscosity_pascal_sec * velocity_gradient;

        // van't Riet-style kLa (1/hr): A·(P/V)^a · N^b — interior industrial band, no artificial floor
        let power_draw_w = 1.5 * 1000.0 * (self.impeller_speed_rpm / 60.0).powi(3)
            * params.impeller_diameter_m.powi(5);
        let power_per_vol = (power_draw_w / (self.vessel_volume_liters / 1000.0)).max(1e-6);
        let kla_hr = 0.22
            * power_per_vol.powf(0.55)
            * (self.impeller_speed_rpm / 60.0).powf(0.40)
            * (params.vessel_aspect_ratio / 3.0).powf(0.15);
        // Soft physical bounds only (not a marketing floor at 50)
        self.kla_mass_transfer_hr = kla_hr.clamp(8.0, 650.0);

        // Cell Shear Lysis Check
        if self.max_shear_stress_pa > params.critical_cell_shear_limit_pa {
            let shear_overhang = self.max_shear_stress_pa - params.critical_cell_shear_limit_pa;
            let death_rate = 0.35 * shear_overhang * dt_hr;
            self.cellular_viability_pct = (self.cellular_viability_pct - death_rate).max(0.0);
            if self.cellular_viability_pct < 85.0 {
                self.is_shear_damaged = true;
            }
        }

        // Yield couples viability and oxygen transfer (kLa)
        let effective_biomass = (self.cellular_viability_pct / 100.0)
            * (self.kla_mass_transfer_hr / 180.0).clamp(0.08, 2.5);
        let synthesis_rate = params.substrate_feed_rate_g_l_hr * 0.45 * effective_biomass;
        self.product_yield_g_l += synthesis_rate * dt_hr;
    }
}
