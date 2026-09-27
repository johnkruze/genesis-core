//! G^G Genesis Core: Advanced Materials Tribology & Accelerated Aging Physics Engine
//! Models high-pressure surface contact friction, flash temperature spikes (T_flash),
//! galling wear rates, Hamrock-Dowson EHL fluid film thickness, Basquin S-N fatigue,
//! and cable elastic deformation mechanics.

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
        // Flash temperature at micro-asperity contact points: T_flash = T_ambient + (mu * P_MPa * v * 120 / k)
        let delta_t_flash = (self.friction_coefficient_mu * self.contact_pressure_mpa * self.sliding_velocity_m_s * 120.0) / 250.0;
        self.flash_temperature_k = self.ambient_temperature_k + delta_t_flash;

        // Archard's Galling Wear Rate: dW/dt (microns/hour)
        let thermal_activation = (-params.activation_energy_kj_mol / (8.314e-3 * self.flash_temperature_k)).exp();
        let wear_rate_um_hr = (self.contact_pressure_mpa * 0.005) * self.sliding_velocity_m_s * (1.0 + thermal_activation * 5.0);
        self.cumulative_galling_wear_um += wear_rate_um_hr * dt_hr;

        // Phase Crystallization rate accelerates above 550K
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

/// Calculates S-N high-cycle fatigue cycles to failure via Basquin's equation
/// sigma_a = sigma_f' * (2 * N_f)^b  ==>  N_f = 0.5 * (sigma_a / sigma_f')^(1/b)
/// Note: basquin_exponent `b` is negative (typically -0.05 to -0.15 for structural steels and titanium alloys).
#[inline]
pub fn basquin_fatigue_life_cycles(
    stress_amplitude_mpa: f64,
    fatigue_strength_coeff_mpa: f64,
    basquin_exponent: f64,
) -> f64 {
    if stress_amplitude_mpa <= 0.0 {
        return f64::INFINITY;
    }
    let ratio = (stress_amplitude_mpa / fatigue_strength_coeff_mpa.max(1.0)).max(1e-6);
    let inv_b = 1.0 / basquin_exponent;
    0.5 * ratio.powf(inv_b)
}

/// Calculates minimum elastohydrodynamic lubrication (EHL) point-contact film thickness (Hamrock-Dowson)
///
/// H_min = 3.63 * U^0.68 * G^0.49 * W^-0.073 * (1 - exp(-0.68 * k_ellipticity))
///
/// - U = eta_0 * u / (E' * R_x)  [dimensionless speed parameter]
/// - G = alpha_piezo * E'       [dimensionless material parameter, ~3000-7000 for steel/mineral oil]
/// - W = F / (E' * R_x^2)       [dimensionless load parameter]
/// Returns minimum film thickness in microns (um).
#[inline]
pub fn ehl_minimum_film_thickness_um(
    viscosity_pa_s: f64,
    entrainment_velocity_m_s: f64,
    effective_radius_m: f64,
    normal_load_n: f64,
    effective_modulus_gpa: f64,
    pressure_viscosity_coeff_per_gpa: f64, // alpha_piezo ~ 15-25 GPa^-1 for synthetic/mineral oils
) -> f64 {
    let e_prime_pa = effective_modulus_gpa * 1e9;
    let r_x = effective_radius_m.max(1e-4);

    let u = (viscosity_pa_s.max(1e-4) * entrainment_velocity_m_s.abs().max(1e-3)) / (e_prime_pa * r_x);
    let g = (pressure_viscosity_coeff_per_gpa * 1e-9) * e_prime_pa;
    let w = normal_load_n.max(1.0) / (e_prime_pa * r_x.powi(2));

    // Point contact circular ellipticity (k=1): (1 - exp(-0.68)) ~ 0.4934
    let ellipticity_factor = 1.0 - (-0.68f64).exp();
    let h_dim = 3.63 * u.powf(0.68) * g.max(1.0).powf(0.49) * w.powf(-0.073) * ellipticity_factor;

    (h_dim * r_x * 1e6).max(0.001)
}

/// Calculates lubrication regime ratio Lambda = h_min / sigma_composite
/// Lambda < 1.0: Boundary lubrication (severe asperity contact, high galling/scuffing risk)
/// 1.0 <= Lambda <= 3.0: Mixed lubrication (partial asperity contact)
/// Lambda > 3.0: Full elastohydrodynamic / hydrodynamic fluid film
#[inline]
pub fn lambda_lubrication_ratio(film_thickness_um: f64, composite_roughness_um: f64) -> f64 {
    film_thickness_um / composite_roughness_um.max(1e-3)
}

/// Archard asperity wear drops once a film carries load. Named reduced-order scale.
/// Boundary Λ<1 → 1. Mixed → 0.25. EHL Λ>3 → 0.05.
#[inline]
pub fn lambda_wear_multiplier(lambda: f64) -> f64 {
    if lambda < 1.0 {
        1.0
    } else if lambda < 3.0 {
        0.25
    } else {
        0.05
    }
}

/// Calculates tendon/cable elastic elongation and tension stress
/// Returns (elongation_m, tensile_stress_mpa)
#[inline]
pub fn cable_elastic_mechanics(
    tension_n: f64,
    length_m: f64,
    diameter_mm: f64,
    youngs_modulus_gpa: f64,
) -> (f64, f64) {
    let radius_m = (diameter_mm * 1e-3) / 2.0;
    let area_m2 = std::f64::consts::PI * radius_m.powi(2);
    let stress_pa = tension_n / area_m2.max(1e-9);
    let strain = stress_pa / (youngs_modulus_gpa * 1e9);
    let elongation_m = strain * length_m;
    (elongation_m, stress_pa * 1e-6)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tribology_step() {
        let mut state = TribologySurfaceState::new(100.0, 1.0, 300.0);
        let params = TribologyAgingParams::default();
        state.step(&params, 1.0);
        assert!(state.cumulative_galling_wear_um > 0.0);
    }

    #[test]
    fn test_basquin_fatigue() {
        // Steel 4340: sigma_f' ~ 1200 MPa, b = -0.09
        let n_f = basquin_fatigue_life_cycles(600.0, 1200.0, -0.09);
        // (600/1200)^(-1/0.09) = (0.5)^(-11.11) = 2^11.11 ~ 2200 -> 0.5 * 2200 ~ 1100 cycles
        assert!(n_f > 500.0 && n_f < 50_000.0);
    }

    #[test]
    fn test_hamrock_dowson_point_contact() {
        // Mineral oil (eta = 0.05 Pa.s, alpha = 20 GPa^-1), steel ball on steel flat (E' = 220 GPa, R = 0.01 m, load = 100 N, u = 1.0 m/s)
        let h_min_um = ehl_minimum_film_thickness_um(0.05, 1.0, 0.01, 100.0, 220.0, 20.0);
        // Should yield realistic EHL film thickness between 0.1 and 2.0 microns
        assert!(h_min_um > 0.05 && h_min_um < 5.0);
        let lambda = lambda_lubrication_ratio(h_min_um, 0.05);
        assert!(lambda > 1.0); // Full/mixed EHL film
    }

    #[test]
    fn test_cable_mechanics() {
        let (elong, stress_mpa) = cable_elastic_mechanics(500.0, 1.0, 2.0, 200.0);
        assert!(elong > 0.0);
        assert!(stress_mpa > 100.0);
    }

    #[test]
    fn test_lambda_wear_scale() {
        assert_eq!(lambda_wear_multiplier(0.5), 1.0);
        assert_eq!(lambda_wear_multiplier(2.0), 0.25);
        assert_eq!(lambda_wear_multiplier(4.0), 0.05);
    }

    #[test]
    fn test_basquin_hundred_k_neighborhood() {
        // 4340-ish: 260 MPa lives ~10^7; 430 MPa dies before 1e5. Gate must sit in this cliff.
        let n_hi = basquin_fatigue_life_cycles(260.0, 1200.0, -0.09);
        let n_lo = basquin_fatigue_life_cycles(430.0, 1200.0, -0.09);
        assert!(n_hi > 1.0e6);
        assert!(n_lo < 1.0e5);
    }
}
