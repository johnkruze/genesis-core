//! Reduced-order aerodynamics, rotor momentum theory, ice accretion, GPS plasma cutoff.
//! Not lattice-Boltzmann. `lbm_bridge` is the Metal D3Q19 demo; these bins do not belong on it.

use serde::{Deserialize, Serialize};

pub const RHO_SL: f64 = 1.225;
pub const G: f64 = 9.81;
pub const A_SL: f64 = 340.3; // m/s, ISA sea-level a
pub const GPS_L1_HZ: f64 = 1.57542e9;

/// q = ½ ρ V² [Pa]
#[inline]
pub fn dynamic_pressure_pa(rho_kg_m3: f64, tas_m_s: f64) -> f64 {
    0.5 * rho_kg_m3.max(0.0) * tas_m_s * tas_m_s
}

/// Troposphere ISA density, h in metres. Valid h ∈ [0, 11000].
#[inline]
pub fn isa_density_kg_m3(alt_m: f64) -> f64 {
    let h = alt_m.clamp(0.0, 11_000.0);
    RHO_SL * (1.0 - 2.25577e-5 * h).powf(4.256)
}

/// TAS from Mach at sea-level a (named reduced-order; not a full ISA table).
#[inline]
pub fn tas_from_mach(mach: f64) -> f64 {
    mach.max(0.0) * A_SL
}

/// Hover induced velocity. Momentum theory: v_i = √(T / (2 ρ A)).
#[inline]
pub fn hover_induced_velocity_ms(thrust_n: f64, rho_kg_m3: f64, rotor_area_m2: f64) -> f64 {
    (thrust_n.max(0.0) / (2.0 * rho_kg_m3.max(0.1) * rotor_area_m2.max(1e-3))).sqrt()
}

/// Descent ratio |v_z| / v_i. VRS bucket starts near 0.5.
#[inline]
pub fn vrs_descent_ratio(descent_ms: f64, induced_ms: f64) -> f64 {
    descent_ms.max(0.0) / induced_ms.max(0.1)
}

/// Empirical VRS thrust efficiency. Onset at ratio 0.5; deep ring ~0.1.
#[inline]
pub fn vrs_efficiency(descent_ratio: f64) -> f64 {
    if descent_ratio <= 0.5 {
        1.0
    } else {
        (1.0 - 1.2 * (descent_ratio - 0.5)).clamp(0.10, 1.0)
    }
}

#[inline]
pub fn in_vortex_ring(descent_ratio: f64) -> bool {
    descent_ratio > 0.5
}

/// Thin-airfoil lift: CL = CL0 + a α, capped at CL_max.
#[inline]
pub fn cl_linear(alpha_rad: f64, cl0: f64, lift_slope_per_rad: f64, cl_max: f64) -> f64 {
    (cl0 + lift_slope_per_rad * alpha_rad).clamp(-cl_max, cl_max)
}

/// Iced stall: CL_max and α_stall drop with ice factor ∈ [0, 1].
#[inline]
pub fn iced_cl_max(cl_max_clean: f64, ice_factor: f64) -> f64 {
    cl_max_clean * (1.0 - 0.45 * ice_factor.clamp(0.0, 1.0))
}

#[inline]
pub fn iced_stall_alpha_rad(alpha_stall_clean: f64, ice_factor: f64) -> f64 {
    alpha_stall_clean * (1.0 - 0.40 * ice_factor.clamp(0.0, 1.0))
}

/// Ice accretion ṁ [kg/s] = LWC [kg/m³] · V · A · E.
#[inline]
pub fn ice_accretion_kg_s(
    lwc_kg_m3: f64,
    tas_m_s: f64,
    collection_area_m2: f64,
    collection_efficiency: f64,
) -> f64 {
    lwc_kg_m3.max(0.0) * tas_m_s.max(0.0) * collection_area_m2.max(0.0) * collection_efficiency.clamp(0.0, 1.0)
}

/// Rotor T/W after ice: lift scales down with ice_mass / ice_ref.
#[inline]
pub fn iced_thrust_to_weight(thrust0_n: f64, weight_n: f64, ice_kg: f64, ice_ref_kg: f64) -> f64 {
    let loss = (ice_kg / ice_ref_kg.max(1e-6)).clamp(0.0, 0.70);
    (thrust0_n * (1.0 - loss)) / weight_n.max(1.0)
}

/// Electron plasma frequency [Hz]. f_p ≈ 8.98 √n_e  with n_e in m⁻³.
#[inline]
pub fn plasma_frequency_hz(electron_density_m3: f64) -> f64 {
    8.98 * electron_density_m3.max(0.0).sqrt()
}

#[inline]
pub fn gps_l1_blackout(electron_density_m3: f64) -> bool {
    plasma_frequency_hz(electron_density_m3) > GPS_L1_HZ
}

/// Named sheath density [m⁻³]. Not Saha.
/// Peak at 22 km (bow-shock layer). Amplitude ∝ ((M−2.8)/3)³. Mach ≲ 3.2 is quiet.
#[inline]
pub fn sheath_electron_density_m3(mach: f64, altitude_m: f64) -> f64 {
    let m_ex = (mach - 2.8).max(0.0) / 3.0;
    let n_ref = 2.5e18 * m_ex.powi(3);
    let layer = (-((altitude_m - 22_000.0) / 9_000.0).powi(2)).exp();
    n_ref * layer
}

/// SDOF wing with delay-line aileron. ÿ + 2ζω ẏ + ω² y = a_aero sin(ωt) + k_ail y_delayed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelayAeroservoelastic {
    pub y_m: f64,
    pub v_m_s: f64,
    pub omega_rad_s: f64,
    pub zeta: f64,
    delay: Vec<f64>,
    head: usize,
}

impl DelayAeroservoelastic {
    pub fn new(freq_hz: f64, zeta: f64, delay_s: f64, dt_s: f64) -> Self {
        let n = ((delay_s / dt_s).round() as usize).clamp(1, 400);
        Self {
            y_m: 0.0,
            v_m_s: 0.0,
            omega_rad_s: freq_hz.max(0.5) * 2.0 * std::f64::consts::PI,
            zeta: zeta.max(0.0),
            delay: vec![0.0; n],
            head: 0,
        }
    }

    pub fn step(&mut self, aero_accel: f64, aileron_gain: f64, dt_s: f64) -> f64 {
        let delayed = self.delay[self.head];
        let acc = -self.omega_rad_s.powi(2) * self.y_m
            - 2.0 * self.zeta * self.omega_rad_s * self.v_m_s
            + aero_accel
            + aileron_gain * delayed;
        self.v_m_s += acc * dt_s;
        self.y_m += self.v_m_s * dt_s;
        self.delay[self.head] = self.y_m;
        self.head = (self.head + 1) % self.delay.len();
        self.y_m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sea_level_q_mach_085() {
        let v = tas_from_mach(0.85);
        let q = dynamic_pressure_pa(RHO_SL, v);
        // ½ × 1.225 × (289.3)² ≈ 51.2 kPa
        assert!((q - 51_200.0).abs() < 2_000.0);
    }

    #[test]
    fn cargo_drone_vi() {
        let vi = hover_induced_velocity_ms(225.0 * G, RHO_SL, 4.0);
        assert!((vi - 15.1).abs() < 0.4);
        assert!(!in_vortex_ring(vrs_descent_ratio(0.3 * vi, vi)));
        assert!(in_vortex_ring(vrs_descent_ratio(1.0 * vi, vi)));
        assert!(vrs_efficiency(0.3) > 0.99);
        assert!(vrs_efficiency(1.0) < 0.5);
    }

    #[test]
    fn ice_and_stall_drop() {
        let cl = iced_cl_max(1.40, 0.8);
        assert!(cl < 1.10);
        let a = iced_stall_alpha_rad(0.26, 0.8);
        assert!(a < 0.20);
        let mdot = ice_accretion_kg_s(0.001, 40.0, 0.15, 0.5);
        assert!((mdot - 0.003).abs() < 1e-6);
    }

    #[test]
    fn plasma_l1_cutoff() {
        // n_e such that f_p = L1: n = (L1/8.98)² ≈ 3.08e16 m⁻³
        assert!(!gps_l1_blackout(1.0e16));
        assert!(gps_l1_blackout(1.0e18));
        let n = sheath_electron_density_m3(6.0, 22_000.0);
        assert!(gps_l1_blackout(n));
        let n_high = sheath_electron_density_m3(2.5, 22_000.0);
        assert!(!gps_l1_blackout(n_high));
    }

    #[test]
    fn delay_loop_moves() {
        let dt = 0.005;
        let mut w = DelayAeroservoelastic::new(12.0, 0.03, 0.020, dt);
        let mut peak: f64 = 0.0;
        for k in 0..600 {
            let t = k as f64 * dt;
            let y = w.step(18.0 * (w.omega_rad_s * t).sin(), -40.0, dt);
            peak = peak.max(y.abs());
        }
        assert!(peak > 0.002); // millimetres, not stuck at 0
    }
}
