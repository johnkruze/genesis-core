//! G^G Genesis Core: Structural Dynamics, Harmonic Resonance & Shock Physics Engine
//!
//! Sub-system physics modeling single/multi-degree of freedom harmonic oscillators,
//! base-excited vibration transmissibility, acoustic pipe resonance, Friedlander blast waves,
//! and oleo-pneumatic viscous shock absorber thermodynamics.

use serde::{Deserialize, Serialize};

/// SDOF (Single Degree of Freedom) Mechanical Oscillator State
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DynamicOscillator {
    pub displacement_m: f64,
    pub velocity_m_s: f64,
    pub natural_frequency_rad_s: f64, // omega_n = sqrt(k / m)
    pub damping_ratio_zeta: f64,      // zeta = c / (2 * sqrt(k * m))
}

pub const GRAVITY: f64 = 9.81;
/// Dilatational bar speed in steel. Named reduced-order for hull shock delay.
pub const STEEL_BAR_WAVE_MS: f64 = 5960.0;

/// Inverse-square hull shock [g] from impact KE. Named coupling, not a hydrocode.
#[inline]
pub fn inverse_square_shock_g(kinetic_energy_j: f64, range_m: f64, coupling: f64) -> f64 {
    coupling * kinetic_energy_j.max(0.0) / (range_m.max(0.05).powi(2) * 9.81)
}

/// Point-mass linear inverted pendulum about the ankle/foot.
/// θ̈ = (g/h) sinθ − τ / (m h²). Restoring ankle torque must beat m g h.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct InvertedPendulum {
    pub theta_rad: f64,
    pub omega_rad_s: f64,
    pub com_height_m: f64,
    pub mass_kg: f64,
}

impl InvertedPendulum {
    pub fn new(theta_rad: f64, com_height_m: f64, mass_kg: f64) -> Self {
        Self {
            theta_rad,
            omega_rad_s: 0.0,
            com_height_m: com_height_m.max(0.05),
            mass_kg: mass_kg.max(1.0),
        }
    }

    pub fn gravity_torque_nm(&self) -> f64 {
        self.mass_kg * GRAVITY * self.com_height_m * self.theta_rad.sin()
    }

    pub fn inertia_kg_m2(&self) -> f64 {
        self.mass_kg * self.com_height_m.powi(2)
    }

    /// Critical PD stiffness: k_p > m g h or the upright is unstabilizable.
    pub fn mgh_nm_per_rad(&self) -> f64 {
        self.mass_kg * GRAVITY * self.com_height_m
    }

    pub fn step(&mut self, ankle_torque_nm: f64, dt_s: f64) -> f64 {
        let net = self.gravity_torque_nm() - ankle_torque_nm;
        let acc = net / self.inertia_kg_m2();
        self.omega_rad_s += acc * dt_s;
        self.theta_rad += self.omega_rad_s * dt_s;
        acc
    }
}

/// x_zmp = τ_ankle / F_normal. Outside the half-foot is a fall if it stays there.
#[inline]
pub fn zmp_from_ankle_torque_m(ankle_torque_nm: f64, normal_force_n: f64) -> f64 {
    ankle_torque_nm / normal_force_n.max(1.0)
}

/// PD ankle: τ = k_p θ + k_d ω, saturated. Sign restores toward upright.
#[inline]
pub fn pd_ankle_torque_nm(theta_rad: f64, omega_rad_s: f64, kp: f64, kd: f64, tau_max_nm: f64) -> f64 {
    (kp * theta_rad + kd * omega_rad_s).clamp(-tau_max_nm, tau_max_nm)
}

/// Gearbox deadband: torque does not transmit until |τ_cmd| exceeds τ_db.
#[inline]
pub fn backlash_deadband_torque_nm(tau_cmd_nm: f64, tau_deadband_nm: f64) -> f64 {
    let db = tau_deadband_nm.abs();
    if tau_cmd_nm.abs() <= db {
        0.0
    } else {
        tau_cmd_nm - db.copysign(tau_cmd_nm)
    }
}

impl DynamicOscillator {
    pub fn new(natural_freq_hz: f64, damping_ratio: f64) -> Self {
        let omega_n = natural_freq_hz * 2.0 * std::f64::consts::PI;
        Self {
            displacement_m: 0.0,
            velocity_m_s: 0.0,
            natural_frequency_rad_s: omega_n.max(1e-3),
            damping_ratio_zeta: damping_ratio.max(0.0),
        }
    }

    /// Advances the harmonic oscillator by dt under an external forcing acceleration [m/s^2]
    pub fn step(&mut self, forcing_accel_m_s2: f64, dt_s: f64) -> (f64, f64) {
        // x_ddot + 2 * zeta * omega_n * x_dot + omega_n^2 * x = forcing_accel
        let spring_accel = -self.natural_frequency_rad_s.powi(2) * self.displacement_m;
        let damping_accel = -2.0 * self.damping_ratio_zeta * self.natural_frequency_rad_s * self.velocity_m_s;
        let total_accel = forcing_accel_m_s2 + spring_accel + damping_accel;

        self.velocity_m_s += total_accel * dt_s;
        self.displacement_m += self.velocity_m_s * dt_s;

        (self.displacement_m, total_accel)
    }
}

/// Calculates steady-state base-excitation vibration transmissibility ratio TR = X / Y
/// TR = sqrt((1 + (2*zeta*r)^2) / ((1 - r^2)^2 + (2*zeta*r)^2)) where r = f_excitation / f_natural
#[inline]
pub fn vibration_transmissibility(excitation_freq_hz: f64, natural_freq_hz: f64, damping_ratio: f64) -> f64 {
    let r = excitation_freq_hz / natural_freq_hz.max(1e-4);
    let zeta = damping_ratio.max(1e-4);

    let num = 1.0 + (2.0 * zeta * r).powi(2);
    let den = (1.0 - r.powi(2)).powi(2) + (2.0 * zeta * r).powi(2);

    (num / den.max(1e-9)).sqrt()
}

/// Calculates Friedlander explosive blast wave overpressure P(t) [Pascals]
/// P(t) = P_so * (1 - t / t_d) * exp(-t / t_d) for t < t_d
#[inline]
pub fn friedlander_blast_overpressure_pa(
    peak_overpressure_pa: f64,
    positive_phase_duration_s: f64,
    time_since_arrival_s: f64,
) -> f64 {
    if time_since_arrival_s < 0.0 || time_since_arrival_s > positive_phase_duration_s {
        return 0.0;
    }
    let t_ratio = time_since_arrival_s / positive_phase_duration_s.max(1e-6);
    peak_overpressure_pa * (1.0 - t_ratio) * (-t_ratio).exp()
}

/// Calculates Hopkinson-Cranz scaled distance Z = R / W^(1/3) [m / kg^(1/3)]
/// and estimates peak incident shock overpressure P_so in kPa
#[inline]
pub fn hopkinson_cranz_peak_overpressure_kpa(standoff_distance_m: f64, tnt_equivalent_kg: f64) -> f64 {
    let z = standoff_distance_m / (tnt_equivalent_kg.max(1e-3)).cbrt();
    let z_clamped = z.max(0.1);
    // Standard Kingery-Bulmash polynomial approximation family: P_so ~ 200/Z + 150/Z^2 + 30/Z^3
    (200.0 / z_clamped) + (150.0 / z_clamped.powi(2)) + (30.0 / z_clamped.powi(3))
}

/// Oleo-Pneumatic Shock Absorber Thermodynamics
/// Models pneumatic gas spring F_gas = P0 * A * (V0 / (V0 - A*x))^gamma
/// and hydraulic orifice damping F_hyd = 0.5 * rho * A^3 * x_dot^2 / (C_d * A_orifice)^2
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OleoStrutDamper {
    pub piston_area_m2: f64,
    pub initial_gas_pressure_pa: f64,
    pub initial_gas_volume_m3: f64,
    pub fluid_density_kg_m3: f64,
    pub discharge_coeff: f64,
    pub orifice_area_m2: f64,
    pub gas_gamma: f64,
}

impl Default for OleoStrutDamper {
    fn default() -> Self {
        Self {
            piston_area_m2: 0.015,                // ~138 mm diameter piston
            initial_gas_pressure_pa: 4.5e6,       // 4.5 MPa (650 PSI precharge)
            initial_gas_volume_m3: 0.009,         // 9 liters
            fluid_density_kg_m3: 880.0,           // MIL-PRF-5606 hydraulic fluid
            discharge_coeff: 0.65,
            orifice_area_m2: 0.00018,             // 180 mm^2 metering orifice
            gas_gamma: 1.35,                      // Polytropic nitrogen compression
        }
    }
}

/// First-order Dryden-like lateral gust. τ v̇ + v = σ w.
#[inline]
pub fn dryden_gust_step(v_ms: f64, white: f64, tau_s: f64, sigma_ms: f64, dt_s: f64) -> f64 {
    v_ms + (-v_ms / tau_s.max(1e-3) + sigma_ms * white) * dt_s
}

impl OleoStrutDamper {
    pub fn forces_n(&self, stroke_displacement_m: f64, stroke_velocity_m_s: f64) -> (f64, f64) {
        let swept_vol = self.piston_area_m2 * stroke_displacement_m.max(0.0);
        let remaining_vol = (self.initial_gas_volume_m3 - swept_vol).max(1e-4);
        let vol_ratio = self.initial_gas_volume_m3 / remaining_vol;
        let gas_pressure = self.initial_gas_pressure_pa * vol_ratio.powf(self.gas_gamma);
        let gas_force = gas_pressure * self.piston_area_m2;

        let v_sign = stroke_velocity_m_s.signum();
        let effective_orifice = self.discharge_coeff * self.orifice_area_m2;
        let hyd_force = 0.5 * self.fluid_density_kg_m3 * (self.piston_area_m2.powi(3) / effective_orifice.powi(2)) * stroke_velocity_m_s.powi(2) * v_sign;

        (gas_force, hyd_force)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oscillator_resonance() {
        let mut osc = DynamicOscillator::new(10.0, 0.05); // 10 Hz, 5% damping
        // Exciting at resonance: 10 Hz
        let dt = 0.001;
        let mut max_disp = 0.0;
        for tick in 0..1000 {
            let t = tick as f64 * dt;
            let force = (t * 10.0 * 2.0 * std::f64::consts::PI).sin() * 10.0;
            let (disp, _) = osc.step(force, dt);
            if disp.abs() > max_disp {
                max_disp = disp.abs();
            }
        }
        // Static deflection is F0 / omega_n^2 = 10 / (20*pi)^2 = 0.00253 m
        // At resonance with Q = 1/(2*zeta) = 10, dynamic amplitude reaches ~ 0.0253 m
        assert!(max_disp > 0.02); // >8x resonance amplification over static deflection
    }

    #[test]
    fn test_transmissibility_at_resonance() {
        let tr = vibration_transmissibility(10.0, 10.0, 0.05);
        // At r=1, TR = sqrt((1 + 4*zeta^2) / (4*zeta^2)) = sqrt(1 + 1/(4*zeta^2)) ~ 1 / (2*zeta) = 10.0
        assert!(tr > 9.5 && tr < 10.5);
    }

    #[test]
    fn test_friedlander_blast() {
        let p_peak = 100_000.0; // 100 kPa
        let p0 = friedlander_blast_overpressure_pa(p_peak, 0.01, 0.0);
        assert_eq!(p0, p_peak);
        let p_mid = friedlander_blast_overpressure_pa(p_peak, 0.01, 0.005);
        assert!(p_mid > 0.0 && p_mid < p_peak);
        let p_end = friedlander_blast_overpressure_pa(p_peak, 0.01, 0.01);
        assert_eq!(p_end, 0.0);
    }

    #[test]
    fn test_oleo_strut_forces() {
        let strut = OleoStrutDamper::default();
        let (f_gas, f_hyd) = strut.forces_n(0.1, 2.0);
        assert!(f_gas > 50_000.0); // Gas spring supports vehicle weight
        assert!(f_hyd > 10_000.0); // Dynamic hydraulic damping absorbs sink kinetic energy
    }

    #[test]
    fn inverted_pendulum_needs_kp_above_mgh() {
        let mut plant = InvertedPendulum::new(0.08, 0.85, 70.0);
        let mgh = plant.mgh_nm_per_rad();
        assert!((mgh - 584.0).abs() < 5.0);
        let kp = 950.0;
        let kd = 80.0;
        assert!(kp > mgh);
        for _ in 0..800 {
            let tau = pd_ankle_torque_nm(plant.theta_rad, plant.omega_rad_s, kp, kd, 120.0);
            plant.step(tau, 0.001);
        }
        assert!(plant.theta_rad.abs() < 0.03);

        let mut fall = InvertedPendulum::new(0.08, 0.85, 70.0);
        let kp_low = 200.0;
        assert!(kp_low < fall.mgh_nm_per_rad());
        for _ in 0..400 {
            let tau = pd_ankle_torque_nm(fall.theta_rad, fall.omega_rad_s, kp_low, 20.0, 120.0);
            fall.step(tau, 0.001);
        }
        assert!(fall.theta_rad.abs() > 0.08);
    }

    #[test]
    fn oleo_static_preload_beats_light_fighter() {
        let strut = OleoStrutDamper::default();
        let (f_gas, _) = strut.forces_n(0.0, 0.0);
        // 4.5 MPa × 0.015 m² = 67.5 kN > 5000 kg share
        assert!(f_gas > 60_000.0);
        assert!(f_gas < 80_000.0);
    }

    #[test]
    fn zmp_and_deadband() {
        let z = zmp_from_ankle_torque_m(45.0, 700.0);
        assert!((z - 0.0643).abs() < 1e-3);
        assert_eq!(backlash_deadband_torque_nm(2.0, 5.0), 0.0);
        assert!((backlash_deadband_torque_nm(12.0, 5.0) - 7.0).abs() < 1e-12);
    }

    #[test]
    fn steel_shock_inverse_square() {
        let t = 1.0 / STEEL_BAR_WAVE_MS;
        assert!((t - 1.678e-4).abs() < 1e-6);
        let g1 = inverse_square_shock_g(1.0e6, 1.0, 1.0e-3);
        let g2 = inverse_square_shock_g(1.0e6, 2.0, 1.0e-3);
        assert!((g1 / g2 - 4.0).abs() < 1e-9);
    }
}
