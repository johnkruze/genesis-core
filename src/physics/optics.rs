//! Reduced-order optics, radar path loss, and wave-sensor fusion.
//! Snell, Beer-Lambert, Atlas rain extinction, FMCW Doppler rain, X-band mud (R ∝ P^{1/4}).
//! Fictional TSV gears `radar` / `sensors` are not modules. Those bins live here.

/// Air / liquid water refractive index (visible, named).
pub const N_AIR: f64 = 1.0003;
pub const N_WATER: f64 = 1.333;
/// Wet-clay X-band coating. Named reduced-order: pure water at 10 GHz is ~5–8 dB/cm; mud is less.
pub const XBAND_WET_MUD_DB_PER_CM: f64 = 2.0;

/// 2D Snell's law. None on total internal reflection.
#[inline]
pub fn snells_law_refraction_rad(
    incident_angle_rad: f64,
    n1_medium: f64,
    n2_medium: f64,
) -> Option<f64> {
    let sin_theta2 = (n1_medium / n2_medium.max(1e-4)) * incident_angle_rad.sin();
    if sin_theta2.abs() > 1.0 {
        None
    } else {
        Some(sin_theta2.asin())
    }
}

/// Point-cloud depth error from a droplet lens on a dome. n1=air, n2=water.
#[inline]
pub fn droplet_refraction_projection_error_m(
    target_distance_m: f64,
    incident_angle_rad: f64,
    n_droplet: f64,
) -> f64 {
    if let Some(refracted) = snells_law_refraction_rad(incident_angle_rad, N_AIR, n_droplet) {
        target_distance_m * (incident_angle_rad - refracted).abs().tan()
    } else {
        0.0
    }
}

/// Beer-Lambert T = exp(-κ L).
#[inline]
pub fn beer_lambert_transmittance(extinction_coeff_per_m: f64, path_length_m: f64) -> f64 {
    (-extinction_coeff_per_m.max(0.0) * path_length_m.max(0.0))
        .exp()
        .clamp(0.0, 1.0)
}

/// Atlas (1973) optical rain extinction [1/m]: β ≈ 0.24 R^{0.63} km⁻¹.
#[inline]
pub fn rain_extinction_per_m(rainfall_mm_hr: f64) -> f64 {
    2.4e-4 * rainfall_mm_hr.max(0.0).powf(0.63)
}

/// Two-way LiDAR range. Named reduced-order: R = R0 · exp(-2 α R0).
#[inline]
pub fn lidar_two_way_range_m(nominal_range_m: f64, extinction_per_m: f64) -> f64 {
    let r0 = nominal_range_m.max(1.0);
    r0 * beer_lambert_transmittance(2.0 * extinction_per_m, r0)
}

/// Highway stop: v t_r + v²/(2a). 75 mph, 0.4 g, 1.5 s ≈ 194 m.
#[inline]
pub fn stopping_distance_m(speed_ms: f64, accel_ms2: f64, reaction_s: f64) -> f64 {
    let v = speed_ms.max(0.0);
    v * reaction_s.max(0.0) + v * v / (2.0 * accel_ms2.max(0.1))
}

/// FMCW Doppler velocity noise [m/s] from Marshall-Palmer rain + truck motion.
#[inline]
pub fn fmcw_rain_doppler_noise_power(rainfall_rate_mm_hr: f64, truck_velocity_m_s: f64) -> f64 {
    let relative_velocity = (truck_velocity_m_s.powi(2) + 9.0f64.powi(2)).sqrt();
    let rain_cross_section = (rainfall_rate_mm_hr / 25.0).powf(0.85);
    rain_cross_section * (relative_velocity / 10.0)
}

/// Two-way radar range under a dielectric coating. R ~ P_r^{1/4}.
#[inline]
pub fn radar_attenuated_range_m(
    nominal_range_m: f64,
    mud_thickness_cm: f64,
    attenuation_db_per_cm: f64,
) -> f64 {
    let two_way_loss_db = 2.0 * mud_thickness_cm.max(0.0) * attenuation_db_per_cm.max(0.0);
    let power_fraction = 10.0f64.powf(-two_way_loss_db / 10.0);
    nominal_range_m * power_fraction.powf(0.25)
}

/// Salt-film transmittance. κ = 0.09 m²/g (named Mie crystals). Σ is areal g/m².
#[inline]
pub fn salt_film_transmittance(mass_mg: f64, window_area_m2: f64) -> f64 {
    let areal_g_m2 = (mass_mg.max(0.0) / 1000.0) / window_area_m2.max(1e-4);
    (-0.09 * areal_g_m2).exp().clamp(0.0, 1.0)
}

/// Wood-smoke extinction [1/m]. Named 0.12 m²/g.
#[inline]
pub fn smoke_extinction_per_m(density_g_m3: f64) -> f64 {
    0.12 * density_g_m3.max(0.0)
}

/// Complementary mag/GPS heading. mag_trust=1 is compass-only.
#[inline]
pub fn mag_gps_fused_heading_rad(mag_rad: f64, gps_rad: f64, mag_trust: f64) -> f64 {
    let w = mag_trust.clamp(0.0, 1.0);
    w * mag_rad + (1.0 - w) * gps_rad
}

/// Pinhole angular size in pixels: f · D / R.
#[inline]
pub fn pinhole_apparent_px(size_m: f64, range_m: f64, focal_px: f64) -> f64 {
    focal_px * size_m.max(0.0) / range_m.max(1e-3)
}

/// Rigid-deck image source: p / p_ff = 1 + h_ref / h.
#[inline]
pub fn image_source_pressure_ratio(alt_m: f64, ref_alt_m: f64) -> f64 {
    1.0 + ref_alt_m.max(0.0) / alt_m.max(0.15)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snells_law() {
        let theta1 = 30.0f64.to_radians();
        let theta2 = snells_law_refraction_rad(theta1, 1.0, 1.33).unwrap();
        assert!((theta2.to_degrees() - 22.08).abs() < 0.1);
        // Water → air at 50° is TIR (critical ≈ 48.8°). 40° is not.
        assert!(snells_law_refraction_rad(50.0f64.to_radians(), N_WATER, N_AIR).is_none());
        assert!(snells_law_refraction_rad(40.0f64.to_radians(), N_WATER, N_AIR).is_some());
    }

    #[test]
    fn snell_not_theta_times_delta_n() {
        // Gemini: θ·(1.33−1) at 15°, 5 m → ~0.43 m. Product is Snell (~0.33 m).
        let e = droplet_refraction_projection_error_m(5.0, 15.0f64.to_radians(), N_WATER);
        let costume = 5.0 * (15.0f64.to_radians() * 0.33).tan();
        assert!(e > 0.30 && e < 0.36);
        assert!(costume > 0.40);
        let e45 = droplet_refraction_projection_error_m(5.0, 45.0f64.to_radians(), N_WATER);
        assert!(e45 > 1.05 && e45 < 1.25);
    }

    #[test]
    fn test_beer_lambert() {
        let t1 = beer_lambert_transmittance(0.1, 10.0);
        assert!((t1 - 0.3678).abs() < 1e-3);
    }

    #[test]
    fn atlas_rain_and_lidar_range() {
        let a25 = rain_extinction_per_m(25.0);
        assert!(a25 > 0.0012 && a25 < 0.0025);
        let dry = lidar_two_way_range_m(250.0, 0.0);
        let wet = lidar_two_way_range_m(250.0, rain_extinction_per_m(50.0));
        assert!((dry - 250.0).abs() < 1e-9);
        assert!(wet < 80.0);
        assert!(fmcw_rain_doppler_noise_power(50.0, 25.0) > fmcw_rain_doppler_noise_power(5.0, 25.0));
    }

    #[test]
    fn highway_stop_seventy_five_mph() {
        let v = 75.0 * 1609.34 / 3600.0;
        let d = stopping_distance_m(v, 0.4 * 9.81, 1.5);
        assert!((d - 193.7).abs() < 2.0);
    }

    #[test]
    fn test_radar_attenuation() {
        let r = radar_attenuated_range_m(400.0, 5.0, XBAND_WET_MUD_DB_PER_CM);
        assert!((r - 126.49).abs() < 1.0);
    }

    #[test]
    fn salt_and_smoke_beer() {
        let t = salt_film_transmittance(400.0, 0.04);
        assert!(t > 0.30 && t < 0.50);
        let t_clear = salt_film_transmittance(20.0, 0.04);
        assert!(t_clear > 0.90);
        let k = smoke_extinction_per_m(8.0);
        assert!((k - 0.96).abs() < 1e-9);
    }

    #[test]
    fn mag_fuse_and_pinhole_and_image() {
        let crab = mag_gps_fused_heading_rad(0.05, 0.0, 0.9);
        assert!((crab - 0.045).abs() < 1e-12);
        let gps = mag_gps_fused_heading_rad(0.05, 0.0, 0.1);
        assert!(gps.abs() < 0.01);
        let px_near = pinhole_apparent_px(0.4, 10.0, 1800.0);
        let px_far = pinhole_apparent_px(0.4, 40.0, 1800.0);
        assert!((px_near - 72.0).abs() < 1e-9);
        assert!((px_far - 18.0).abs() < 1e-9);
        let p = image_source_pressure_ratio(2.0, 2.0);
        assert!((p - 2.0).abs() < 1e-12);
    }
}
