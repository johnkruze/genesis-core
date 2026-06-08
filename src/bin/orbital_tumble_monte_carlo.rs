// Category 4: Orbital Systems & Deep Space (Attitude Control & EDL)
// 1000Hz Euler Integration of spacecraft attitude gyroscopic coupling with cross-inertia products.
// Enforces compile-time assertion that the state struct size is exactly 128 bytes to align with UMA cache lines.

use genesis_core::rng::Rng;
use rayon::prelude::*;
use serde::Serialize;
use std::fs::File;
use std::time::Instant;
use sha2::{Sha256, Digest};

// Arrow / Parquet imports for native writing
use arrow::array::*;
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::file::properties::WriterProperties;
use parquet::basic::Compression;
use std::sync::Arc;

const M_SPACECRAFT: f32 = 500.0;       // kg
const EARTH_GM: f64 = 3.986004418e14;  // m^3/s^2
const EARTH_RADIUS: f64 = 6378137.0;   // meters
const RCS_L_ARM: f32 = 1.5;            // meters
const RCS_MAX_FORCE: f32 = 5.0;        // Newtons

// 32-Dimensional state struct, size assertion = exactly 128 bytes
#[repr(C)]
#[derive(Clone, Copy, Debug, Serialize)]
struct OrbitalDynamicsState {
    timestamp: f32,                // 1. Time (4 bytes)
    quat: [f32; 4],                // 2-5. Attitude quaternion: [w, x, y, z] (16 bytes)
    ang_vel: [f32; 3],             // 6-8. Angular velocity (12 bytes)
    rcs_torques: [f32; 6],         // 9-14. Commanded thruster moments (24 bytes)
    thruster_jacobians: [f32; 6],  // 15-20. Thruster Jacobians (24 bytes)
    inertia_tensor: [f32; 5],      // 21-25. Principal / cross inertias: [I_xx, I_yy, I_zz, I_xy, I_xz] (20 bytes)
    fuel_fraction: f32,            // 26. Remaining fuel ratio (4 bytes)
    x_slosh: f32,                  // 27. Slosh displacement (4 bytes)
    v_slosh: f32,                  // 28. Slosh velocity (4 bytes)
    decoupled_w: [f32; 3],         // 29-31. Decoupled simulator angular velocity (12 bytes)
    decoupled_model_divergence: f32, // 32. Divergence between models (4 bytes)
}

// Compile-time assertion of exactly 128 bytes for L1/L2 cache line alignment
const _: () = assert!(std::mem::size_of::<OrbitalDynamicsState>() == 128);

#[derive(Serialize)]
struct SpacecraftStepState {
    timestamp: f32,
    spacecraft_quat_w: f32,
    spacecraft_quat_x: f32,
    spacecraft_quat_y: f32,
    spacecraft_quat_z: f32,
    ang_vel_x: f32,
    ang_vel_y: f32,
    ang_vel_z: f32,
    thruster_force_x: f32,
    thruster_force_y: f32,
    thruster_force_z: f32,
    inertia_tensor_xx: f32,
    inertia_tensor_yy: f32,
    inertia_tensor_zz: f32,
    inertia_tensor_xy: f32,
    inertia_tensor_xz: f32,
    fuel_fraction: f32,
    x_slosh: f32,
    v_slosh: f32,
    decoupled_model_divergence: f32,
    scenario: String,
    sha256_seal: String,
}

#[derive(Serialize)]
struct SpacecraftTrajectory {
    trajectory_id: String,
    data: Vec<SpacecraftStepState>,
    proof_hash: String,
    attitude_loss_fatal: bool,
    failure_mode: String,
}

fn run_single_trajectory(index: usize, seed: u64, scenario: &str) -> SpacecraftTrajectory {
    let mut rng = Rng::new(seed);
    
    // Inertia tensor principal values (typical small satellite)
    let i_xx_nom = rng.range(210.0, 230.0) as f32;
    let i_yy_nom = rng.range(240.0, 260.0) as f32;
    let i_zz_nom = rng.range(270.0, 290.0) as f32;
    
    // Initial attitude: upright
    let mut quat = [1.0f32, 0.0f32, 0.0f32, 0.0f32];
    
    let w_max_init = match scenario {
        "cross_inertia_coupling" => 12.0f64,
        "fuel_depletion" => 8.0f64,
        _ => 2.5f64,
    };
    let mut w_x = rng.range(0.1, w_max_init) as f32 * if rng.chance(0.5) { 1.0f32 } else { -1.0f32 };
    let mut w_y = rng.range(0.1, w_max_init) as f32 * if rng.chance(0.5) { 1.0f32 } else { -1.0f32 };
    let mut w_z = rng.range(0.1, w_max_init) as f32 * if rng.chance(0.5) { 1.0f32 } else { -1.0f32 };
    
    // Decoupled 1-axis simulator states (believes axes do not cross-couple)
    let mut dec_w_x = w_x;
    let mut dec_w_y = w_y;
    let mut dec_w_z = w_z;
    
    // LEO circular orbit setup (400km height)
    let orbit_altitude = 400000.0f64;
    let r_orbit = EARTH_RADIUS + orbit_altitude;
    let v_orbit = (EARTH_GM / r_orbit).sqrt() as f32;
    
    let inclination = 0.785398f32; // 45 degrees in rad
    // Orbital parameters (unobservable over 5s context, but kept as constants for gravity gradient calculation)
    let dist = r_orbit as f32;
    
    let dt = 0.001f32; // 1000Hz (1.0ms steps)
    let total_time = 5.0f32; // 5.0 second simulation
    let steps_count = (total_time / dt) as usize;
    
    let mut states = Vec::with_capacity(steps_count / 10);
    
    // Cryptographic running hash chain
    let mut running_hash = Sha256::new();
    running_hash.update(&seed.to_le_bytes());
    let mut last_hash = running_hash.finalize();
    
    let mut max_euler_divergence = 0.0f32;
    
    // Environment sweeps
    let (solar_pressure, cd) = match scenario {
        "solar_storm" => (2.0e-5f32, 4.5f32),
        "nominal" => (rng.range(1.0e-6, 5.0e-6) as f32, 2.2f32),
        _ => (3.0e-6f32, 2.2f32),
    };
    
    let mut fuel_fraction = if scenario == "fuel_depletion" { 0.4f32 } else { 1.0f32 };
    let mut x_slosh = 0.0f32;
    let mut v_slosh = 0.0f32;
    let mut dw_y_prev = 0.0f32;
    
    let mut consecutive_divergence_steps = 0;
    let mut attitude_loss_fatal = false;
    let mut failure_mode = "nominal".to_string();
    
    for step in 0..steps_count {
        let t = step as f32 * dt;
        
        // LEO coordinates pos/vel integration removed to align spatial and temporal scales
        
        // Dynamic fuel depletion (RCS thrusters consume fuel)
        let fuel_multiplier = if scenario == "fuel_depletion" { 2.5f32 } else { 1.0f32 };
        let fuel_depletion_rate = 0.05f32 * (w_x.abs() + w_y.abs() + w_z.abs()) * fuel_multiplier;
        fuel_fraction = (fuel_fraction - fuel_depletion_rate * dt).max(0.0f32);
        
        // 1-DOF spring-mass-damper slosh dynamics:
        // natural frequency omega_n = 3.5 rad/s, damping zeta = 0.08
        let omega_n = 3.5f32;
        let zeta = 0.08f32;
        let l_tank = 1.2f32;
        // Lateral acceleration of the tank attachment point is approximated by l_tank * dw_y_prev
        let a_lateral = l_tank * dw_y_prev;
        let a_slosh = -2.0f32 * zeta * omega_n * v_slosh - omega_n.powi(2) * x_slosh - a_lateral;
        v_slosh += a_slosh * dt;
        x_slosh += v_slosh * dt;
        x_slosh = x_slosh.clamp(-0.4f32, 0.4f32); // clamp to tank boundary
        
        // Dynamic shift of moments of inertia due to fuel depletion & structural loads
        let fuel_factor = 0.8f32 + 0.2f32 * fuel_fraction;
        let i_xx = i_xx_nom * fuel_factor;
        let i_yy = i_yy_nom * fuel_factor;
        let i_zz = i_zz_nom * fuel_factor;
        // Asymmetry cross products of inertia
        let cross_coupling_boost = if scenario == "cross_inertia_coupling" { 35.0f32 } else { 0.0f32 };
        let cross_coupling_xz_boost = if scenario == "cross_inertia_coupling" { 25.0f32 } else { 0.0f32 };
        let i_xy = 15.0f32 * (1.0f32 - fuel_fraction) + cross_coupling_boost;
        
        // Fuel slosh mass scales with fuel fraction (max slosh mass = 50kg)
        let m_slosh = 50.0f32 * fuel_fraction;
        let i_xz = 8.0f32 * (1.0f32 - fuel_fraction) + x_slosh * m_slosh * l_tank + cross_coupling_xz_boost;
        
        // Gravity Gradient perturbing moment
        let gravity_gradient = 3.0f32 * (EARTH_GM as f32) / dist.powi(3) * (i_zz - i_yy) * 1.0e-5f32;
        
        // RCS Cold-Gas Thrusters: attempting to damp tumbling to zero (active only if fuel remains)
        let kd_rcs = 90.0f32;
        let (cmd_torque_x, cmd_torque_y, cmd_torque_z) = if fuel_fraction > 0.0f32 {
            (-kd_rcs * w_x, -kd_rcs * w_y, -kd_rcs * w_z)
        } else {
            (0.0f32, 0.0f32, 0.0f32)
        };
        
        let mut force_x = cmd_torque_x / RCS_L_ARM;
        let mut force_y = cmd_torque_y / RCS_L_ARM;
        let mut force_z = cmd_torque_z / RCS_L_ARM;
        
        let limit_force = |f: &mut f32| {
            *f = f.clamp(-RCS_MAX_FORCE, RCS_MAX_FORCE);
        };
        limit_force(&mut force_x);
        limit_force(&mut force_y);
        limit_force(&mut force_z);
        
        // EKF / Divergence calculation
        let dec_dw_x = (force_x * RCS_L_ARM) / i_xx;
        let dec_dw_y = (force_y * RCS_L_ARM) / i_yy;
        let dec_dw_z = (force_z * RCS_L_ARM) / i_zz;
        
        dec_w_x += dec_dw_x * dt;
        dec_w_y += dec_dw_y * dt;
        dec_w_z += dec_dw_z * dt;
        
        let decoupled_model_divergence = ((w_x - dec_w_x).powi(2) + (w_y - dec_w_y).powi(2) + (w_z - dec_w_z).powi(2)).sqrt();
        if decoupled_model_divergence > max_euler_divergence {
            max_euler_divergence = decoupled_model_divergence;
        }
        
        // Active WBC Lyapunov reallocation & cross-coupling compensation
        // If gyroscopic divergence crosses threshold, activate feedback linearization torque boosts
        let (active_torque_x, active_torque_y, active_torque_z) = if fuel_fraction <= 0.0f32 {
            (0.0f32, 0.0f32, 0.0f32)
        } else if decoupled_model_divergence > 0.05f32 {
            // Compensate cross-axis coupling term: \omega \times (I \omega)
            let h_x = i_xx * w_x + i_xy * w_y + i_xz * w_z;
            let h_y = i_xy * w_x + i_yy * w_y;
            let h_z = i_xz * w_x + i_zz * w_z;
            
            let gyro_comp_x = w_y * h_z - w_z * h_y;
            let gyro_comp_y = w_z * h_x - w_x * h_z;
            let gyro_comp_z = w_x * h_y - w_y * h_x;
            
            let tx = -kd_rcs * 1.5f32 * w_x + gyro_comp_x;
            let ty = -kd_rcs * 1.5f32 * w_y + gyro_comp_y;
            let tz = -kd_rcs * 1.5f32 * w_z + gyro_comp_z;
            
            (tx, ty, tz)
        } else {
            (force_x * RCS_L_ARM, force_y * RCS_L_ARM, force_z * RCS_L_ARM)
        };
        
        // Dynamic Allocation to Positive/Negative Thruster Channels
        let tx_pos = (active_torque_x / RCS_L_ARM).max(0.0f32) * RCS_L_ARM;
        let tx_neg = ((-active_torque_x) / RCS_L_ARM).max(0.0f32) * RCS_L_ARM;
        let ty_pos = (active_torque_y / RCS_L_ARM).max(0.0f32) * RCS_L_ARM;
        let ty_neg = ((-active_torque_y) / RCS_L_ARM).max(0.0f32) * RCS_L_ARM;
        let tz_pos = (active_torque_z / RCS_L_ARM).max(0.0f32) * RCS_L_ARM;
        let tz_neg = ((-active_torque_z) / RCS_L_ARM).max(0.0f32) * RCS_L_ARM;
        
        let _rcs_torques = [tx_pos, tx_neg, ty_pos, ty_neg, tz_pos, tz_neg];
        let thruster_jacobians = [1.0f32, -1.0f32, 1.0f32, -1.0f32, 1.0f32, -1.0f32];
        
        // Newton-Euler equations with cross products of inertia
        let h_x = i_xx * w_x + i_xy * w_y + i_xz * w_z;
        let h_y = i_xy * w_x + i_yy * w_y;
        let h_z = i_xz * w_x + i_zz * w_z;
        
        let gyro_x = w_y * h_z - w_z * h_y;
        let gyro_y = w_z * h_x - w_x * h_z;
        let gyro_z = w_x * h_y - w_y * h_x;
        
        // Add gravity gradient and solar radiation pressure moments
        let storm_disturbance = if scenario == "solar_storm" {
            [
                5.0f32 * (10.0f32 * t).sin(),
                5.0f32 * (12.0f32 * t).cos(),
                5.0f32 * (8.0f32 * t).sin(),
            ]
        } else {
            [0.0f32; 3]
        };
        let f_x = active_torque_x - gyro_x + gravity_gradient + storm_disturbance[0];
        let f_y = active_torque_y - gyro_y + storm_disturbance[1];
        let f_z = active_torque_z - gyro_z + solar_pressure * RCS_L_ARM + storm_disturbance[2];
        
        // Invert inertia tensor
        let det_i = i_xx * i_yy * i_zz - i_xy * i_xy * i_zz - i_xz * i_xz * i_yy;
        let inv_i_11 = i_yy * i_zz / det_i;
        let inv_i_12 = -i_xy * i_zz / det_i;
        let inv_i_13 = -i_xz * i_yy / det_i;
        let inv_i_22 = (i_xx * i_zz - i_xz * i_xz) / det_i;
        let inv_i_23 = i_xy * i_xz / det_i;
        let inv_i_33 = (i_xx * i_yy - i_xy * i_xy) / det_i;
        
        let dw_x = inv_i_11 * f_x + inv_i_12 * f_y + inv_i_13 * f_z;
        let dw_y = inv_i_12 * f_x + inv_i_22 * f_y + inv_i_23 * f_z;
        let dw_z = inv_i_13 * f_x + inv_i_23 * f_y + inv_i_33 * f_z;
        
        w_x += dw_x * dt;
        w_y += dw_y * dt;
        w_z += dw_z * dt;
        dw_y_prev = dw_y;
        
        // Quaternion derivative
        let qw = quat[0]; let qx = quat[1]; let qy = quat[2]; let qz = quat[3];
        let qw_dot = -0.5f32 * (qx * w_x + qy * w_y + qz * w_z);
        let qx_dot =  0.5f32 * (qw * w_x + qy * w_z - qz * w_y);
        let qy_dot =  0.5f32 * (qw * w_y - qx * w_z + qz * w_x);
        let qz_dot =  0.5f32 * (qw * w_z + qx * w_y - qy * w_x);
        
        quat[0] += qw_dot * dt;
        quat[1] += qx_dot * dt;
        quat[2] += qy_dot * dt;
        quat[3] += qz_dot * dt;
        
        let q_norm = (quat[0].powi(2) + quat[1].powi(2) + quat[2].powi(2) + quat[3].powi(2)).sqrt();
        if q_norm > 0.0f32 {
            quat[0] /= q_norm; quat[1] /= q_norm; quat[2] /= q_norm; quat[3] /= q_norm;
        }
        
        let w_mag = (w_x * w_x + w_y * w_y + w_z * w_z).sqrt();
        if w_mag > 15.0f32 && decoupled_model_divergence > 0.0f32 {
            consecutive_divergence_steps += 1;
        } else {
            consecutive_divergence_steps = 0;
        }
        
        let is_logging_step = step % 10 == 0;
        let is_terminal_step = step == steps_count - 1 || consecutive_divergence_steps >= 50;

        if is_logging_step || is_terminal_step {
            // Cryptographic hash chain step seal update
            let mut hasher = Sha256::new();
            hasher.update(&last_hash);
            hasher.update(&t.to_le_bytes());
            hasher.update(&quat[0].to_le_bytes());
            hasher.update(&quat[1].to_le_bytes());
            hasher.update(&quat[2].to_le_bytes());
            hasher.update(&quat[3].to_le_bytes());
            hasher.update(&w_x.to_le_bytes());
            hasher.update(&w_y.to_le_bytes());
            hasher.update(&w_z.to_le_bytes());
            hasher.update(&tx_pos.to_le_bytes());
            hasher.update(&tx_neg.to_le_bytes());
            hasher.update(&ty_pos.to_le_bytes());
            hasher.update(&ty_neg.to_le_bytes());
            hasher.update(&tz_pos.to_le_bytes());
            hasher.update(&tz_neg.to_le_bytes());
            hasher.update(&thruster_jacobians[0].to_le_bytes());
            hasher.update(&thruster_jacobians[1].to_le_bytes());
            hasher.update(&thruster_jacobians[2].to_le_bytes());
            hasher.update(&thruster_jacobians[3].to_le_bytes());
            hasher.update(&thruster_jacobians[4].to_le_bytes());
            hasher.update(&thruster_jacobians[5].to_le_bytes());
            hasher.update(&i_xx.to_le_bytes());
            hasher.update(&i_yy.to_le_bytes());
            hasher.update(&i_zz.to_le_bytes());
            hasher.update(&i_xy.to_le_bytes());
            hasher.update(&i_xz.to_le_bytes());
            hasher.update(&fuel_fraction.to_le_bytes());
            hasher.update(&x_slosh.to_le_bytes());
            hasher.update(&v_slosh.to_le_bytes());
            hasher.update(&decoupled_model_divergence.to_le_bytes());

            last_hash = hasher.finalize();
            let sha256_seal = hex::encode(last_hash);

            states.push(SpacecraftStepState {
                timestamp: t,
                spacecraft_quat_w: quat[0],
                spacecraft_quat_x: quat[1],
                spacecraft_quat_y: quat[2],
                spacecraft_quat_z: quat[3],
                ang_vel_x: w_x,
                ang_vel_y: w_y,
                ang_vel_z: w_z,
                thruster_force_x: active_torque_x / RCS_L_ARM,
                thruster_force_y: active_torque_y / RCS_L_ARM,
                thruster_force_z: active_torque_z / RCS_L_ARM,
                inertia_tensor_xx: i_xx,
                inertia_tensor_yy: i_yy,
                inertia_tensor_zz: i_zz,
                inertia_tensor_xy: i_xy,
                inertia_tensor_xz: i_xz,
                fuel_fraction,
                x_slosh,
                v_slosh,
                decoupled_model_divergence,
                scenario: scenario.to_string(),
                sha256_seal,
            });
        }
        
        if consecutive_divergence_steps >= 50 {
            attitude_loss_fatal = true;
            failure_mode = "attitude_loss".to_string();
            break;
        }
    }
    
    let proof_hash = hex::encode(last_hash);
    let mut final_attitude_loss_fatal = attitude_loss_fatal;
    if max_euler_divergence > 0.15f32 {
        final_attitude_loss_fatal = true;
        failure_mode = "attitude_loss".to_string();
    }
    
    SpacecraftTrajectory {
        trajectory_id: format!("pg_orb_{:05x}", index),
        data: states,
        proof_hash,
        attitude_loss_fatal: final_attitude_loss_fatal,
        failure_mode,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n_trajectories: usize = args.get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(12000);
        
    let out_path = args.iter().position(|a| a == "--out")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "/Users/aijesusbro/Spectrum/data/products/orbital_systems_deep_space.parquet".to_string());

    let scenario = args.iter().position(|a| a == "--scenario")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "nominal".to_string());
        
    eprintln!("Generating {} Spacecraft trajectories to Parquet...", n_trajectories);
    let start = Instant::now();

    // Define Arrow schema
    let schema = Arc::new(Schema::new(vec![
        Field::new("timestamp", DataType::Float32, false),
        Field::new("spacecraft_quat_w", DataType::Float32, false),
        Field::new("spacecraft_quat_x", DataType::Float32, false),
        Field::new("spacecraft_quat_y", DataType::Float32, false),
        Field::new("spacecraft_quat_z", DataType::Float32, false),
        Field::new("ang_vel_x", DataType::Float32, false),
        Field::new("ang_vel_y", DataType::Float32, false),
        Field::new("ang_vel_z", DataType::Float32, false),
        Field::new("thruster_force_x", DataType::Float32, false),
        Field::new("thruster_force_y", DataType::Float32, false),
        Field::new("thruster_force_z", DataType::Float32, false),
        Field::new("inertia_tensor_xx", DataType::Float32, false),
        Field::new("inertia_tensor_yy", DataType::Float32, false),
        Field::new("inertia_tensor_zz", DataType::Float32, false),
        Field::new("inertia_tensor_xy", DataType::Float32, false),
        Field::new("inertia_tensor_xz", DataType::Float32, false),
        Field::new("fuel_fraction", DataType::Float32, false),
        Field::new("x_slosh", DataType::Float32, false),
        Field::new("v_slosh", DataType::Float32, false),
        Field::new("decoupled_model_divergence", DataType::Float32, false),
        Field::new("scenario", DataType::Utf8, false),
        Field::new("sha256_seal", DataType::Utf8, false),
        Field::new("trajectory_id", DataType::Utf8, false),
    ]));

    let file = File::create(&out_path).expect("Failed to create output Parquet file");
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .build();
    let mut writer = ArrowWriter::try_new(file, schema.clone(), Some(props))
        .expect("Failed to create ArrowWriter");
    
    // Seed generator
    let base_seed = 0x0AB1_7A1E_DACE_DA4Eu64;
    let seed_multiplier = 0x9E37_79B1_85EB_CA87u64;
    
    // Chunk size to prevent OOM
    let chunk_size = 2000;
    let mut written_count = 0;
    let mut total_rows = 0;
    
    while written_count < n_trajectories {
        let this_chunk_size = std::cmp::min(chunk_size, n_trajectories - written_count);
        let start_i = written_count;
        let end_i = start_i + this_chunk_size;
        
        let trajectories: Vec<SpacecraftTrajectory> = (start_i..end_i)
            .into_par_iter()
            .map(|i| {
                let seed = base_seed ^ (i as u64).wrapping_mul(seed_multiplier);
                let scenario_for_traj = if scenario == "sweep" {
                    match i % 4 {
                        0 => "nominal",
                        1 => "fuel_depletion",
                        2 => "cross_inertia_coupling",
                        _ => "solar_storm",
                    }
                } else {
                    &scenario
                };
                run_single_trajectory(i, seed, scenario_for_traj)
            })
            .collect();
            
        // Columnar buffers for RecordBatch
        let mut timestamp = Vec::new();
        let mut spacecraft_quat_w = Vec::new();
        let mut spacecraft_quat_x = Vec::new();
        let mut spacecraft_quat_y = Vec::new();
        let mut spacecraft_quat_z = Vec::new();
        let mut ang_vel_x = Vec::new();
        let mut ang_vel_y = Vec::new();
        let mut ang_vel_z = Vec::new();
        let mut thruster_force_x = Vec::new();
        let mut thruster_force_y = Vec::new();
        let mut thruster_force_z = Vec::new();
        let mut inertia_tensor_xx = Vec::new();
        let mut inertia_tensor_yy = Vec::new();
        let mut inertia_tensor_zz = Vec::new();
        let mut inertia_tensor_xy = Vec::new();
        let mut inertia_tensor_xz = Vec::new();
        let mut fuel_fraction = Vec::new();
        let mut x_slosh = Vec::new();
        let mut v_slosh = Vec::new();
        let mut decoupled_model_divergence = Vec::new();
        let mut scenario_vec = Vec::new();
        let mut sha256_seal = Vec::new();
        let mut trajectory_id = Vec::new();

        for traj in trajectories {
            let t_id = traj.trajectory_id;
            for step in traj.data {
                timestamp.push(step.timestamp);
                spacecraft_quat_w.push(step.spacecraft_quat_w);
                spacecraft_quat_x.push(step.spacecraft_quat_x);
                spacecraft_quat_y.push(step.spacecraft_quat_y);
                spacecraft_quat_z.push(step.spacecraft_quat_z);
                ang_vel_x.push(step.ang_vel_x);
                ang_vel_y.push(step.ang_vel_y);
                ang_vel_z.push(step.ang_vel_z);
                thruster_force_x.push(step.thruster_force_x);
                thruster_force_y.push(step.thruster_force_y);
                thruster_force_z.push(step.thruster_force_z);
                inertia_tensor_xx.push(step.inertia_tensor_xx);
                inertia_tensor_yy.push(step.inertia_tensor_yy);
                inertia_tensor_zz.push(step.inertia_tensor_zz);
                inertia_tensor_xy.push(step.inertia_tensor_xy);
                inertia_tensor_xz.push(step.inertia_tensor_xz);
                fuel_fraction.push(step.fuel_fraction);
                x_slosh.push(step.x_slosh);
                v_slosh.push(step.v_slosh);
                decoupled_model_divergence.push(step.decoupled_model_divergence);
                scenario_vec.push(step.scenario.clone());
                sha256_seal.push(step.sha256_seal);
                trajectory_id.push(t_id.clone());
            }
        }
        
        let rows_in_batch = timestamp.len();
        if rows_in_batch > 0 {
            let batch = RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(Float32Array::from(timestamp)),
                    Arc::new(Float32Array::from(spacecraft_quat_w)),
                    Arc::new(Float32Array::from(spacecraft_quat_x)),
                    Arc::new(Float32Array::from(spacecraft_quat_y)),
                    Arc::new(Float32Array::from(spacecraft_quat_z)),
                    Arc::new(Float32Array::from(ang_vel_x)),
                    Arc::new(Float32Array::from(ang_vel_y)),
                    Arc::new(Float32Array::from(ang_vel_z)),
                    Arc::new(Float32Array::from(thruster_force_x)),
                    Arc::new(Float32Array::from(thruster_force_y)),
                    Arc::new(Float32Array::from(thruster_force_z)),
                    Arc::new(Float32Array::from(inertia_tensor_xx)),
                    Arc::new(Float32Array::from(inertia_tensor_yy)),
                    Arc::new(Float32Array::from(inertia_tensor_zz)),
                    Arc::new(Float32Array::from(inertia_tensor_xy)),
                    Arc::new(Float32Array::from(inertia_tensor_xz)),
                    Arc::new(Float32Array::from(fuel_fraction)),
                    Arc::new(Float32Array::from(x_slosh)),
                    Arc::new(Float32Array::from(v_slosh)),
                    Arc::new(Float32Array::from(decoupled_model_divergence)),
                    Arc::new(StringArray::from(scenario_vec)),
                    Arc::new(StringArray::from(sha256_seal)),
                    Arc::new(StringArray::from(trajectory_id)),
                ],
            ).expect("Failed to create RecordBatch");

            writer.write(&batch).expect("Failed to write RecordBatch");
            total_rows += rows_in_batch;
        }

        written_count += this_chunk_size;
        eprintln!("  Generated {}/{} trajectories...", written_count, n_trajectories);
    }
    
    writer.close().expect("Failed to close ArrowWriter");
    
    eprintln!("Successfully generated dataset ({} total rows). Total time: {:.2?}", total_rows, start.elapsed());
}
