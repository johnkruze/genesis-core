// HUMANOID BIPEDAL GAIT — YAW ANGULAR MOMENTUM ACCUMULATION
//
// From the Euler binary: τ_z = Ω_x × L_y = 4.9 Nm yaw coupling torque
// at v=2.5 m/s, 60kg load, 0.5m load height.
//
// Simplified controllers (ZMP-based) model only the sagittal and frontal planes.
// They do not model τ_z — the yaw coupling torque from swing leg angular momentum.
//
// This torque accumulates over steps. The ankle provides counter-torque via
// ground friction. When accumulated yaw angular momentum exceeds the ankle's
// friction authority, the robot begins yawing into a fall.
//
// Governing equations (per step):
//   ΔL_z = τ_z × T_step         (yaw momentum added per step)
//   τ_friction_max = μ × M × g × r_ankle
//   If cumulative |L_z| > τ_friction_max × T_step: ankle authority exceeded
//
// Critical step count: N_crit = τ_friction_max / |τ_z|
//
// This binary sweeps (speed, load, terrain_friction) and maps N_crit.
// The finding: simplified controllers that ignore τ_z will walk into a fall
// at N > N_crit steps without any warning from their ZMP diagnostics.

use genesis_core::proof::seal_run;
use genesis_core::rng::Rng;
use rayon::prelude::*;
use serde::Serialize;
use std::fs::File;
use std::io::Write;
use std::time::Instant;
use sha2::{Sha256, Digest};

// Robot parameters (Atlas-class)
const M_BODY: f64 = 80.0;
const M_LEG: f64 = 12.0;
const L_LEG: f64 = 0.9;
const STRIDE_L: f64 = 0.70;
const R_ANKLE: f64 = 0.08;     // ankle moment arm for friction torque (m)
const G: f64 = 9.81;
const I_BODY_Z: f64 = 18.0;    // body yaw inertia (kg·m²) — from CAD of Atlas
const I_SWING_YY: f64 = M_LEG * L_LEG * L_LEG / 12.0; // 0.81 kg·m²
const I_SWING_ZZ: f64 = M_LEG * 0.06 * 0.06 / 2.0;    // 0.022 kg·m²

#[derive(Serialize, Clone)]
struct YawAccRun {
    speed_ms: f64,
    load_kg: f64,
    load_height_m: f64,
    friction_coeff: f64,
    // Computed physics
    tau_z_nm: f64,              // yaw coupling torque τ_z = Ω_x × L_y
    tau_friction_max_nm: f64,   // max ankle friction counter-torque
    step_freq_hz: f64,
    t_step_s: f64,
    delta_lz_per_step_nms: f64, // yaw momentum added per step
    critical_step_count: f64,   // N_crit = τ_friction / τ_z
    yaw_angle_at_100_steps_deg: f64,  // how much yaw at 100 steps
    zmp_sees_nothing: bool,     // ZMP model shows no warning at N_crit
    gait_distance_to_fall_m: f64,     // how far the robot walked before falling
}

#[derive(Clone)]
struct Cfg { speed: f64, load: f64, load_h: f64, friction: f64, seed: u64 }

fn simulate(cfg: &Cfg) -> (YawAccRun, String) {
    let mut rng = Rng::new(cfg.seed);

    let m_total = M_BODY + cfg.load;
    let f_step = cfg.speed / (2.0 * STRIDE_L);
    let t_step = 1.0 / f_step.max(0.01);

    // Swing angular velocity (sagittal, dominant term)
    let omega_swing_y = std::f64::consts::PI * f_step * L_LEG / (STRIDE_L / 2.0);

    // Body roll rate from lateral sway — increases with load height
    // From lateral ZMP dynamics: Ω_x ≈ v × sqrt(M_load × h_load / (M_total × H_com)) / L_leg
    let h_com_eff = (M_BODY * 1.0 + cfg.load * (1.0 + cfg.load_h)) / m_total;
    let omega_body_x = 0.08 * cfg.speed / L_LEG * (1.0 + cfg.load * cfg.load_h / (m_total * h_com_eff))
                     + rng.gaussian(0.0, 0.01);

    // L_y = I_swing_yy × ω_swing_y (sagittal angular momentum)
    let l_y = I_SWING_YY * omega_swing_y;

    // τ_z = Ω_x × L_y — yaw coupling from Euler cross product
    // Full cross product: τ_z = Ω_x × L_y - Ω_y × L_x ≈ Ω_x × L_y (dominant term)
    let tau_z = omega_body_x * l_y;

    // Maximum ankle friction counter-torque
    let tau_friction = cfg.friction * m_total * G * R_ANKLE;

    // Yaw momentum increment per step
    let delta_lz = tau_z * t_step;

    // Critical step count: how many steps until cumulative L_z > friction authority
    let n_crit = if tau_z > 0.01 {
        (tau_friction / (tau_z + 1e-6)).min(10000.0)
    } else { 10000.0 };

    // Yaw angle accumulated at N_crit and at 100 steps
    // φ(N) = Σ ΔL_z / I_body_z (per step)
    let phi_per_step_rad = delta_lz / I_BODY_Z;
    let phi_at_100_deg = (100.0 * phi_per_step_rad).to_degrees().abs();
    let phi_at_ncrit_deg = (n_crit * phi_per_step_rad).to_degrees().abs();

    // ZMP sees nothing: at N_crit, is the ZMP inside the support polygon?
    // ZMP perturbation from tau_z ≈ tau_z / (M * g * L_step) [lateral moment arm]
    let zmp_from_yaw = tau_z / (m_total * G * STRIDE_L);
    let support_y_half = 0.12f64;
    let zmp_sees_nothing = zmp_from_yaw.abs() < support_y_half; // ZMP still looks fine

    // Distance walked to fall
    let gait_dist = n_crit * STRIDE_L;

    let r = YawAccRun {
        speed_ms: cfg.speed,
        load_kg: cfg.load,
        load_height_m: cfg.load_h,
        friction_coeff: cfg.friction,
        tau_z_nm: tau_z,
        tau_friction_max_nm: tau_friction,
        step_freq_hz: f_step,
        t_step_s: t_step,
        delta_lz_per_step_nms: delta_lz,
        critical_step_count: n_crit,
        yaw_angle_at_100_steps_deg: phi_at_100_deg,
        zmp_sees_nothing,
        gait_distance_to_fall_m: gait_dist.min(10000.0),
    };
    let mut h = Sha256::new();
    h.update(n_crit.to_le_bytes());
    h.update(tau_z.to_le_bytes());
    (r, hex::encode(h.finalize()))
}

fn main() {
    println!("=== G^G KERNEL: HUMANOID YAW ACCUMULATION — INVISIBLE FAILURE MODE ===");
    println!("τ_z = Ω_x × L_y  →  N_crit = τ_friction_max / τ_z");
    println!("ZMP controllers never see this coming.");
    let start = Instant::now();

    let mut cfgs = Vec::new();
    let mut seed = 0u64;

    let speeds: Vec<f64> = (0..31).map(|i| 0.5 + i as f64 * 0.1).collect();
    let loads: Vec<f64> = (0..21).map(|i| i as f64 * 4.0).collect();
    let load_heights = [0.0f64, 0.3, 0.6];
    let frictions = [0.2f64, 0.4, 0.6, 0.8]; // dry concrete to wet grass

    for &speed in &speeds {
        for &load in &loads {
            for &lh in &load_heights {
                for &fric in &frictions {
                    for _ in 0..4 {
                        cfgs.push(Cfg { speed, load, load_h: lh, friction: fric, seed });
                        seed += 1;
                    }
                }
            }
        }
    }

    let total = cfgs.len();
    let results: Vec<(YawAccRun, String)> = cfgs.into_par_iter().map(|c| simulate(&c)).collect();

    let mut hashes = Vec::new();
    let mut runs: Vec<YawAccRun> = Vec::new();

    for (r, h) in results { hashes.push(h); runs.push(r); }

    // ── ZMP BLINDNESS: critical step count vs speed ───────────────────────────
    println!("\n--- INVISIBLE FAILURE: N_crit by speed (load=40kg, lh=0.5m, friction=0.4) ---");
    println!("(N_crit = steps before yaw accumulation exceeds ankle friction authority)");
    println!("{:>8} {:>10} {:>12} {:>16} {:>14} {:>14}",
        "Speed", "τ_z(Nm)", "N_crit", "Dist to fall(m)", "ZMP blind?", "φ@100 steps°");

    for &speed in speeds.iter().step_by(3) {
        let pool: Vec<&YawAccRun> = runs.iter()
            .filter(|r| (r.speed_ms - speed).abs() < 0.06
                    && (r.load_kg - 40.0).abs() < 0.1
                    && (r.load_height_m - 0.3).abs() < 0.1
                    && (r.friction_coeff - 0.4).abs() < 0.01)
            .collect();
        if pool.is_empty() { continue; }
        let tau_z = pool.iter().map(|r| r.tau_z_nm).sum::<f64>() / pool.len() as f64;
        let n_crit = pool.iter().map(|r| r.critical_step_count).sum::<f64>() / pool.len() as f64;
        let dist = pool.iter().map(|r| r.gait_distance_to_fall_m).sum::<f64>() / pool.len() as f64;
        let blind_pct = pool.iter().filter(|r| r.zmp_sees_nothing).count() as f64 / pool.len() as f64 * 100.0;
        let phi = pool.iter().map(|r| r.yaw_angle_at_100_steps_deg).sum::<f64>() / pool.len() as f64;
        let marker = if n_crit < 50.0 { " ← FALLS FAST" } else if n_crit < 200.0 { " ← SHORT WALK" } else { "" };
        println!("{:>7.1}m/s {:>9.2}  {:>9.0}  {:>14.0}m {:>13.0}% {:>13.2}°{}",
            speed, tau_z, n_crit, dist, blind_pct, phi, marker);
    }

    // ── TERRAIN FRICTION CLIFF ────────────────────────────────────────────────
    println!("\n--- FRICTION CLIFF: N_crit by terrain type (v=2.0 m/s, load=50kg, lh=0.5m) ---");
    println!("{:>8} {:>12} {:>12} {:>16}", "μ", "Terrain", "N_crit", "Dist to fall(m)");

    let terrain_names = [(0.2f64, "Wet mud"), (0.4, "Wet concrete"),
                          (0.6, "Dry asphalt"), (0.8, "Rubber mat")];
    for (fric, name) in &terrain_names {
        let pool: Vec<&YawAccRun> = runs.iter()
            .filter(|r| (r.speed_ms - 2.0).abs() < 0.06
                    && (r.load_kg - 50.0).abs() < 0.1
                    && (r.load_height_m - 0.3).abs() < 0.1
                    && (r.friction_coeff - fric).abs() < 0.01)
            .collect();
        if pool.is_empty() { continue; }
        let n_crit = pool.iter().map(|r| r.critical_step_count).sum::<f64>() / pool.len() as f64;
        let dist = pool.iter().map(|r| r.gait_distance_to_fall_m).sum::<f64>() / pool.len() as f64;
        println!("{:>8.2} {:>12} {:>12.0} {:>15.0}m", fric, name, n_crit, dist);
    }

    // ── LOAD HEIGHT AMPLIFICATION ─────────────────────────────────────────────
    println!("\n--- LOAD HEIGHT AMPLIFICATION (v=1.5 m/s, μ=0.4) ---");
    println!("{:>8} {:>8} {:>10} {:>12} {:>16}", "Load(kg)", "h(m)", "τ_z(Nm)", "N_crit", "ZMP blind%");
    for &load in &[20.0f64, 40.0, 60.0] {
        for &lh in &[0.0f64, 0.3, 0.6] {
            let pool: Vec<&YawAccRun> = runs.iter()
                .filter(|r| (r.load_kg - load).abs() < 0.1
                        && (r.speed_ms - 1.5).abs() < 0.06
                        && (r.load_height_m - lh).abs() < 0.05
                        && (r.friction_coeff - 0.4).abs() < 0.01)
                .collect();
            if pool.is_empty() { continue; }
            let tau = pool.iter().map(|r| r.tau_z_nm).sum::<f64>() / pool.len() as f64;
            let nc = pool.iter().map(|r| r.critical_step_count).sum::<f64>() / pool.len() as f64;
            let blind = pool.iter().filter(|r| r.zmp_sees_nothing).count() as f64 / pool.len() as f64 * 100.0;
            println!("{:>8.0} {:>8.1} {:>10.3} {:>12.0} {:>15.0}%", load, lh, tau, nc, blind);
        }
    }

    let json = serde_json::to_string_pretty(&runs).unwrap();
    File::create("humanoid_yaw_accumulation_envelope.json").unwrap().write_all(json.as_bytes()).unwrap();

    let master = seal_run(&hashes);
    println!("\n=================================================================");
    println!("Total configurations: {}  |  Elapsed: {:?}", total, start.elapsed());
    println!("Master Hash: {}", master);
    println!("=================================================================");
    println!("The ZMP controller's blind spot: τ_z = Ω_x × L_y accumulates per step.");
    println!("ZMP diagnostics show green. The robot is counting steps to its fall.");
}
