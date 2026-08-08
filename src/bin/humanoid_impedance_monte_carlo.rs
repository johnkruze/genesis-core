// Category 5: Humanoid Active Joint-Impedance & Contact Stability
// 1000Hz Symplectic Euler integration of humanoid walking on variable-stiffness contact manifold.
// Enforces a compile-time assertion that the state struct size is exactly 128 bytes to align with Apple UMA cache line.

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

const M_TOTAL: f32 = 120.0;     // kg - 120kg Humanoid
const G: f32 = 9.81;            // m/s^2
const H_COM: f32 = 0.9;         // m - COM height
const I_PELVIS: f32 = 15.0;     // kg*m^2 - pitch rotational inertia
const M_FOOT: f32 = 5.0;        // kg - foot mass
const L_THIGH: f32 = 0.45;      // m - thigh segment length
const L_SHIN: f32 = 0.45;       // m - shin segment length
const FOOT_CONTACT_OFFSET: f32 = 0.012; // 12mm ankle/sole height

// 32-Dimensional state struct, size assertion = exactly 128 bytes
#[allow(dead_code)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Serialize)]
struct ImpedanceState {
    timestamp: f32,                // 1. Timestamp (4 bytes)
    q: [f32; 6],                   // 2-7. Joint positions: [hip_l, knee_l, ankle_l, hip_r, knee_r, ankle_r] (24 bytes)
    dq: [f32; 6],                  // 8-13. Joint velocities: [hip_l, knee_l, ankle_l, hip_r, knee_r, ankle_r] (24 bytes)
    torques: [f32; 6],             // 14-19. Commanded torques: [hip_l, knee_l, ankle_l, hip_r, knee_r, ankle_r] (24 bytes)
    contact_forces: [f32; 2],      // 20-21. Ground normal forces: [left, right] (8 bytes)
    j_contact_l: [f32; 4],         // 22-25. Left leg Contact Jacobian: [j_11, j_12, j_21, j_22] (16 bytes)
    j_contact_r: [f32; 4],         // 26-29. Right leg Contact Jacobian: [j_11, j_12, j_21, j_22] (16 bytes)
    m_11: [f32; 2],                // 30-31. Dynamic joint-space mass term M_11: [left, right] (8 bytes)
    thermal_accumulated: f32,      // 32. Integrated torque squared (4 bytes)
}

// Compile-time assertion of exactly 128 bytes for L1/L2 cache line alignment
const _: () = assert!(std::mem::size_of::<ImpedanceState>() == 128);

#[derive(Serialize)]
struct ImpedanceStepState {
    timestamp: f32,
    q_hip_l: f32,
    q_knee_l: f32,
    q_ankle_l: f32,
    q_hip_r: f32,
    q_knee_r: f32,
    q_ankle_r: f32,
    dq_hip_l: f32,
    dq_knee_l: f32,
    dq_ankle_l: f32,
    dq_hip_r: f32,
    dq_knee_r: f32,
    dq_ankle_r: f32,
    torque_hip_l: f32,
    torque_knee_l: f32,
    torque_ankle_l: f32,
    torque_hip_r: f32,
    torque_knee_r: f32,
    torque_ankle_r: f32,
    contact_force_l: f32,
    contact_force_r: f32,
    j_11_l: f32,
    j_12_l: f32,
    j_21_l: f32,
    j_22_l: f32,
    j_11_r: f32,
    j_12_r: f32,
    j_21_r: f32,
    j_22_r: f32,
    m_11_l: f32,
    m_11_r: f32,
    thermal_accumulated: f32,
    mu_friction: f32,
    mu_est: f32,
    slip_velocity_l: f32,
    slip_velocity_r: f32,
    com_pos_x: f32,
    com_pos_z: f32,
    com_vel_x: f32,
    com_vel_z: f32,
    pitch_rad: f32,
    pitch_vel: f32,
    scenario: String,
    sha256_seal: String,
    gear_wear_factor: f32,
    backlash_limit: f32,
    tendon_elongation_l_mm: f32,
    tendon_elongation_r_mm: f32,
    payload_slosh_displacement_m: f32,
    payload_mass_kg: f32,
    battery_cell_temp_c: f32,
    bms_trip: bool,
}

#[derive(Serialize)]
struct ImpedanceTrajectory {
    trajectory_id: String,
    data: Vec<ImpedanceStepState>,
    proof_hash: String,
    buckle_failure: bool,
    thermal_failure: bool,
}

fn solve_3x3(m: [[f32; 3]; 3], b: [f32; 3]) -> [f32; 3] {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
            
    if det.abs() < 1e-6 {
        return [b[0]/4.0, b[1]/3.0, b[2]/1.5];
    }
    
    let inv_det = 1.0 / det;
    
    let x0 = ((m[1][1] * m[2][2] - m[1][2] * m[2][1]) * b[0]
            - (m[0][1] * m[2][2] - m[0][2] * m[2][1]) * b[1]
            + (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * b[2]) * inv_det;
            
    let x1 = (-(m[1][0] * m[2][2] - m[1][2] * m[2][0]) * b[0]
            + (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * b[1]
            - (m[0][0] * m[1][2] - m[0][2] * m[1][0]) * b[2]) * inv_det;
            
    let x2 = ((m[1][0] * m[2][1] - m[1][1] * m[2][0]) * b[0]
            - (m[0][0] * m[2][1] - m[0][1] * m[2][0]) * b[1]
            + (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * b[2]) * inv_det;
            
    [x0, x1, x2]
}

fn run_single_trajectory(index: usize, seed: u64, scenario: &str) -> ImpedanceTrajectory {
    let mut rng = Rng::new(seed);
    let m_total_base = rng.range(80.0, 150.0) as f32;
    let m_total = if scenario == "isometric_hold" { m_total_base * 1.5f32 } else { m_total_base };
    let mass_factor = m_total / M_TOTAL;
    
    // Joint impedance variations (proportional and derivative stiffness)
    let kp_hip = rng.range(280.0, 360.0) as f32;
    let kp_knee = rng.range(420.0, 520.0) as f32;
    let kp_ankle = rng.range(220.0, 260.0) as f32;
    
    let kd_hip = rng.range(20.0, 35.0) as f32;
    let kd_knee = rng.range(30.0, 45.0) as f32;
    let kd_ankle = rng.range(12.0, 18.0) as f32;
    
    // Virtual Model Control gains for pelvis pitch stabilization (randomized to capture unstable regimes)
    let kp_pelvis = rng.range(50.0, 3500.0) as f32;
    let kd_pelvis = rng.range(5.0, 250.0) as f32;
    
    // Surface friction randomized per trajectory to simulate varied terrain materials
    let mu_friction = if scenario == "nominal" {
        rng.range(0.15, 0.60) as f32
    } else {
        match scenario {
            "ice" | "low_friction" => 0.10f32,
            _ => 0.50f32,
        }
    };
    
    // Initial states
    let mut pos_x = 0.0f32;
    let mut pos_z = H_COM;
    
    let mut vel_x = rng.range(1.1, 1.4) as f32; // Forward velocity m/s
    let vel_z = 0.0f32;
    
    let mut pitch_rad = 0.0f32;
    let mut pitch_vel = 0.0f32;
    
    // Joint states (actual angles)
    let mut q_hip_l = 0.0f32;
    let mut q_knee_l = 0.0f32;
    let mut q_ankle_l = 0.0f32;
    
    let mut q_hip_r = 0.0f32;
    let mut q_knee_r = 0.0f32;
    let mut q_ankle_r = 0.0f32;
    
    // Joint velocities
    let mut dq_hip_l = 0.0f32;
    let mut dq_knee_l = 0.0f32;
    let mut dq_ankle_l = 0.0f32;
    
    let mut dq_hip_r = 0.0f32;
    let mut dq_knee_r = 0.0f32;
    let mut dq_ankle_r = 0.0f32;
    
    // Instability indicators
    let mut slip_distance_l = 0.0f32;
    let mut slip_distance_r = 0.0f32;
    let mut slip_vel_l = 0.0f32;
    let mut slip_vel_r = 0.0f32;
    let mut thermal_tax = 0.0f32;
    let mut buckle_failure = false;
    let mut thermal_failure = false;
    
    let dt = 0.001f32; // 1000Hz solver steps
    let total_time = 5.0f32; // 5.0s simulation
    let steps_count = (total_time / dt) as usize;
    
    // Adversarial sweep setup
    let compromise_active = scenario == "adversarial";
    let compromise_time = rng.range(0.25, 0.75) as f32; // Compromise happens midway
    let compromised_joint = rng.range(0.0, 6.0) as usize;  // 0-2: left hip/knee/ankle, 3-5: right hip/knee/ankle
    let compromised_limb_is_left = compromised_joint < 3;
    let sensor_delay_steps = rng.range(50.0, 150.0) as usize; // 50ms - 150ms feedback delay
    
    // Ring buffers for delaying sensor readings
    let mut history_normal_l = vec![0.0f32; steps_count];
    let mut history_normal_r = vec![0.0f32; steps_count];
    
    let mut states = Vec::with_capacity(steps_count / 10);
    
    let (mut j_11_l, mut j_12_l, mut j_21_l, mut j_22_l, mut j_11_r, mut j_12_r, mut j_21_r, mut j_22_r);
    
    let mut running_hash = Sha256::new();
    running_hash.update(&seed.to_le_bytes());
    let mut last_hash = running_hash.finalize();
    
    // Pick-a-Part dynamic states
    let mut gear_wear_factor = 0.0f32;
    let mut tendon_elongation_l_mm = 0.0f32;
    let mut tendon_elongation_r_mm = 0.0f32;
    let mut torque_sq = 0.0f32;
    let mut torque_ankle_l_prev = 0.0f32;
    let mut torque_ankle_r_prev = 0.0f32;
    let payload_mass_kg = if scenario == "liquid_transport" { 10.0f32 } else { 0.0f32 };
    let slosh_omega = 1.2f32 * 2.0f32 * std::f32::consts::PI; // 1.2 Hz
    let slosh_stiffness = payload_mass_kg * slosh_omega * slosh_omega;
    let slosh_damping = 1.5f32; // N*s/m
    let mut slosh_displacement_m = 0.0f32;
    let mut slosh_velocity_ms = 0.0f32;
    let mut battery_cell_temp_c = 25.0f32;
    let mut bms_trip = false;
    let mut seize_duration = 0;
    
    for step in 0..steps_count {
        let t = step as f32 * dt;
        
        // 1. Variable-stiffness terrain model
        // Dynamic soil stiffness varies spatially as a sinusoidal profile
        let terrain_stiffness_base = match scenario {
            "soft_terrain" | "mud" => 10000.0f32,
            _ => 50000.0f32,
        };
        let terrain_stiffness = terrain_stiffness_base + 30000.0f32 * (2.0f32 * std::f32::consts::PI * pos_x).sin();
        let terrain_damping = 800.0f32;
        
        // 2. Target swing generator (desired joint configurations)
        let step_cycle = if scenario == "liquid_transport" {
            0.7f32 + (t / 5.0f32) * 0.4f32 // sweeps step cycle 0.7s -> 1.1s (~1.43 Hz down to 0.9 Hz)
        } else {
            0.8f32
        };
        let omega_gait = 2.0f32 * std::f32::consts::PI / step_cycle;
        
        let is_isometric = scenario == "isometric_hold";
        
        let (q_des_hip_l, q_des_knee_l, q_des_ankle_l, q_des_hip_r, q_des_knee_r, q_des_ankle_r) = if is_isometric {
            // Static squat position holding load against gravity
            (0.15f32, -0.60f32, 0.45f32, 0.15f32, -0.60f32, 0.45f32)
        } else {
            let q_hip_l_des = 0.3f32 * (omega_gait * t).sin();
            let q_knee_l_des = -0.25f32 * (1.0f32 - (omega_gait * t).cos());
            let q_ankle_l_des = -q_hip_l_des - q_knee_l_des;
            
            let q_hip_r_des = 0.3f32 * (omega_gait * t + std::f32::consts::PI).sin();
            let q_knee_r_des = -0.25f32 * (1.0f32 - (omega_gait * t + std::f32::consts::PI).cos());
            let q_ankle_r_des = -q_hip_r_des - q_knee_r_des;
            (q_hip_l_des, q_knee_l_des, q_ankle_l_des, q_hip_r_des, q_knee_r_des, q_ankle_r_des)
        };
        
        let (dq_des_hip_l, dq_des_knee_l, dq_des_ankle_l, dq_des_hip_r, dq_des_knee_r, dq_des_ankle_r) = if is_isometric {
            (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32)
        } else {
            let dq_hip_l_des = 0.3f32 * omega_gait * (omega_gait * t).cos();
            let dq_knee_l_des = -0.25f32 * omega_gait * (omega_gait * t).sin();
            let dq_ankle_l_des = -dq_hip_l_des - dq_knee_l_des;
            
            let dq_hip_r_des = 0.3f32 * omega_gait * (omega_gait * t + std::f32::consts::PI).cos();
            let dq_knee_r_des = -0.25f32 * omega_gait * (omega_gait * t + std::f32::consts::PI).sin();
            let dq_ankle_r_des = -dq_hip_r_des - dq_knee_r_des;
            (dq_hip_l_des, dq_knee_l_des, dq_ankle_l_des, dq_hip_r_des, dq_knee_r_des, dq_ankle_r_des)
        };
        
        // 3. Active Impedance Joint Controller: apply backlash dynamically
        let apply_backlash = |q_des: f32, q: f32, backlash_limit: f32| -> f32 {
            let q_error = q_des - q;
            if q_error.abs() > backlash_limit {
                q_error - q_error.signum() * backlash_limit
            } else {
                0.0f32
            }
        };
        
        // Update gear wear factor and stiction/backlash limits dynamically
        let wear_rate = if scenario == "wear_fatigue" { 0.25f32 } else { 0.01f32 };
        gear_wear_factor = (gear_wear_factor + (torque_sq / 10000.0f32) * wear_rate * dt).min(1.0f32);
        
        let mut seize_event = false;
        let dynamic_backlash = if gear_wear_factor >= 0.95f32 {
            seize_event = true;
            0.080f32 // Backlash spike > 50 mrad
        } else {
            0.002f32 + 0.013f32 * gear_wear_factor
        };
        
        if seize_event {
            seize_duration += 1;
            if seize_duration >= 100 { // 100ms
                buckle_failure = true;
            }
        } else {
            seize_duration = 0;
        }

        // Update tendon elongation and asymmetric creep factors
        let creep_rate = if scenario == "wear_fatigue" { 0.5f32 } else { 0.02f32 };
        let creep_accum_l = (torque_ankle_l_prev.abs() / 80.0f32).powi(2) * creep_rate * dt;
        let creep_accum_r = (torque_ankle_r_prev.abs() / 80.0f32).powi(2) * creep_rate * dt;
        tendon_elongation_l_mm += creep_accum_l * 5.0f32; // scale to mm
        tendon_elongation_r_mm += creep_accum_r * 5.0f32;
        
        let creep_factor_l = (tendon_elongation_l_mm / 1.0f32).min(0.5f32); // Max 50% stiffness loss
        let creep_factor_r = (tendon_elongation_r_mm / 1.0f32).min(0.5f32);
        
        let kp_ankle_flex_l = kp_ankle;
        let kp_ankle_extend_l = kp_ankle * (1.0f32 - creep_factor_l);
        let kp_ankle_flex_r = kp_ankle;
        let kp_ankle_extend_r = kp_ankle * (1.0f32 - creep_factor_r);
        
        let err_ankle_l = q_des_ankle_l - q_ankle_l;
        let kp_ankle_active_l = if err_ankle_l < 0.0f32 { kp_ankle_extend_l } else { kp_ankle_flex_l };
        
        let err_ankle_r = q_des_ankle_r - q_ankle_r;
        let kp_ankle_active_r = if err_ankle_r < 0.0f32 { kp_ankle_extend_r } else { kp_ankle_flex_r };
        
        // Determine backlash limits (with dynamic spike under adversarial attack or wear)
        let get_backlash = |joint_idx: usize| -> f32 {
            if compromise_active && t >= compromise_time && joint_idx == compromised_joint {
                0.100f32 // 100 mrad backlash spike (mechanical compromise)
            } else if scenario == "extreme_backlash" || scenario == "backlash" {
                0.015f32
            } else {
                dynamic_backlash
            }
        };
        
        let bl_hip_l = get_backlash(0);
        let bl_knee_l = get_backlash(1);
        let bl_ankle_l = get_backlash(2);
        let bl_hip_r = get_backlash(3);
        let bl_knee_r = get_backlash(4);
        let bl_ankle_r = get_backlash(5);
        
        // 4. Contact Mechanics (Double-support phase determination)
        let foot_height_l = pos_z - (L_THIGH * q_hip_l.cos() + L_SHIN * (q_hip_l + q_knee_l).cos()) - FOOT_CONTACT_OFFSET;
        let foot_height_r = pos_z - (L_THIGH * q_hip_r.cos() + L_SHIN * (q_hip_r + q_knee_r).cos()) - FOOT_CONTACT_OFFSET;
        
        // True physical normal forces
        let normal_l = if foot_height_l < 0.0f32 {
            -foot_height_l * terrain_stiffness - vel_z * terrain_damping
        } else {
            0.0f32
        }.max(0.0f32);
        
        let normal_r = if foot_height_r < 0.0f32 {
            -foot_height_r * terrain_stiffness - vel_z * terrain_damping
        } else {
            0.0f32
        }.max(0.0f32);
        
        // Save history for sensor delay modeling
        history_normal_l[step] = normal_l;
        history_normal_r[step] = normal_r;
        
        // Determine sensor readings (measured forces, potentially delayed & spoofed)
        let mut normal_measured_l = normal_l;
        let mut normal_measured_r = normal_r;
        
        if compromise_active && t >= compromise_time {
            if compromised_limb_is_left {
                // Left sensor spoofed: delayed feedback + sinusodial feedback disruption
                let delay_idx = if step >= sensor_delay_steps { step - sensor_delay_steps } else { 0 };
                normal_measured_l = history_normal_l[delay_idx] + 25.0f32 * (5.0f32 * t).sin();
                if normal_measured_l < 0.0f32 { normal_measured_l = 0.0f32; }
            } else {
                // Right sensor spoofed: delayed feedback + sinusodial feedback disruption
                let delay_idx = if step >= sensor_delay_steps { step - sensor_delay_steps } else { 0 };
                normal_measured_r = history_normal_r[delay_idx] + 25.0f32 * (5.0f32 * t).sin();
                if normal_measured_r < 0.0f32 { normal_measured_r = 0.0f32; }
            }
        }
        
        // WBC Inconsistency Detection using Contact Jacobian and kinematics
        // Est. force based on nominal stiffness (50000.0) compared with measured forces, combined with tracking error
        let expected_force_est_l = if foot_height_l < 0.0f32 { -foot_height_l * 50000.0f32 } else { 0.0f32 };
        let expected_force_est_r = if foot_height_r < 0.0f32 { -foot_height_r * 50000.0f32 } else { 0.0f32 };
        
        let tracking_err_l = (q_des_hip_l - q_hip_l).abs() + (q_des_knee_l - q_knee_l).abs() + (q_des_ankle_l - q_ankle_l).abs();
        let tracking_err_r = (q_des_hip_r - q_hip_r).abs() + (q_des_knee_r - q_knee_r).abs() + (q_des_ankle_r - q_ankle_r).abs();
        
        let inconsistency_l = (expected_force_est_l - normal_measured_l).abs() + 1500.0f32 * tracking_err_l;
        let inconsistency_r = (expected_force_est_r - normal_measured_r).abs() + 1500.0f32 * tracking_err_r;
        
        let left_isolated = compromise_active && compromised_limb_is_left && inconsistency_l > 150.0f32;
        let right_isolated = compromise_active && !compromised_limb_is_left && inconsistency_r > 150.0f32;
        
        // Pelvis Pitch Stabilization Feedback (Virtual Model Control)
        let torque_stabilize = kp_pelvis * pitch_rad + kd_pelvis * pitch_vel;
        
        // Command torques with isolation and load redistribution logic
        let (mut torque_hip_l, mut torque_knee_l, mut torque_ankle_l, mut torque_hip_r, mut torque_knee_r, mut torque_ankle_r);
        
        if left_isolated {
            // Left leg compromised and isolated:
            // 1. Bypass compromised joints (safe passive damping)
            torque_hip_l = -0.5f32 * dq_hip_l;
            torque_knee_l = -0.5f32 * dq_knee_l;
            torque_ankle_l = -0.5f32 * dq_ankle_l;
            
            // 2. Retract compromised leg to lift foot and prevent tripping
            let q_des_hip_l_ret = 0.15f32;
            let q_des_knee_l_ret = -0.80f32;
            let q_des_ankle_l_ret = -q_des_hip_l_ret - q_des_knee_l_ret;
            
            // Apply a small joint control on top of damping to achieve retraction (bypassing jammed joints as much as possible)
            torque_hip_l += 50.0f32 * (q_des_hip_l_ret - q_hip_l);
            torque_knee_l += 50.0f32 * (q_des_knee_l_ret - q_knee_l);
            torque_ankle_l += 30.0f32 * (q_des_ankle_l_ret - q_ankle_l);
            
            // 3. Stable limb (Right): Boost stiffness, route 100% stabilization torque, extend to support weight
            let kp_hip_r_active = kp_hip * 1.6f32;
            let kp_knee_r_active = kp_knee * 1.6f32;
            let kp_ankle_r_active = kp_ankle_active_r * 1.6f32;
            let kd_hip_r_active = kd_hip * 1.6f32;
            let kd_knee_r_active = kd_knee * 1.6f32;
            let kd_ankle_r_active = kd_ankle * 1.6f32;
            
            let q_des_knee_r_ext = q_des_knee_r * 0.90f32; // extend slightly (less flexed knee)
            
            torque_hip_r = kp_hip_r_active * (apply_backlash(q_des_hip_r, q_hip_r, bl_hip_r) + pitch_rad)
                + kd_hip_r_active * (dq_des_hip_r - dq_hip_r + pitch_vel)
                + 1.0f32 * torque_stabilize; // Redirect 100% pelvis stabilization
            torque_knee_r = kp_knee_r_active * apply_backlash(q_des_knee_r_ext, q_knee_r, bl_knee_r) + kd_knee_r_active * (dq_des_knee_r - dq_knee_r);
            torque_ankle_r = kp_ankle_r_active * apply_backlash(-q_des_hip_r - q_des_knee_r_ext, q_ankle_r, bl_ankle_r) + kd_ankle_r_active * (dq_des_ankle_r - dq_ankle_r);
        } else if right_isolated {
            // Right leg compromised and isolated:
            // 1. Bypass compromised joints (safe passive damping)
            torque_hip_r = -0.5f32 * dq_hip_r;
            torque_knee_r = -0.5f32 * dq_knee_r;
            torque_ankle_r = -0.5f32 * dq_ankle_r;
            
            // 2. Retract compromised leg
            let q_des_hip_r_ret = 0.15f32;
            let q_des_knee_r_ret = -0.80f32;
            let q_des_ankle_r_ret = -q_des_hip_r_ret - q_des_knee_r_ret;
            
            torque_hip_r += 50.0f32 * (q_des_hip_r_ret - q_hip_r);
            torque_knee_r += 50.0f32 * (q_des_knee_r_ret - q_knee_r);
            torque_ankle_r += 30.0f32 * (q_des_ankle_r_ret - q_ankle_r);
            
            // 3. Stable limb (Left): Boost stiffness, route 100% stabilization torque, extend
            let kp_hip_l_active = kp_hip * 1.6f32;
            let kp_knee_l_active = kp_knee * 1.6f32;
            let kp_ankle_l_active = kp_ankle_active_l * 1.6f32;
            let kd_hip_l_active = kd_hip * 1.6f32;
            let kd_knee_l_active = kd_knee * 1.6f32;
            let kd_ankle_l_active = kd_ankle * 1.6f32;
            
            let q_des_knee_l_ext = q_des_knee_l * 0.90f32;
            
            torque_hip_l = kp_hip_l_active * (apply_backlash(q_des_hip_l, q_hip_l, bl_hip_l) + pitch_rad)
                + kd_hip_l_active * (dq_des_hip_l - dq_hip_l + pitch_vel)
                + 1.0f32 * torque_stabilize; // Redirect 100% pelvis stabilization
            torque_knee_l = kp_knee_l_active * apply_backlash(q_des_knee_l_ext, q_knee_l, bl_knee_l) + kd_knee_l_active * (dq_des_knee_l - dq_knee_l);
            torque_ankle_l = kp_ankle_l_active * apply_backlash(-q_des_hip_l - q_des_knee_l_ext, q_ankle_l, bl_ankle_l) + kd_ankle_l_active * (dq_des_ankle_l - dq_ankle_l);
        } else {
            // Nominal controller (before detection/isolation, or nominal scenario)
            torque_hip_l = kp_hip * (apply_backlash(q_des_hip_l, q_hip_l, bl_hip_l) + pitch_rad) 
                + kd_hip * (dq_des_hip_l - dq_hip_l + pitch_vel) 
                + 0.5f32 * torque_stabilize;
            torque_knee_l = kp_knee * apply_backlash(q_des_knee_l, q_knee_l, bl_knee_l) + kd_knee * (dq_des_knee_l - dq_knee_l);
            torque_ankle_l = kp_ankle_active_l * apply_backlash(q_des_ankle_l, q_ankle_l, bl_ankle_l) + kd_ankle * (dq_des_ankle_l - dq_ankle_l);
            
            torque_hip_r = kp_hip * (apply_backlash(q_des_hip_r, q_hip_r, bl_hip_r) + pitch_rad) 
                + kd_hip * (dq_des_hip_r - dq_hip_r + pitch_vel) 
                + 0.5f32 * torque_stabilize;
            torque_knee_r = kp_knee * apply_backlash(q_des_knee_r, q_knee_r, bl_knee_r) + kd_knee * (dq_des_knee_r - dq_knee_r);
            torque_ankle_r = kp_ankle_active_r * apply_backlash(q_des_ankle_r, q_ankle_r, bl_ankle_r) + kd_ankle * (dq_des_ankle_r - dq_ankle_r);
        }
        
        // Whole-Body Control (WBC) dynamic ankle-torque limits using MEASURED forces
        let mu_est = 0.40f32;
        let tau_ankle_max_l = if normal_measured_l > 0.0f32 {
            80.0f32.min(mu_est * normal_measured_l * 0.15f32)
        } else {
            80.0f32
        };
        let torque_ankle_l = torque_ankle_l.clamp(-tau_ankle_max_l, tau_ankle_max_l);
        
        let tau_ankle_max_r = if normal_measured_r > 0.0f32 {
            80.0f32.min(mu_est * normal_measured_r * 0.15f32)
        } else {
            80.0f32
        };
        let torque_ankle_r = torque_ankle_r.clamp(-tau_ankle_max_r, tau_ankle_max_r);
        
        // Actuator torque-squared thermal accumulation
        torque_sq = torque_hip_l.powi(2) + torque_knee_l.powi(2) + torque_ankle_l.powi(2)
            + torque_hip_r.powi(2) + torque_knee_r.powi(2) + torque_ankle_r.powi(2);
        thermal_tax += (torque_sq / 1000.0f32) * dt;
        
        if thermal_tax > 350.0f32 {
            thermal_failure = true;
        }
        
        // Battery thermal dynamics
        let battery_heating = (torque_sq / 12000.0f32) * dt;
        let cooling_coefficient = if scenario == "isometric_hold" { 0.005f32 } else { 0.015f32 };
        let battery_cooling = cooling_coefficient * (battery_cell_temp_c - 25.0f32) * dt;
        battery_cell_temp_c += battery_heating * 80.0f32 - battery_cooling;
        if battery_cell_temp_c > 85.0f32 {
            bms_trip = true;
            buckle_failure = true;
        }
        
        // Store previous ankle torques for creep loop
        torque_ankle_l_prev = torque_ankle_l;
        torque_ankle_r_prev = torque_ankle_r;
        
        let shear_cmd_l = torque_ankle_l / 0.15f32; // Commanded shear force based on ankle moment arm
        let shear_max_l = mu_friction * normal_l;
        if normal_l > 20.0f32 && shear_cmd_l.abs() > shear_max_l {
            let excess_force = shear_cmd_l.abs() - shear_max_l;
            let slip_accel = excess_force / M_FOOT;
            slip_vel_l += slip_accel * dt;
        } else {
            slip_vel_l *= 0.95f32;
        }
        slip_distance_l += slip_vel_l * dt;
        
        let shear_cmd_r = torque_ankle_r / 0.15f32;
        let shear_max_r = mu_friction * normal_r;
        if normal_r > 20.0f32 && shear_cmd_r.abs() > shear_max_r {
            let excess_force = shear_cmd_r.abs() - shear_max_r;
            let slip_accel = excess_force / M_FOOT;
            slip_vel_r += slip_accel * dt;
        } else {
            slip_vel_r *= 0.95f32;
        }
        slip_distance_r += slip_vel_r * dt;
        
        // Buckle catastrophe boundary condition
        if slip_distance_l > 0.40f32 || slip_distance_r > 0.40f32 {
            buckle_failure = true;
        }
        
        // Payload slosh dynamics (coupled pendulum)
        let force_prop_l = if normal_l > 0.0f32 { shear_cmd_l.clamp(-shear_max_l, shear_max_l) } else { 0.0f32 };
        let force_prop_r = if normal_r > 0.0f32 { shear_cmd_r.clamp(-shear_max_r, shear_max_r) } else { 0.0f32 };
        let approx_accel_x = (force_prop_l + force_prop_r) / m_total - 0.02f32 * vel_x;
        
        let slosh_accel = if payload_mass_kg > 0.0f32 {
            (-slosh_stiffness * slosh_displacement_m - slosh_damping * slosh_velocity_ms - (payload_mass_kg * approx_accel_x)) / payload_mass_kg
        } else {
            0.0f32
        };
        slosh_velocity_ms += slosh_accel * dt;
        slosh_displacement_m += slosh_velocity_ms * dt;
        let slosh_gravity_torque = payload_mass_kg * G * slosh_displacement_m;
        
        // 5. Instability Pitch Torque: base of support sliding forward creates pelvis moment
        let max_slip_dist = slip_distance_l.max(slip_distance_r);
        let torque_tipping = m_total * G * max_slip_dist + slosh_gravity_torque;
        // Newton's Third Law: reaction torque on pelvis from hips is exactly -(torque_hip_l + torque_hip_r)
        let pitch_accel = -torque_tipping / I_PELVIS - (torque_hip_l + torque_hip_r) / I_PELVIS;
        
        pitch_vel += pitch_accel * dt;
        pitch_rad += pitch_vel * dt;
        
        // COM height drops as body pitches or slips
        pos_z = H_COM - 0.7f32 * pitch_rad.abs() - 0.4f32 * max_slip_dist;
        if pos_z < 0.15f32 {
            buckle_failure = true;
        }
        
        // 6. Joint integration (internal dynamics using 3-DOF coupled rigid body leg EOM)
        let solve_coupled_leg = |q: &mut [f32; 3], dq: &mut [f32; 3], torques: [f32; 3], normal: f32, shear_prop: f32, j11: f32, j12: f32, j21: f32, j22: f32, q_min: [f32; 3], q_max: [f32; 3]| {
            // Mass Matrix terms (double pendulum + ankle actuator coupling)
            let m11 = (3.375f32 + 2.025f32 * q[1].cos()) * mass_factor;
            let m12 = (1.125f32 + 1.0125f32 * q[1].cos()) * mass_factor;
            let m13 = 0.125f32 * mass_factor;
            
            let m21 = m12;
            let m22 = 1.125f32 * mass_factor;
            let m23 = 0.125f32 * mass_factor;
            
            let m31 = m13;
            let m32 = m23;
            let m33 = 0.375f32 * mass_factor;
            
            let m_mat = [[m11, m12, m13], [m21, m22, m23], [m31, m32, m33]];

            // Coriolis / Centripetal vector
            let c1 = -1.0125f32 * q[1].sin() * (2.0f32 * dq[0] * dq[1] + dq[1] * dq[1]) * mass_factor;
            let c2 = 1.0125f32 * q[1].sin() * dq[0] * dq[0] * mass_factor;
            let c3 = -0.05f32 * q[1].sin() * dq[0] * dq[0] * mass_factor;
            
            // Gravity vector
            let g1 = (147.15f32 * q[0].sin() + 49.05f32 * (q[0] + q[1]).sin()) * mass_factor;
            let g2 = 49.05f32 * (q[0] + q[1]).sin() * mass_factor;
            let g_const = 9.81f32;
            let g3 = 1.5f32 * mass_factor * g_const * (q[0] + q[1] + q[2]).sin();

            // Contact forces mapping back to hip, knee, and ankle joint accelerations via J^T * F
            let j13 = FOOT_CONTACT_OFFSET * (q[0] + q[1] + q[2]).cos();
            let j23 = FOOT_CONTACT_OFFSET * (q[0] + q[1] + q[2]).sin();
            let tau_contact_ankle = j13 * shear_prop + j23 * normal;
            let tau_contact_hip = (j11 + j13) * shear_prop + (j21 + j23) * normal;
            let tau_contact_knee = (j12 + j13) * shear_prop + (j22 + j23) * normal;

            let b = [
                torques[0] - 0.25f32 * dq[0] - c1 - g1 + tau_contact_hip,
                torques[1] - 0.25f32 * dq[1] - c2 - g2 + tau_contact_knee,
                torques[2] - 0.25f32 * dq[2] - c3 - g3 + tau_contact_ankle,
            ];
            
            let acc = solve_3x3(m_mat, b);
            
            // Integrate velocities and positions with clamping
            for i in 0..3 {
                dq[i] = (dq[i] + acc[i] * dt).clamp(-12.0f32, 12.0f32);
                q[i] += dq[i] * dt;
                if q[i] < q_min[i] { q[i] = q_min[i]; dq[i] = 0.0f32; }
                else if q[i] > q_max[i] { q[i] = q_max[i]; dq[i] = 0.0f32; }
            }
        };

        j_11_l = L_THIGH * q_hip_l.cos() + L_SHIN * (q_hip_l + q_knee_l).cos();
        j_12_l = L_SHIN * (q_hip_l + q_knee_l).cos();
        j_21_l = L_THIGH * q_hip_l.sin() + L_SHIN * (q_hip_l + q_knee_l).sin();
        j_22_l = L_SHIN * (q_hip_l + q_knee_l).sin();
        
        j_11_r = L_THIGH * q_hip_r.cos() + L_SHIN * (q_hip_r + q_knee_r).cos();
        j_12_r = L_SHIN * (q_hip_r + q_knee_r).cos();
        j_21_r = L_THIGH * q_hip_r.sin() + L_SHIN * (q_hip_r + q_knee_r).sin();
        j_22_r = L_SHIN * (q_hip_r + q_knee_r).sin();

        let mut q_l = [q_hip_l, q_knee_l, q_ankle_l];
        let mut dq_l = [dq_hip_l, dq_knee_l, dq_ankle_l];
        let mut q_r = [q_hip_r, q_knee_r, q_ankle_r];
        let mut dq_r = [dq_hip_r, dq_knee_r, dq_ankle_r];

        solve_coupled_leg(&mut q_l, &mut dq_l, [torque_hip_l, torque_knee_l, torque_ankle_l], normal_l, shear_cmd_l.clamp(-shear_max_l, shear_max_l), j_11_l, j_12_l, j_21_l, j_22_l, [-1.5f32, -2.0f32, -1.0f32], [1.5f32, 0.0f32, 1.0f32]);
        solve_coupled_leg(&mut q_r, &mut dq_r, [torque_hip_r, torque_knee_r, torque_ankle_r], normal_r, shear_cmd_r.clamp(-shear_max_r, shear_max_r), j_11_r, j_12_r, j_21_r, j_22_r, [-1.5f32, -2.0f32, -1.0f32], [1.5f32, 0.0f32, 1.0f32]);

        q_hip_l = q_l[0]; q_knee_l = q_l[1]; q_ankle_l = q_l[2];
        dq_hip_l = dq_l[0]; dq_knee_l = dq_l[1]; dq_ankle_l = dq_l[2];
        
        q_hip_r = q_r[0]; q_knee_r = q_r[1]; q_ankle_r = q_r[2];
        dq_hip_r = dq_r[0]; dq_knee_r = dq_r[1]; dq_ankle_r = dq_r[2];
        
        // COM velocities
        let force_prop_l = if normal_l > 0.0f32 { shear_cmd_l.clamp(-shear_max_l, shear_max_l) } else { 0.0f32 };
        let force_prop_r = if normal_r > 0.0f32 { shear_cmd_r.clamp(-shear_max_r, shear_max_r) } else { 0.0f32 };
        let accel_x = (force_prop_l + force_prop_r) / m_total - 0.02f32 * vel_x;
        vel_x += accel_x * dt;
        pos_x += vel_x * dt;
        
        // Contact Jacobians & Mass Matrices computation
        j_11_l = L_THIGH * q_hip_l.cos() + L_SHIN * (q_hip_l + q_knee_l).cos();
        j_12_l = L_SHIN * (q_hip_l + q_knee_l).cos();
        j_21_l = L_THIGH * q_hip_l.sin() + L_SHIN * (q_hip_l + q_knee_l).sin();
        j_22_l = L_SHIN * (q_hip_l + q_knee_l).sin();

        j_11_r = L_THIGH * q_hip_r.cos() + L_SHIN * (q_hip_r + q_knee_r).cos();
        j_12_r = L_SHIN * (q_hip_r + q_knee_r).cos();
        j_21_r = L_THIGH * q_hip_r.sin() + L_SHIN * (q_hip_r + q_knee_r).sin();
        j_22_r = L_SHIN * (q_hip_r + q_knee_r).sin();

        let m_11_l = (3.375f32 + 2.025f32 * q_knee_l.cos()) * mass_factor;
        let m_11_r = (3.375f32 + 2.025f32 * q_knee_r.cos()) * mass_factor;

        let is_logging_step = step % 10 == 0;
        let is_terminal_step = step == steps_count - 1 || buckle_failure || thermal_failure;

        if is_logging_step || is_terminal_step {
            // Cryptographic hash chain step seal update
            let mut hasher = Sha256::new();
            hasher.update(&last_hash);
            hasher.update(&t.to_le_bytes());
            hasher.update(&q_hip_l.to_le_bytes());
            hasher.update(&q_knee_l.to_le_bytes());
            hasher.update(&q_ankle_l.to_le_bytes());
            hasher.update(&q_hip_r.to_le_bytes());
            hasher.update(&q_knee_r.to_le_bytes());
            hasher.update(&q_ankle_r.to_le_bytes());
            hasher.update(&dq_hip_l.to_le_bytes());
            hasher.update(&dq_knee_l.to_le_bytes());
            hasher.update(&dq_ankle_l.to_le_bytes());
            hasher.update(&dq_hip_r.to_le_bytes());
            hasher.update(&dq_knee_r.to_le_bytes());
            hasher.update(&dq_ankle_r.to_le_bytes());
            hasher.update(&torque_hip_l.to_le_bytes());
            hasher.update(&torque_knee_l.to_le_bytes());
            hasher.update(&torque_ankle_l.to_le_bytes());
            hasher.update(&torque_hip_r.to_le_bytes());
            hasher.update(&torque_knee_r.to_le_bytes());
            hasher.update(&torque_ankle_r.to_le_bytes());
            hasher.update(&normal_l.to_le_bytes());
            hasher.update(&normal_r.to_le_bytes());
            hasher.update(&j_11_l.to_le_bytes());
            hasher.update(&j_12_l.to_le_bytes());
            hasher.update(&j_21_l.to_le_bytes());
            hasher.update(&j_22_l.to_le_bytes());
            hasher.update(&j_11_r.to_le_bytes());
            hasher.update(&j_12_r.to_le_bytes());
            hasher.update(&j_21_r.to_le_bytes());
            hasher.update(&j_22_r.to_le_bytes());
            hasher.update(&m_11_l.to_le_bytes());
            hasher.update(&m_11_r.to_le_bytes());
            hasher.update(&mu_friction.to_le_bytes());
            hasher.update(&mu_est.to_le_bytes());
            hasher.update(&slip_vel_l.to_le_bytes());
            hasher.update(&slip_vel_r.to_le_bytes());
            hasher.update(&thermal_tax.to_le_bytes());
            hasher.update(&pos_x.to_le_bytes());
            hasher.update(&pos_z.to_le_bytes());
            hasher.update(&vel_x.to_le_bytes());
            hasher.update(&vel_z.to_le_bytes());
            hasher.update(&pitch_rad.to_le_bytes());
            hasher.update(&pitch_vel.to_le_bytes());
            hasher.update(&gear_wear_factor.to_le_bytes());
            hasher.update(&dynamic_backlash.to_le_bytes());
            hasher.update(&tendon_elongation_l_mm.to_le_bytes());
            hasher.update(&tendon_elongation_r_mm.to_le_bytes());
            hasher.update(&slosh_displacement_m.to_le_bytes());
            hasher.update(&payload_mass_kg.to_le_bytes());
            hasher.update(&battery_cell_temp_c.to_le_bytes());
            hasher.update(&[bms_trip as u8]);
            
            last_hash = hasher.finalize();
            let sha256_seal = hex::encode(last_hash);

            states.push(ImpedanceStepState {
                timestamp: t,
                q_hip_l,
                q_knee_l,
                q_ankle_l,
                q_hip_r,
                q_knee_r,
                q_ankle_r,
                dq_hip_l,
                dq_knee_l,
                dq_ankle_l,
                dq_hip_r,
                dq_knee_r,
                dq_ankle_r,
                torque_hip_l,
                torque_knee_l,
                torque_ankle_l,
                torque_hip_r,
                torque_knee_r,
                torque_ankle_r,
                contact_force_l: normal_l,
                contact_force_r: normal_r,
                j_11_l,
                j_12_l,
                j_21_l,
                j_22_l,
                j_11_r,
                j_12_r,
                j_21_r,
                j_22_r,
                m_11_l,
                m_11_r,
                thermal_accumulated: thermal_tax,
                mu_friction,
                mu_est,
                slip_velocity_l: slip_vel_l,
                slip_velocity_r: slip_vel_r,
                com_pos_x: pos_x,
                com_pos_z: pos_z,
                com_vel_x: vel_x,
                com_vel_z: vel_z,
                pitch_rad,
                pitch_vel,
                scenario: scenario.to_string(),
                sha256_seal,
                gear_wear_factor,
                backlash_limit: dynamic_backlash,
                tendon_elongation_l_mm,
                tendon_elongation_r_mm,
                payload_slosh_displacement_m: slosh_displacement_m,
                payload_mass_kg,
                battery_cell_temp_c,
                bms_trip,
            });
        }
        
        if buckle_failure || thermal_failure {
            break;
        }
    }
    
    let proof_hash = hex::encode(last_hash);
    
    ImpedanceTrajectory {
        trajectory_id: format!("pg_hum_{:05x}", index),
        data: states,
        proof_hash,
        buckle_failure,
        thermal_failure,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n_trajectories: usize = args.get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10000);
        
    let out_path = args.iter().position(|a| a == "--out")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/products/humanoid_dynamics.parquet").to_string());

    let scenario = args.iter().position(|a| a == "--scenario")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "nominal".to_string());
        
    eprintln!("Generating {} Humanoid Impedance trajectories to Parquet...", n_trajectories);
    let start = Instant::now();

    // Define Arrow schema matching the fields
    let schema = Arc::new(Schema::new(vec![
        Field::new("timestamp", DataType::Float32, false),
        Field::new("q_hip_l", DataType::Float32, false),
        Field::new("q_knee_l", DataType::Float32, false),
        Field::new("q_ankle_l", DataType::Float32, false),
        Field::new("q_hip_r", DataType::Float32, false),
        Field::new("q_knee_r", DataType::Float32, false),
        Field::new("q_ankle_r", DataType::Float32, false),
        Field::new("dq_hip_l", DataType::Float32, false),
        Field::new("dq_knee_l", DataType::Float32, false),
        Field::new("dq_ankle_l", DataType::Float32, false),
        Field::new("dq_hip_r", DataType::Float32, false),
        Field::new("dq_knee_r", DataType::Float32, false),
        Field::new("dq_ankle_r", DataType::Float32, false),
        Field::new("torque_hip_l", DataType::Float32, false),
        Field::new("torque_knee_l", DataType::Float32, false),
        Field::new("torque_ankle_l", DataType::Float32, false),
        Field::new("torque_hip_r", DataType::Float32, false),
        Field::new("torque_knee_r", DataType::Float32, false),
        Field::new("torque_ankle_r", DataType::Float32, false),
        Field::new("contact_force_l", DataType::Float32, false),
        Field::new("contact_force_r", DataType::Float32, false),
        Field::new("j_11_l", DataType::Float32, false),
        Field::new("j_12_l", DataType::Float32, false),
        Field::new("j_21_l", DataType::Float32, false),
        Field::new("j_22_l", DataType::Float32, false),
        Field::new("j_11_r", DataType::Float32, false),
        Field::new("j_12_r", DataType::Float32, false),
        Field::new("j_21_r", DataType::Float32, false),
        Field::new("j_22_r", DataType::Float32, false),
        Field::new("m_11_l", DataType::Float32, false),
        Field::new("m_11_r", DataType::Float32, false),
        Field::new("thermal_accumulated", DataType::Float32, false),
        Field::new("mu_friction", DataType::Float32, false),
        Field::new("mu_est", DataType::Float32, false),
        Field::new("slip_velocity_l", DataType::Float32, false),
        Field::new("slip_velocity_r", DataType::Float32, false),
        Field::new("com_pos_x", DataType::Float32, false),
        Field::new("com_pos_z", DataType::Float32, false),
        Field::new("com_vel_x", DataType::Float32, false),
        Field::new("com_vel_z", DataType::Float32, false),
        Field::new("pitch_rad", DataType::Float32, false),
        Field::new("pitch_vel", DataType::Float32, false),
        Field::new("scenario", DataType::Utf8, false),
        Field::new("sha256_seal", DataType::Utf8, false),
        Field::new("trajectory_id", DataType::Utf8, false),
        Field::new("gear_wear_factor", DataType::Float32, false),
        Field::new("backlash_limit", DataType::Float32, false),
        Field::new("tendon_elongation_l_mm", DataType::Float32, false),
        Field::new("tendon_elongation_r_mm", DataType::Float32, false),
        Field::new("payload_slosh_displacement_m", DataType::Float32, false),
        Field::new("payload_mass_kg", DataType::Float32, false),
        Field::new("battery_cell_temp_c", DataType::Float32, false),
        Field::new("bms_trip", DataType::Boolean, false),
    ]));

    let file = File::create(&out_path).expect("Failed to create output Parquet file");
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .build();
    let mut writer = ArrowWriter::try_new(file, schema.clone(), Some(props))
        .expect("Failed to create ArrowWriter");
    
    // Seed generator
    let base_seed = 0x1337_F00D_BAAD_F00Du64;
    let seed_multiplier = 0x9E37_79B1_85EB_CA87u64;
    
    // Chunk size to prevent OOM
    let chunk_size = 2000;
    let mut written_count = 0;
    let mut total_rows = 0;
    
    while written_count < n_trajectories {
        let this_chunk_size = std::cmp::min(chunk_size, n_trajectories - written_count);
        let start_i = written_count;
        let end_i = start_i + this_chunk_size;
        
        let trajectories: Vec<ImpedanceTrajectory> = (start_i..end_i)
            .into_par_iter()
            .map(|i| {
                let seed = base_seed ^ (i as u64).wrapping_mul(seed_multiplier);
                let scenario_for_traj = if scenario == "sweep" {
                    match i % 8 {
                        0 => "nominal",
                        1 => "ice",
                        2 => "backlash",
                        3 => "mud",
                        4 => "adversarial",
                        5 => "wear_fatigue",
                        6 => "liquid_transport",
                        _ => "isometric_hold",
                    }
                } else {
                    &scenario
                };
                run_single_trajectory(i, seed, scenario_for_traj)
            })
            .collect();
            
        // Columnar buffers for RecordBatch
        let mut timestamp = Vec::new();
        let mut q_hip_l = Vec::new();
        let mut q_knee_l = Vec::new();
        let mut q_ankle_l = Vec::new();
        let mut q_hip_r = Vec::new();
        let mut q_knee_r = Vec::new();
        let mut q_ankle_r = Vec::new();
        let mut dq_hip_l = Vec::new();
        let mut dq_knee_l = Vec::new();
        let mut dq_ankle_l = Vec::new();
        let mut dq_hip_r = Vec::new();
        let mut dq_knee_r = Vec::new();
        let mut dq_ankle_r = Vec::new();
        let mut torque_hip_l = Vec::new();
        let mut torque_knee_l = Vec::new();
        let mut torque_ankle_l = Vec::new();
        let mut torque_hip_r = Vec::new();
        let mut torque_knee_r = Vec::new();
        let mut torque_ankle_r = Vec::new();
        let mut contact_force_l = Vec::new();
        let mut contact_force_r = Vec::new();
        let mut j_11_l = Vec::new();
        let mut j_12_l = Vec::new();
        let mut j_21_l = Vec::new();
        let mut j_22_l = Vec::new();
        let mut j_11_r = Vec::new();
        let mut j_12_r = Vec::new();
        let mut j_21_r = Vec::new();
        let mut j_22_r = Vec::new();
        let mut m_11_l = Vec::new();
        let mut m_11_r = Vec::new();
        let mut thermal_accumulated = Vec::new();
        let mut mu_friction = Vec::new();
        let mut mu_est = Vec::new();
        let mut slip_velocity_l = Vec::new();
        let mut slip_velocity_r = Vec::new();
        let mut com_pos_x = Vec::new();
        let mut com_pos_z = Vec::new();
        let mut com_vel_x = Vec::new();
        let mut com_vel_z = Vec::new();
        let mut pitch_rad = Vec::new();
        let mut pitch_vel = Vec::new();
        let mut scenario_vec = Vec::new();
        let mut sha256_seal = Vec::new();
        let mut trajectory_id = Vec::new();
        let mut gear_wear_factor = Vec::new();
        let mut backlash_limit = Vec::new();
        let mut tendon_elongation_l_mm = Vec::new();
        let mut tendon_elongation_r_mm = Vec::new();
        let mut payload_slosh_displacement_m = Vec::new();
        let mut payload_mass_kg = Vec::new();
        let mut battery_cell_temp_c = Vec::new();
        let mut bms_trip = Vec::new();

        for traj in trajectories {
            let t_id = traj.trajectory_id;
            for step in traj.data {
                timestamp.push(step.timestamp);
                q_hip_l.push(step.q_hip_l);
                q_knee_l.push(step.q_knee_l);
                q_ankle_l.push(step.q_ankle_l);
                q_hip_r.push(step.q_hip_r);
                q_knee_r.push(step.q_knee_r);
                q_ankle_r.push(step.q_ankle_r);
                dq_hip_l.push(step.dq_hip_l);
                dq_knee_l.push(step.dq_knee_l);
                dq_ankle_l.push(step.dq_ankle_l);
                dq_hip_r.push(step.dq_hip_r);
                dq_knee_r.push(step.dq_knee_r);
                dq_ankle_r.push(step.dq_ankle_r);
                torque_hip_l.push(step.torque_hip_l);
                torque_knee_l.push(step.torque_knee_l);
                torque_ankle_l.push(step.torque_ankle_l);
                torque_hip_r.push(step.torque_hip_r);
                torque_knee_r.push(step.torque_knee_r);
                torque_ankle_r.push(step.torque_ankle_r);
                contact_force_l.push(step.contact_force_l);
                contact_force_r.push(step.contact_force_r);
                j_11_l.push(step.j_11_l);
                j_12_l.push(step.j_12_l);
                j_21_l.push(step.j_21_l);
                j_22_l.push(step.j_22_l);
                j_11_r.push(step.j_11_r);
                j_12_r.push(step.j_12_r);
                j_21_r.push(step.j_21_r);
                j_22_r.push(step.j_22_r);
                m_11_l.push(step.m_11_l);
                m_11_r.push(step.m_11_r);
                thermal_accumulated.push(step.thermal_accumulated);
                mu_friction.push(step.mu_friction);
                mu_est.push(step.mu_est);
                slip_velocity_l.push(step.slip_velocity_l);
                slip_velocity_r.push(step.slip_velocity_r);
                com_pos_x.push(step.com_pos_x);
                com_pos_z.push(step.com_pos_z);
                com_vel_x.push(step.com_vel_x);
                com_vel_z.push(step.com_vel_z);
                pitch_rad.push(step.pitch_rad);
                pitch_vel.push(step.pitch_vel);
                scenario_vec.push(step.scenario.clone());
                sha256_seal.push(step.sha256_seal);
                trajectory_id.push(t_id.clone());
                gear_wear_factor.push(step.gear_wear_factor);
                backlash_limit.push(step.backlash_limit);
                tendon_elongation_l_mm.push(step.tendon_elongation_l_mm);
                tendon_elongation_r_mm.push(step.tendon_elongation_r_mm);
                payload_slosh_displacement_m.push(step.payload_slosh_displacement_m);
                payload_mass_kg.push(step.payload_mass_kg);
                battery_cell_temp_c.push(step.battery_cell_temp_c);
                bms_trip.push(step.bms_trip);
            }
        }
        
        let rows_in_batch = timestamp.len();
        if rows_in_batch > 0 {
            let batch = RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(Float32Array::from(timestamp)),
                    Arc::new(Float32Array::from(q_hip_l)),
                    Arc::new(Float32Array::from(q_knee_l)),
                    Arc::new(Float32Array::from(q_ankle_l)),
                    Arc::new(Float32Array::from(q_hip_r)),
                    Arc::new(Float32Array::from(q_knee_r)),
                    Arc::new(Float32Array::from(q_ankle_r)),
                    Arc::new(Float32Array::from(dq_hip_l)),
                    Arc::new(Float32Array::from(dq_knee_l)),
                    Arc::new(Float32Array::from(dq_ankle_l)),
                    Arc::new(Float32Array::from(dq_hip_r)),
                    Arc::new(Float32Array::from(dq_knee_r)),
                    Arc::new(Float32Array::from(dq_ankle_r)),
                    Arc::new(Float32Array::from(torque_hip_l)),
                    Arc::new(Float32Array::from(torque_knee_l)),
                    Arc::new(Float32Array::from(torque_ankle_l)),
                    Arc::new(Float32Array::from(torque_hip_r)),
                    Arc::new(Float32Array::from(torque_knee_r)),
                    Arc::new(Float32Array::from(torque_ankle_r)),
                    Arc::new(Float32Array::from(contact_force_l)),
                    Arc::new(Float32Array::from(contact_force_r)),
                    Arc::new(Float32Array::from(j_11_l)),
                    Arc::new(Float32Array::from(j_12_l)),
                    Arc::new(Float32Array::from(j_21_l)),
                    Arc::new(Float32Array::from(j_22_l)),
                    Arc::new(Float32Array::from(j_11_r)),
                    Arc::new(Float32Array::from(j_12_r)),
                    Arc::new(Float32Array::from(j_21_r)),
                    Arc::new(Float32Array::from(j_22_r)),
                    Arc::new(Float32Array::from(m_11_l)),
                    Arc::new(Float32Array::from(m_11_r)),
                    Arc::new(Float32Array::from(thermal_accumulated)),
                    Arc::new(Float32Array::from(mu_friction)),
                    Arc::new(Float32Array::from(mu_est)),
                    Arc::new(Float32Array::from(slip_velocity_l)),
                    Arc::new(Float32Array::from(slip_velocity_r)),
                    Arc::new(Float32Array::from(com_pos_x)),
                    Arc::new(Float32Array::from(com_pos_z)),
                    Arc::new(Float32Array::from(com_vel_x)),
                    Arc::new(Float32Array::from(com_vel_z)),
                    Arc::new(Float32Array::from(pitch_rad)),
                    Arc::new(Float32Array::from(pitch_vel)),
                    Arc::new(StringArray::from(scenario_vec)),
                    Arc::new(StringArray::from(sha256_seal)),
                    Arc::new(StringArray::from(trajectory_id)),
                    Arc::new(Float32Array::from(gear_wear_factor)),
                    Arc::new(Float32Array::from(backlash_limit)),
                    Arc::new(Float32Array::from(tendon_elongation_l_mm)),
                    Arc::new(Float32Array::from(tendon_elongation_r_mm)),
                    Arc::new(Float32Array::from(payload_slosh_displacement_m)),
                    Arc::new(Float32Array::from(payload_mass_kg)),
                    Arc::new(Float32Array::from(battery_cell_temp_c)),
                    Arc::new(BooleanArray::from(bms_trip)),
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
