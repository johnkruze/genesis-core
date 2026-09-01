//! G^G Genesis Core: Fluid Power, Precision Hydraulics & Pneumatics Physics Engine
//!
//! Sub-system physics modeling fluid compressibility under extreme pressure loads,
//! entrained air bulk modulus degradation, acoustic pipe water hammer pressure waves,
//! line harmonic choke frequencies, and Arctic viscous stiction dynamics.

/// Nominal bulk modulus of hydraulic mineral oil (Pascals) ~ 1.7 GPa (250,000 PSI)
pub const NOMINAL_OIL_BULK_MODULUS_PA: f64 = 1.72e9;

/// Atmospheric air bulk modulus (isothermal) ~ 101.3 kPa (14.7 PSI)
pub const AIR_BULK_MODULUS_PA: f64 = 1.013e5;

/// Calculates effective fluid bulk modulus Beta_eff (Pascals) with entrained air bubbles
/// 1 / Beta_eff = 1 / Beta_oil + x_air / (P_gauge + P_atm)
#[inline]
pub fn entrained_air_effective_bulk_modulus_pa(
    oil_bulk_modulus_pa: f64,
    entrained_air_volume_fraction: f64,
    operating_pressure_pa: f64,
) -> f64 {
    let air_fraction = entrained_air_volume_fraction.clamp(0.0, 0.50);
    let p_abs = operating_pressure_pa.max(1e5);
    let air_compliance = air_fraction / p_abs;
    let oil_compliance = (1.0 - air_fraction) / oil_bulk_modulus_pa.max(1e6);
    1.0 / (oil_compliance + air_compliance).max(1e-12)
}

/// Calculates mechanical fluid column compression displacement (meters)
/// delta_L = L * (Delta_P / Beta_eff)
#[inline]
pub fn hydraulic_column_compression_m(
    column_length_m: f64,
    pressure_spike_pa: f64,
    effective_bulk_modulus_pa: f64,
) -> f64 {
    column_length_m * (pressure_spike_pa / effective_bulk_modulus_pa.max(1e6))
}

/// Calculates acoustic speed of sound inside a flexible or rigid hydraulic/pneumatic line
/// c = sqrt(Beta_eff / rho)
#[inline]
pub fn acoustic_fluid_wave_speed_m_s(effective_bulk_modulus_pa: f64, fluid_density_kg_m3: f64) -> f64 {
    (effective_bulk_modulus_pa / fluid_density_kg_m3.max(1.0)).sqrt()
}

/// Calculates the fundamental acoustic standing wave organ-pipe resonant frequency (Hz)
/// for a fluid line of length L open at one end (solenoid) and closed at the other (brake chamber):
/// f_0 = c / (4 * L)  [or f_0 = c / (2 * L) for both ends open]
#[inline]
pub fn pneumatic_line_resonance_freq_hz(
    sound_speed_m_s: f64,
    line_length_m: f64,
    is_open_closed: bool,
) -> f64 {
    let denominator = if is_open_closed {
        4.0 * line_length_m.max(0.1)
    } else {
        2.0 * line_length_m.max(0.1)
    };
    sound_speed_m_s / denominator
}

/// Calculates Arctic hydraulic viscous stiction breakaway force [Newtons]
/// F_stiction = F_coulomb + A_piston * (mu_viscosity * (v_shear / clearance_m))
#[inline]
pub fn hydraulic_actuator_viscous_drag_n(
    kinematic_viscosity_cst: f64,
    fluid_density_kg_m3: f64,
    piston_velocity_m_s: f64,
    piston_area_m2: f64,
    radial_clearance_m: f64,
) -> f64 {
    let dynamic_viscosity_pa_s = (kinematic_viscosity_cst * 1e-6) * fluid_density_kg_m3;
    let shear_rate = piston_velocity_m_s.abs() / radial_clearance_m.max(1e-6);
    let shear_stress_pa = dynamic_viscosity_pa_s * shear_rate;
    shear_stress_pa * piston_area_m2
}

/// Joukowsky water-hammer: ΔP = ρ c Δv.
#[inline]
pub fn joukowsky_delta_pa(fluid_density_kg_m3: f64, wave_speed_m_s: f64, delta_v_m_s: f64) -> f64 {
    fluid_density_kg_m3.max(1.0) * wave_speed_m_s.max(1.0) * delta_v_m_s.abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entrained_air_bulk_modulus() {
        // Pure oil: Beta = 1.72 GPa
        let beta_pure = entrained_air_effective_bulk_modulus_pa(NOMINAL_OIL_BULK_MODULUS_PA, 0.0, 10.0e6);
        assert!((beta_pure - NOMINAL_OIL_BULK_MODULUS_PA).abs() < 1e6);

        // 2% entrained air at 10 MPa (100 bar)
        let beta_air = entrained_air_effective_bulk_modulus_pa(NOMINAL_OIL_BULK_MODULUS_PA, 0.02, 10.0e6);
        // Bulk modulus should drop significantly due to air bubble compliance
        assert!(beta_air < beta_pure * 0.5);
    }

    #[test]
    fn test_pneumatic_line_resonance() {
        // Class 8 trailer line: L = 20m, sound speed in air = 343 m/s
        let f0 = pneumatic_line_resonance_freq_hz(343.0, 20.0, false);
        // f0 = 343 / (2 * 20) = 343 / 40 = 8.575 Hz
        assert!((f0 - 8.575).abs() < 0.01);
        let closed = pneumatic_line_resonance_freq_hz(343.0, 20.0, true);
        assert!((closed - 4.2875).abs() < 0.01);
    }

    #[test]
    fn column_and_joukowsky() {
        let b = NOMINAL_OIL_BULK_MODULUS_PA;
        let d = hydraulic_column_compression_m(0.5, 2.5e6, b);
        assert!(d > 0.0006 && d < 0.0009);
        let c = acoustic_fluid_wave_speed_m_s(b, 870.0);
        assert!(c > 1300.0 && c < 1500.0);
        let dp = joukowsky_delta_pa(1.2, 343.0, 8.0);
        assert!((dp - 3292.8).abs() < 1.0);
        let f = hydraulic_actuator_viscous_drag_n(14.0, 870.0, 0.10, 0.020, 5e-6);
        assert!(f > 4.0 && f < 6.0);
    }
}
