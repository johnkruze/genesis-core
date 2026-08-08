//! G^G Genesis Core: Advanced Materials Tribology & Accelerated Aging Physics Engine
//! Models high-pressure surface contact friction, flash temperature spikes (T_flash),
//! galling wear rates, and thermal phase crystallization under extreme environmental stress.

use serde::{Deserialize, Serialize};

/// Dynamic State of a Material Surface undergoing Friction & Thermal Degradation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TribologySurfaceState {
    pub contact_pressure_mpa: f64,
    pub sliding_velocity_m_s: f64,
    pub ambient_temperature_k: f64,
    pub flash_temperature_k: f64,
    pub cumulative_galling_wear_um: f64,
    pub phase_crystallization_pct: f64,
    pub friction_coefficient_mu: f64,
    pub is_galling_seizure_failed: bool,
}

/// Parameters for Accelerated Material Aging Simulations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TribologyAgingParams {
    pub material_hardness_vickers: f64,
    pub critical_seizure_wear_um: f64,
    pub activation_energy_kj_mol: f64,
}

impl Default for TribologyAgingParams {
    fn default() -> Self {
        Self {
            material_hardness_vickers: 650.0, // e.g. hardened tool steel / ceramic alloy
            critical_seizure_wear_um: 50.0,    // 50 microns wear threshold for component seizure
            activation_energy_kj_mol: 45.0,
        }
    }
}

impl TribologySurfaceState {
    pub fn new(pressure_mpa: f64, velocity_m_s: f64, temp_k: f64) -> Self {
        Self {
            contact_pressure_mpa: pressure_mpa,
            sliding_velocity_m_s: velocity_m_s,
            ambient_temperature_k: temp_k,
            flash_temperature_k: temp_k,
            cumulative_galling_wear_um: 0.0,
            phase_crystallization_pct: 0.0,
            friction_coefficient_mu: 0.15,
            is_galling_seizure_failed: false,
        }
    }

    /// Advances Surface Friction & Accelerated Thermal Degradation Physics by dt (hours)
    pub fn step(&mut self, params: &TribologyAgingParams, dt_hr: f64) {
        // Flash temperature at micro-asperity contact points: T_flash = T_ambient + (mu * P_MPa * v * 100 / k)
        let delta_t_flash = (self.friction_coefficient_mu * self.contact_pressure_mpa * self.sliding_velocity_m_s * 120.0) / 250.0;
        self.flash_temperature_k = self.ambient_temperature_k + delta_t_flash;

        // Archard's Galling Wear Rate: dW/dt (microns/hour)
        let thermal_activation = (-params.activation_energy_kj_mol / (8.314e-3 * self.flash_temperature_k)).exp();
        let wear_rate_um_hr = (self.contact_pressure_mpa * 0.005) * self.sliding_velocity_m_s * (1.0 + thermal_activation * 5.0);
        self.cumulative_galling_wear_um += wear_rate_um_hr * dt_hr;

        // Phase Crystallization rate accelerates above 600K
        if self.flash_temperature_k > 550.0 {
            let temp_excess = self.flash_temperature_k - 550.0;
            self.phase_crystallization_pct = (self.phase_crystallization_pct + (temp_excess * 0.05 * dt_hr)).min(100.0);
            self.friction_coefficient_mu = 0.15 + (self.phase_crystallization_pct / 100.0) * 0.35;
        }

        if self.cumulative_galling_wear_um > 45.0 || self.friction_coefficient_mu > 0.48 {
            self.is_galling_seizure_failed = true;
        }
    }
}
