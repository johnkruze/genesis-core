//! Thermal & aerothermal reduced-order organ.
//!
//! Lumped RC node with the exact exponential step (unconditionally stable).
//! Battery Joule + linear-in-SoC ESR. Thin-lens optothermal defocus.
//! Arrhenius outgassing. Walther viscosity. Adiabatic wall temperature.
//! CMOS dark-current SNR. Stefan–Boltzmann exitance. AGC blackbody well fill.

use serde::{Deserialize, Serialize};

/// Stefan–Boltzmann constant [W / (m² K⁴)]
pub const STEFAN_BOLTZMANN_SIGMA: f64 = 5.670374419e-8;

/// Boltzmann constant [eV/K]
pub const BOLTZMANN_EV_K: f64 = 8.617333262e-5;

/// Visible wavelength used for geometric depth of focus [m]
pub const VISIBLE_WAVELENGTH_M: f64 = 550e-9;

/// Lumped-parameter thermal node. C [J/K], R [K/W].
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct LumpedThermalNode {
    pub temperature_c: f64,
    pub thermal_capacitance_j_k: f64,
    pub thermal_resistance_k_w: f64,
}

impl LumpedThermalNode {
    pub fn new(initial_temp_c: f64, capacitance_j_k: f64, resistance_k_w: f64) -> Self {
        Self {
            temperature_c: initial_temp_c,
            thermal_capacitance_j_k: capacitance_j_k.max(1e-3),
            thermal_resistance_k_w: resistance_k_w.max(1e-6),
        }
    }

    /// T(t+dt) = T_eq + (T − T_eq) exp(−dt/τ), τ = RC, T_eq = T_amb + Q̇ R.
    /// Unconditionally stable for all dt > 0.
    pub fn step(&mut self, heat_power_in_w: f64, ambient_temp_c: f64, dt_s: f64) -> f64 {
        let tau = self.thermal_capacitance_j_k * self.thermal_resistance_k_w;
        let t_eq = ambient_temp_c + (heat_power_in_w * self.thermal_resistance_k_w);
        let decay = (-dt_s / tau).exp();
        self.temperature_c = t_eq + (self.temperature_c - t_eq) * decay;
        self.temperature_c
    }
}

/// Joule heating P = I² R [W]
#[inline]
pub fn joule_heating_watts(current_amps: f64, resistance_ohms: f64) -> f64 {
    current_amps.powi(2) * resistance_ohms
}

/// Linear-in-SoC pack ESR. Reduced-order: not the U-shaped Li-ion curve.
/// R = R0 (1 + (100 − SoC)/100). Named, not a chemistry model.
#[inline]
pub fn battery_dynamic_resistance_ohms(base_resistance_ohms: f64, soc_pct: f64) -> f64 {
    let soc_clamped = soc_pct.clamp(1.0, 100.0);
    base_resistance_ohms * (1.0 + (100.0 - soc_clamped) / 100.0)
}

/// Terminal voltage under load. Returns (V_term, V_sag).
#[inline]
pub fn battery_voltage_sag(
    open_circuit_voltage_v: f64,
    current_draw_amps: f64,
    soc_pct: f64,
    base_resistance_ohms: f64,
) -> (f64, f64) {
    let r_int = battery_dynamic_resistance_ohms(base_resistance_ohms, soc_pct);
    let v_sag = current_draw_amps * r_int;
    let v_term = (open_circuit_voltage_v - v_sag).max(0.0);
    (v_term, v_sag)
}

/// Linear expansion strain ε = α ΔT.
#[inline]
pub fn thermal_expansion_strain(alpha_per_c: f64, delta_temp_c: f64) -> f64 {
    alpha_per_c * delta_temp_c
}

/// Thin-lens relative focal shift: Δf/f = [α − (dn/dT)/(n−1)] ΔT.
/// Simple lens in air, f = R/(n−1). Returns percent.
#[inline]
pub fn lens_optothermal_focal_shift_pct(
    alpha_cte: f64,
    dn_dt: f64,
    refractive_index: f64,
    delta_temp_c: f64,
) -> f64 {
    let n_minus_1 = (refractive_index - 1.0).max(1e-6);
    let coeff = alpha_cte - (dn_dt / n_minus_1);
    coeff * delta_temp_c * 100.0
}

/// Absolute defocus [m] for a thin lens of focal length `focal_length_m`.
#[inline]
pub fn lens_optothermal_defocus_m(
    focal_length_m: f64,
    alpha_cte: f64,
    dn_dt: f64,
    refractive_index: f64,
    delta_temp_c: f64,
) -> f64 {
    lens_optothermal_focal_shift_pct(alpha_cte, dn_dt, refractive_index, delta_temp_c) / 100.0
        * focal_length_m
}

/// Geometric depth of focus δ ≈ 2 λ N² [m].
#[inline]
pub fn geometric_depth_of_focus_m(wavelength_m: f64, f_number: f64) -> f64 {
    2.0 * wavelength_m.max(1e-9) * f_number.max(0.5).powi(2)
}

/// Arrhenius desorption rate ∝ A exp(−Ea / kT). T in °C, Ea in eV.
#[inline]
pub fn arrhenius_outgassing_rate(
    activation_energy_ev: f64,
    temperature_c: f64,
    pre_exponential_factor: f64,
) -> f64 {
    let temp_k = temperature_c + 273.15;
    if temp_k <= 0.0 {
        return 0.0;
    }
    let exponent = -activation_energy_ev / (BOLTZMANN_EV_K * temp_k);
    pre_exponential_factor * exponent.exp()
}

/// Elastomeric seal friction vs Tg. Sub-Tg stiffening, above-Tg bore expansion.
#[inline]
pub fn elastomeric_seal_friction_surge(
    temp_c: f64,
    glass_transition_c: f64,
    nominal_friction: f64,
) -> f64 {
    if temp_c > glass_transition_c {
        let expansion_factor = 1.0 + 0.015 * (temp_c - glass_transition_c);
        nominal_friction * expansion_factor
    } else {
        let stiffening_factor = 1.0 + 0.04 * (glass_transition_c - temp_c);
        nominal_friction * stiffening_factor
    }
}

/// Johnson–Nyquist voltage-SNR shift: 10 log10(T/Tref). Small for CMOS (dark current dominates).
#[inline]
pub fn sensor_thermal_snr_db(baseline_snr_db: f64, temp_k: f64, reference_temp_k: f64) -> f64 {
    let temp_ratio = (temp_k / reference_temp_k.max(1.0)).max(0.1);
    let noise_increase_db = 10.0 * temp_ratio.log10();
    (baseline_snr_db - noise_increase_db).max(0.0)
}

/// Silicon photodiode dark-current SNR. i_dark doubles every `doubling_temp_c` (~7 °C).
/// SNR ≈ SNR0 − 10 log10(2^((T−Tref)/Td)).
#[inline]
pub fn cmos_dark_current_snr_db(
    baseline_snr_db: f64,
    temp_c: f64,
    reference_temp_c: f64,
    doubling_temp_c: f64,
) -> f64 {
    let decades = (temp_c - reference_temp_c) / doubling_temp_c.max(1.0);
    let snr = baseline_snr_db - 10.0 * (2.0_f64.powf(decades)).log10();
    snr
}

/// Walther ASTM D341: log10(log10(ν + 0.7)) = A − B log10(T_K). Returns cSt.
#[inline]
pub fn walther_lubricant_viscosity_cst(a: f64, b: f64, temp_c: f64) -> f64 {
    let temp_k = (temp_c + 273.15).max(10.0);
    let log_t = temp_k.log10();
    let val = a - b * log_t;
    let ten_val = 10.0f64.powf(val);
    let nu = 10.0f64.powf(ten_val) - 0.7;
    nu.clamp(0.1, 100_000.0)
}

/// Recovery temperature T_aw = T_∞ (1 + r (γ−1)/2 M²). γ = 1.4.
#[inline]
pub fn adiabatic_wall_temperature_k(t_inf_k: f64, mach: f64, recovery_factor: f64) -> f64 {
    let gamma = 1.4f64;
    t_inf_k * (1.0 + recovery_factor * ((gamma - 1.0) / 2.0) * mach.powi(2))
}

/// Blackbody exitance E = ε σ T⁴ [W/m²].
#[inline]
pub fn stefan_boltzmann_emission_w_m2(emissivity: f64, temp_k: f64) -> f64 {
    emissivity * STEFAN_BOLTZMANN_SIGMA * temp_k.max(0.0).powi(4)
}

/// AGC holds background at `agc_well_fraction` of well. Hot source of fill-factor f
/// in the same pixel: well = (1−f) a + f a (T_s / T_bg)⁴.
/// Reduced-order MWIR: in-band radiance ∝ T⁴ in one spectral window.
#[inline]
pub fn agc_blackbody_well_fill(
    source_temp_k: f64,
    background_temp_k: f64,
    source_fill_factor: f64,
    agc_well_fraction: f64,
) -> f64 {
    let t_s = source_temp_k.max(1.0);
    let t_b = background_temp_k.max(1.0);
    let f = source_fill_factor.clamp(0.0, 1.0);
    let a = agc_well_fraction.clamp(0.0, 1.0);
    let contrast = (t_s / t_b).powi(4);
    (1.0 - f) * a + f * a * contrast
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lumped_step_exact_decay() {
        let mut node = LumpedThermalNode::new(25.0, 1000.0, 0.5);
        let t1 = node.step(100.0, 25.0, 10.0);
        assert!(t1 > 25.0 && t1 < 27.0);
        let t_final = node.step(100.0, 25.0, 10_000.0);
        assert!((t_final - 75.0).abs() < 1e-3);
    }

    #[test]
    fn battery_voltage_sag_partitions() {
        let (v_term, v_sag) = battery_voltage_sag(50.4, 80.0, 50.0, 0.06);
        assert!(v_sag > 0.0);
        assert!((v_term + v_sag - 50.4).abs() < 1e-12);
    }

    #[test]
    fn thin_lens_bk7_fifty_kelvin() {
        // BK7: α ≈ 7.1e-6 /K, dn/dT ≈ 3e-6 /K, n ≈ 1.5168
        // (n−1) form: coeff ≈ 1.3e-6 /K → 0.0065 % at 50 K
        let shift = lens_optothermal_focal_shift_pct(7.1e-6, 3.0e-6, 1.5168, 50.0);
        assert!(shift > 0.0);
        assert!(shift < 0.02);
        let df = lens_optothermal_defocus_m(0.10, 7.1e-6, 3.0e-6, 1.5168, 50.0);
        let dof = geometric_depth_of_focus_m(VISIBLE_WAVELENGTH_M, 1.4);
        assert!(df > dof); // 100 mm F/1.4, 50 K: defocus exceeds DoF
        assert!(dof > 1e-6 && dof < 5e-6);
    }

    #[test]
    fn adiabatic_wall_mach_12_and_24() {
        let t_aw_12 = adiabatic_wall_temperature_k(216.65, 1.2, 0.89);
        assert!((t_aw_12 - 272.18).abs() < 1.0);
        let t_aw_24 = adiabatic_wall_temperature_k(216.65, 2.4, 0.89);
        let t_c = t_aw_24 - 273.15;
        assert!((t_c - 165.6).abs() < 2.0);
    }

    #[test]
    fn walther_vg46_family() {
        let nu_40 = walther_lubricant_viscosity_cst(9.2, 3.6, 40.0);
        assert!((nu_40 - 46.0).abs() < 10.0);
        let nu_100 = walther_lubricant_viscosity_cst(9.2, 3.6, 100.0);
        assert!(nu_100 < nu_40);
        let nu_cold = walther_lubricant_viscosity_cst(9.2, 3.6, -10.0);
        assert!(nu_cold > 500.0);
    }

    #[test]
    fn arrhenius_half_ev_sixty_c() {
        let r20 = arrhenius_outgassing_rate(0.5, 20.0, 1.0);
        let r60 = arrhenius_outgassing_rate(0.5, 60.0, 1.0);
        let ratio = r60 / r20;
        assert!(ratio > 8.0 && ratio < 14.0);
    }

    #[test]
    fn dark_current_snr_doubling_seven() {
        let snr20 = cmos_dark_current_snr_db(38.0, 20.0, 20.0, 7.0);
        assert!((snr20 - 38.0).abs() < 1e-9);
        let snr70 = cmos_dark_current_snr_db(38.0, 70.0, 20.0, 7.0);
        assert!((snr70 - 16.5).abs() < 1.0);
        let nyquist = sensor_thermal_snr_db(38.0, 343.15, 293.15);
        assert!(nyquist > 36.0); // Johnson–Nyquist is ~0.7 dB; not the cliff
        assert!(snr70 < nyquist);
    }

    #[test]
    fn agc_well_fill_hot_spot() {
        let cool = agc_blackbody_well_fill(320.0, 300.0, 0.05, 0.40);
        assert!(cool > 0.35 && cool < 0.55);
        let flare = agc_blackbody_well_fill(800.0, 300.0, 0.15, 0.40);
        assert!(flare > 1.0);
    }
}
