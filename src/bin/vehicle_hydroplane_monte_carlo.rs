// Category 2: Autonomous Vehicles & Heavy Logistics (Tire-Terrain Dynamics)
// 1000Hz Euler Integration of chassis dynamics under Pacejka magic formula tire-slip.
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

const M_CHASSIS: f32 = 2000.0; // kg
const G: f32 = 9.81;
const I_YAW: f32 = 3000.0;     // kg*m^2
const A_FRONT: f32 = 1.4;      // m - CoG to front axle
const B_REAR: f32 = 1.4;       // m - CoG to rear axle
const TRACK_W: f32 = 1.6;      // m - track width
const H_CG: f32 = 0.6;         // m - center of gravity height
const R_WHEEL: f32 = 0.35;     // m - wheel radius
const I_WHEEL: f32 = 2.0;      // kg*m^2 - wheel rotational inertia

// 32-Dimensional state struct, size assertion = exactly 128 bytes
#[allow(dead_code)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Serialize)]
struct VehicleDynamicsState {
    timestamp: f32,                // 1. Time (4 bytes)
    chassis_q: [f32; 6],           // 2-7. Chassis coordinates: [x, y, z, roll, pitch, yaw] (24 bytes)
    chassis_dq: [f32; 6],          // 8-13. Chassis velocities: [vx, vy, vz, roll_rate, pitch_rate, yaw_rate] (24 bytes)
    wheel_q: [f32; 4],             // 14-17. Wheel angular positions: [fl, fr, rl, rr] (16 bytes)
    wheel_dq: [f32; 4],            // 18-21. Wheel angular velocities: [fl, fr, rl, rr] (16 bytes)
    wheel_torques: [f32; 4],       // 22-25. Engine/braking torques: [fl, fr, rl, rr] (16 bytes)
    pacejka_jacobians: [f32; 4],   // 26-29. Pacejka tire slip-force Jacobians: [fl, fr, rl, rr] (16 bytes)
    normal_forces: [f32; 2],       // 30-31. Front/Rear vertical loads: [front, rear] (8 bytes)
    thermal_accumulated: f32,      // 32. Drivetrain thermal tax: integrated torque squared (4 bytes)
}

// Compile-time assertion of exactly 128 bytes for L1/L2 cache line alignment
const _: () = assert!(std::mem::size_of::<VehicleDynamicsState>() == 128);

#[derive(Serialize)]
struct VehicleStepState {
    timestamp: f32,
    chassis_q_x: f32,
    chassis_q_y: f32,
    chassis_q_z: f32,
    chassis_q_roll: f32,
    chassis_q_pitch: f32,
    chassis_q_yaw: f32,
    chassis_dq_x: f32,
    chassis_dq_y: f32,
    chassis_dq_z: f32,
    chassis_dq_roll_rate: f32,
    chassis_dq_pitch_rate: f32,
    chassis_dq_yaw_rate: f32,
    wheel_q_fl: f32,
    wheel_q_fr: f32,
    wheel_q_rl: f32,
    wheel_q_rr: f32,
    wheel_dq_fl: f32,
    wheel_dq_fr: f32,
    wheel_dq_rl: f32,
    wheel_dq_rr: f32,
    wheel_torque_fl: f32,
    wheel_torque_fr: f32,
    wheel_torque_rl: f32,
    wheel_torque_rr: f32,
    pacejka_jacobian_fl: f32,
    pacejka_jacobian_fr: f32,
    pacejka_jacobian_rl: f32,
    pacejka_jacobian_rr: f32,
    normal_load_front: f32,
    normal_load_rear: f32,
    thermal_accumulated: f32,
    mu_friction: f32,
    terrain_moisture: f32,
    x_water: f32,
    gg_ekf_divergence: f32,
    fy_fl: f32,
    fy_fr: f32,
    fy_rl: f32,
    fy_rr: f32,
    fx_fl: f32,
    fx_fr: f32,
    fx_rl: f32,
    fx_rr: f32,
    steer_target: f32,
    steer_actual: f32,
    fluid_temp_c: f32,
    actual_viscosity_cst: f32,
    brake_fluid_temp_c: f32,
    d_roll: f32,
    d_pitch: f32,
    scenario: String,
    sha256_seal: String,
}

#[derive(Serialize)]
struct VehicleTrajectory {
    trajectory_id: String,
    data: Vec<VehicleStepState>,
    proof_hash: String,
    ekf_drift_failed: bool,
}

// Pacejka Magic Formula for lateral tire force
fn pacejka_lateral_force(alpha: f32, fz: f32, mu: f32) -> f32 {
    let b = 10.0f32;
    let c = 1.3f32;
    let d = mu * fz;
    let e = -1.0f32;
    d * (c * (b * alpha - e * (b * alpha - (b * alpha).atan()))).atan().sin()
}

// Pacejka Magic Formula for longitudinal tire force
fn pacejka_longitudinal_force(kappa: f32, fz: f32, mu: f32) -> f32 {
    let b = 12.0f32;
    let c = 1.6f32;
    let d = mu * fz;
    let e = -0.5f32;
    d * (c * (b * kappa - e * (b * kappa - (b * kappa).atan()))).atan().sin()
}

fn run_single_trajectory(index: usize, seed: u64, scenario: &str) -> VehicleTrajectory {
    let mut rng = Rng::new(seed);
    
    // Dynamic mass and moments of inertia
    let (m_chassis, i_yaw) = match scenario {
        "loaded" => (4500.0f32, I_YAW * (4500.0 / M_CHASSIS)),
        _ => (M_CHASSIS, I_YAW),
    };

    // Initial condition variations
    let v_start = rng.range(40.0, 44.0) as f32; // speed m/s (~94mph)
    let cornering_radius = rng.range(480.0, 520.0) as f32; // cornering radius meters
    let ekf_steer = ((A_FRONT + B_REAR) / cornering_radius).atan(); // kinematic steer angle
    let steer_angle = ekf_steer + 0.015f32; // slip compensation on dry road
    
    // Initial states
    let mut pos_x = 0.0f32;
    let mut pos_y = 0.0f32;
    let mut yaw = 0.0f32;
    
    let mut vel_x = v_start;
    let mut vel_y = 0.0f32;
    let mut yaw_rate = v_start / cornering_radius;
    
    // Wheel states: initially rolling without slip
    let mut wheel_dq = [v_start / R_WHEEL; 4];
    let mut wheel_q = [0.0f32; 4];
    
    // Suspensional roll/pitch states
    let mut roll = 0.0f32;
    let mut roll_rate = 0.0f32;
    let mut pitch = 0.0f32;
    let mut pitch_rate = 0.0f32;
    
    // EKF estimator state (blind to lateral slip, assumes perfect pavement friction)
    let mut ekf_x = 0.0f32;
    let mut ekf_y = 0.0f32;
    let mut ekf_yaw = 0.0f32;
    let mut ekf_v = v_start;
    let mut ekf_p = [0.0f32; 16];
    ekf_p[0] = 0.1; ekf_p[5] = 0.1; ekf_p[10] = 0.1; ekf_p[15] = 0.1;
    let x_water = rng.range(4.0, 12.0) as f32;
    let water_transition_len = rng.range(0.1, 0.5) as f32;
    
    let dt = 0.001f32; // 1000Hz (1.0ms steps)
    let total_time = 5.0f32; // 5.0 second simulation
    let steps_count = (total_time / dt) as usize;
    
    let mut states = Vec::with_capacity(steps_count / 10);
    
    // Cryptographic running hash chain
    let mut running_hash = Sha256::new();
    running_hash.update(&seed.to_le_bytes());
    let mut last_hash = running_hash.finalize();
    
    let fluid_temp_c = if scenario == "arctic_cold_start" {
        rng.range(-40.0, 0.0) as f32
    } else {
        20.0f32
    };
    let nominal_viscosity_cst = 14.0f32;
    let temp_exponent = (-fluid_temp_c + 20.0f32) * 0.08f32;
    let actual_viscosity_cst = nominal_viscosity_cst * temp_exponent.exp();
    let mut steer_actual = 0.0f32;
    let mut rotor_temp_c = 20.0f32;
    let mut brake_fluid_temp_c = 20.0f32;

    let mut max_ekf_drift = 0.0f32;
    let mut thermal_tax = 0.0f32;
    let mut consecutive_fail_steps = 0;
    
    let mut prev_vel_x = v_start;
    let mut lon_accel_filtered = 0.0f32;
    
    for step in 0..steps_count {
        let t = step as f32 * dt;
        let steer_target = if t < 1.0f32 { (t / 1.0f32) * steer_angle } else { steer_angle };
        let steer_error = steer_target - steer_actual;
        let k_steer = 15.0f32;
        let damping = 1.0f32 + (actual_viscosity_cst - nominal_viscosity_cst) * 0.005f32;
        let stiction_threshold = if actual_viscosity_cst > 100.0f32 {
            0.005f32 * (actual_viscosity_cst / 100.0f32)
        } else {
            0.0f32
        };
        let mut steer_rate = steer_error * k_steer;
        if steer_rate.abs() < stiction_threshold {
            steer_rate = 0.0f32;
        } else {
            steer_rate = (steer_rate - stiction_threshold * steer_rate.signum()) / damping;
        }
        steer_actual += steer_rate * dt;

        // Hitting water patch transition (randomized entry coordinate x_water)
        let terrain_moisture = if pos_x >= x_water {
            // Smooth hydroplaning entry over a randomized transition distance
            let entry_dist = pos_x - x_water;
            (entry_dist / water_transition_len).min(1.0f32)
        } else {
            0.0f32
        };
        
        // Ground truth road friction sweeps down on wet entry
        let (mu_dry, mu_wet) = match scenario {
            "ice" => (0.15f32, 0.05f32),
            "mud" => (0.35f32, 0.10f32),
            "nominal" => (rng.range(0.80, 0.90) as f32, 0.15f32),
            "loaded" => (rng.range(0.30, 0.45) as f32, 0.08f32),
            _ => (0.85f32, 0.15f32),
        };
        let mu_wet_dynamic = mu_wet / (1.0f32 + 0.0005f32 * vel_x * vel_x);
        let mu_actual = mu_dry - (mu_dry - mu_wet_dynamic) * terrain_moisture;
        
        // Dynamic Weight Transfer (Lateral and Longitudinal)
        let lat_accel = vel_x * yaw_rate;
        let lon_accel = (vel_x - prev_vel_x) / dt;
        prev_vel_x = vel_x;
        lon_accel_filtered = 0.95f32 * lon_accel_filtered + 0.05f32 * lon_accel;
        
        let delta_fz_lat = m_chassis * lat_accel * H_CG / TRACK_W;
        let delta_fz_lon = m_chassis * lon_accel_filtered * H_CG / (A_FRONT + B_REAR);
        
        // Dynamic load calculation for FL, FR, RL, RR
        let fz_static_front = 0.25f32 * m_chassis * G;
        let fz_static_rear = 0.25f32 * m_chassis * G;
        let fz_fl = (fz_static_front - 0.5f32 * delta_fz_lat + 0.5f32 * delta_fz_lon).max(100.0f32);
        let fz_fr = (fz_static_front + 0.5f32 * delta_fz_lat + 0.5f32 * delta_fz_lon).max(100.0f32);
        let fz_rl = (fz_static_rear - 0.5f32 * delta_fz_lat - 0.5f32 * delta_fz_lon).max(100.0f32);
        let fz_rr = (fz_static_rear + 0.5f32 * delta_fz_lat - 0.5f32 * delta_fz_lon).max(100.0f32);
        
        let normal_load_front = fz_fl + fz_fr;
        let normal_load_rear = fz_rl + fz_rr;
        
        // Slip angles and longitudinal velocities (lateral/longitudinal kinematics including Ackermann & Track-width)
        let vel_x_fl = vel_x + yaw_rate * (TRACK_W * 0.5f32);
        let vel_x_fr = vel_x - yaw_rate * (TRACK_W * 0.5f32);
        let vel_x_rl = vel_x + yaw_rate * (TRACK_W * 0.5f32);
        let vel_x_rr = vel_x - yaw_rate * (TRACK_W * 0.5f32);
        
        let steer_fl = steer_actual + 0.05f32 * steer_actual * steer_actual.signum();
        let steer_fr = steer_actual - 0.05f32 * steer_actual * steer_actual.signum();
        
        let wheel_slip_angle_fl = steer_fl - ((vel_y + A_FRONT * yaw_rate) / vel_x_fl.max(0.1f32)).atan();
        let wheel_slip_angle_fr = steer_fr - ((vel_y + A_FRONT * yaw_rate) / vel_x_fr.max(0.1f32)).atan();
        let wheel_slip_angle_rl = -((vel_y - B_REAR * yaw_rate) / vel_x_rl.max(0.1f32)).atan();
        let wheel_slip_angle_rr = -((vel_y - B_REAR * yaw_rate) / vel_x_rr.max(0.1f32)).atan();
        
        // Longitudinal slip ratios
        let kappa_fl = (wheel_dq[0] * R_WHEEL - vel_x_fl) / vel_x_fl.max(0.1f32);
        let kappa_fr = (wheel_dq[1] * R_WHEEL - vel_x_fr) / vel_x_fr.max(0.1f32);
        let kappa_rl = (wheel_dq[2] * R_WHEEL - vel_x_rl) / vel_x_rl.max(0.1f32);
        let kappa_rr = (wheel_dq[3] * R_WHEEL - vel_x_rr) / vel_x_rr.max(0.1f32);
        
        // Tire lateral and longitudinal forces
        let f_y_fl = pacejka_lateral_force(wheel_slip_angle_fl, fz_fl, mu_actual);
        let f_y_fr = pacejka_lateral_force(wheel_slip_angle_fr, fz_fr, mu_actual);
        let f_y_rl = pacejka_lateral_force(wheel_slip_angle_rl, fz_rl, mu_actual);
        let f_y_rr = pacejka_lateral_force(wheel_slip_angle_rr, fz_rr, mu_actual);
        
        let f_x_fl = pacejka_longitudinal_force(kappa_fl, fz_fl, mu_actual);
        let f_x_fr = pacejka_longitudinal_force(kappa_fr, fz_fr, mu_actual);
        let f_x_rl = pacejka_longitudinal_force(kappa_rl, fz_rl, mu_actual);
        let f_x_rr = pacejka_longitudinal_force(kappa_rr, fz_rr, mu_actual);
        
        // Pacejka tire slip-force Jacobians
        let jacobian_fl = (pacejka_longitudinal_force(kappa_fl + 0.001f32, fz_fl, mu_actual) - f_x_fl) / 0.001f32;
        let jacobian_fr = (pacejka_longitudinal_force(kappa_fr + 0.001f32, fz_fr, mu_actual) - f_x_fr) / 0.001f32;
        let jacobian_rl = (pacejka_longitudinal_force(kappa_rl + 0.001f32, fz_rl, mu_actual) - f_x_rl) / 0.001f32;
        let jacobian_rr = (pacejka_longitudinal_force(kappa_rr + 0.001f32, fz_rr, mu_actual) - f_x_rr) / 0.001f32;
        
        // Aerodynamic drag
        let f_drag = 0.5f32 * 1.225f32 * 0.3f32 * 2.2f32 * vel_x * vel_x;
        
        // Driver throttle command to maintain speed
        let kp_speed = 50.0f32;
        let drive_cmd = (kp_speed * (v_start - vel_x)).clamp(0.0f32, 800.0f32);
        let drive_torque_nominal = drive_cmd * 0.25f32;
        
        let mut drive_fl = drive_torque_nominal;
        let mut drive_fr = drive_torque_nominal;
        let mut drive_rl = drive_torque_nominal;
        let mut drive_rr = drive_torque_nominal;
        
        // Active WBC ESC Anti-Hydroplane stability logic
        
        // Track history of slips for differentiator
        let slip_fl = (wheel_dq[0] - vel_x / R_WHEEL).max(0.0f32);
        let slip_fr = (wheel_dq[1] - vel_x / R_WHEEL).max(0.0f32);
        let slip_rl = (wheel_dq[2] - vel_x / R_WHEEL).max(0.0f32);
        let slip_rr = (wheel_dq[3] - vel_x / R_WHEEL).max(0.0f32);
        
        let target_spin = 1.5f32; // Allow minor spin-ratio
        let fl_over = slip_fl > target_spin;
        let fr_over = slip_fr > target_spin;
        let rl_over = slip_rl > target_spin;
        let rr_over = slip_rr > target_spin;
        
        // Active brake torque modulation: apply heavy braking to spinning wheels
        let mut brake_fl = 0.0f32;
        let mut brake_fr = 0.0f32;
        let mut brake_rl = 0.0f32;
        let mut brake_rr = 0.0f32;
        
        if fl_over { brake_fl = 300.0f32; drive_fl = 0.0f32; }
        if fr_over { brake_fr = 300.0f32; drive_fr = 0.0f32; }
        if rl_over { brake_rl = 300.0f32; drive_rl = 0.0f32; }
        if rr_over { brake_rr = 300.0f32; drive_rr = 0.0f32; }
        
        // Active Yaw Stability: if yaw rate deviates from EKF target, apply differential braking
        let yaw_target = vel_x * steer_target / (A_FRONT + B_REAR);
        let yaw_error = yaw_rate - yaw_target;
        if yaw_error.abs() > 0.15f32 {
            if yaw_error > 0.0f32 {
                // Oversteer turning left: brake front outer (FR) to counter
                brake_fr += 400.0f32;
                drive_fr = 0.0f32;
            } else {
                // Understeer/spin-out: brake front inner (FL)
                brake_fl += 400.0f32;
                drive_fl = 0.0f32;
            }
        }
        
        let torque_fl = drive_fl - brake_fl;
        let torque_fr = drive_fr - brake_fr;
        let torque_rl = drive_rl - brake_rl;
        let torque_rr = drive_rr - brake_rr;
        let wheel_torques = [torque_fl, torque_fr, torque_rl, torque_rr];
        
        // Wheel dynamics integration
        for i in 0..4 {
            let f_x = match i {
                0 => f_x_fl,
                1 => f_x_fr,
                2 => f_x_rl,
                _ => f_x_rr,
            };
            wheel_dq[i] += ((wheel_torques[i] - f_x * R_WHEEL) / I_WHEEL) * dt;
            wheel_dq[i] = wheel_dq[i].max(0.0f32);
            wheel_q[i] += wheel_dq[i] * dt;
        }
        
        // Thermal accumulated
        let torque_sq = torque_fl.powi(2) + torque_fr.powi(2) + torque_rl.powi(2) + torque_rr.powi(2);
        thermal_tax += (torque_sq / 1000.0f32) * dt;

        // Brake fluid temperature conduction model
        let braking_power = (brake_fl * wheel_dq[0].abs() + brake_fr * wheel_dq[1].abs() + brake_rl * wheel_dq[2].abs() + brake_rr * wheel_dq[3].abs()) * R_WHEEL;
        rotor_temp_c += (braking_power / 15000.0f32) * dt;
        let conduction = (rotor_temp_c - brake_fluid_temp_c) * 100.0f32;
        brake_fluid_temp_c += (conduction / 2000.0f32) * dt;
        // cooling
        rotor_temp_c -= (rotor_temp_c - 20.0f32) * 2.0f32 * dt;
        brake_fluid_temp_c -= (brake_fluid_temp_c - 20.0f32) * 0.5f32 * dt;

        let damper_fade_factor = if brake_fluid_temp_c > 120.0f32 {
            (1.0f32 - 0.70f32 * ((brake_fluid_temp_c - 120.0f32) / 80.0f32).min(1.0f32))
        } else {
            1.0f32
        };
        let d_roll_dynamic = 1500.0f32 * damper_fade_factor;
        let d_pitch_dynamic = 1800.0f32 * damper_fade_factor;
        
        // Chassis equations
        let f_x_total = (f_x_fl + f_x_fr) * steer_actual.cos() - (f_y_fl + f_y_fr) * steer_actual.sin() + f_x_rl + f_x_rr - f_drag;
        let f_y_total = (f_x_fl + f_x_fr) * steer_actual.sin() + (f_y_fl + f_y_fr) * steer_actual.cos() + f_y_rl + f_y_rr;
        let torque_yaw = A_FRONT * ((f_x_fl + f_x_fr) * steer_actual.sin() + (f_y_fl + f_y_fr) * steer_actual.cos()) 
            - B_REAR * (f_y_rl + f_y_rr) 
            + (TRACK_W * 0.5f32) * ((f_x_fr - f_x_fl) * steer_actual.cos() - (f_y_fr - f_y_fl) * steer_actual.sin() + (f_x_rr - f_x_rl));
            
        let acc_x = f_x_total / m_chassis;
        let acc_y = f_y_total / m_chassis;
        let yaw_accel = torque_yaw / i_yaw;
        
        // Kinematics
        vel_x += (acc_x + vel_y * yaw_rate) * dt;
        vel_y += (acc_y - vel_x * yaw_rate) * dt;
        yaw_rate += yaw_accel * dt;
        yaw += yaw_rate * dt;
        
        pos_x += (vel_x * yaw.cos() - vel_y * yaw.sin()) * dt;
        pos_y += (vel_x * yaw.sin() + vel_y * yaw.cos()) * dt;
        
        // Suspension dynamics (roll & pitch)
        let k_roll = 25000.0f32; let d_roll = d_roll_dynamic;
        let k_pitch = 30000.0f32; let d_pitch = d_pitch_dynamic;
        let roll_accel = (f_y_total * H_CG - k_roll * roll - d_roll * roll_rate) / 1000.0f32;
        let pitch_accel = (f_x_total * H_CG - k_pitch * pitch - d_pitch * pitch_rate) / 1000.0f32;
        
        roll_rate += roll_accel * dt;
        roll += roll_rate * dt;
        pitch_rate += pitch_accel * dt;
        pitch += pitch_rate * dt;
        let pos_z = H_CG - 0.25f32 * (roll.abs() + pitch.abs());
        
        // True 4-State Kinematic EKF Prediction & Update (blind to lateral slip)
        let cos_yaw = ekf_yaw.cos();
        let sin_yaw = ekf_yaw.sin();
        let tan_steer = ekf_steer.tan();
        let l_wheelbase = A_FRONT + B_REAR;
        
        let next_ekf_x = ekf_x + ekf_v * cos_yaw * dt;
        let next_ekf_y = ekf_y + ekf_v * sin_yaw * dt;
        let next_ekf_yaw = ekf_yaw + (ekf_v / l_wheelbase) * tan_steer * dt;
        let next_ekf_v = ekf_v + lon_accel_filtered * dt;
        
        // Jacobian F (4x4)
        let mut f_mat = [0.0f32; 16];
        f_mat[0] = 1.0; f_mat[2] = -ekf_v * sin_yaw * dt; f_mat[3] = cos_yaw * dt;
        f_mat[5] = 1.0; f_mat[6] = ekf_v * cos_yaw * dt; f_mat[7] = sin_yaw * dt;
        f_mat[10] = 1.0; f_mat[11] = (tan_steer / l_wheelbase) * dt;
        f_mat[15] = 1.0;
        
        // Covariance Prediction: P = F * P * F^T + Q
        let mut fp = [0.0f32; 16];
        for r in 0..4 {
            for c in 0..4 {
                let mut sum = 0.0f32;
                for k in 0..4 {
                    sum += f_mat[r*4 + k] * ekf_p[k*4 + c];
                }
                fp[r*4 + c] = sum;
            }
        }
        let mut p_next = [0.0f32; 16];
        for r in 0..4 {
            for c in 0..4 {
                let mut sum = 0.0f32;
                for k in 0..4 {
                    sum += fp[r*4 + k] * f_mat[c*4 + k];
                }
                p_next[r*4 + c] = sum;
            }
        }
        // Add process noise
        p_next[0] += 0.001; p_next[5] += 0.001; p_next[10] += 0.001; p_next[15] += 0.001;
        
        // Measurement updates: GPS (x,y) and speedometer (v) with noise
        let z_x = pos_x + rng.range(-0.05, 0.05) as f32;
        let z_y = pos_y + rng.range(-0.05, 0.05) as f32;
        let z_v = vel_x + rng.range(-0.1, 0.1) as f32;
        
        let inn_x = z_x - next_ekf_x;
        let inn_y = z_y - next_ekf_y;
        let inn_v = z_v - next_ekf_v;
        
        // Measurement covariance S = H * P * H^T + R
        let s00 = p_next[0] + 0.01;  let s01 = p_next[1];        let s02 = p_next[3];
        let s10 = p_next[4];         let s11 = p_next[5] + 0.01;  let s12 = p_next[7];
        let s20 = p_next[12];        let s21 = p_next[13];       let s22 = p_next[15] + 0.01;
        
        let det_s = s00 * (s11 * s22 - s12 * s21)
                  - s01 * (s10 * s22 - s12 * s20)
                  + s02 * (s10 * s21 - s11 * s20);
                  
        if det_s.abs() > 1e-6 {
            let inv_det_s = 1.0 / det_s;
            let inv_s00 = (s11 * s22 - s12 * s21) * inv_det_s;
            let inv_s01 = -(s01 * s22 - s02 * s21) * inv_det_s;
            let inv_s02 = (s01 * s12 - s02 * s11) * inv_det_s;
            
            let inv_s10 = -(s10 * s22 - s12 * s20) * inv_det_s;
            let inv_s11 = (s00 * s22 - s02 * s20) * inv_det_s;
            let inv_s12 = -(s00 * s12 - s02 * s10) * inv_det_s;
            
            let inv_s20 = (s10 * s21 - s11 * s20) * inv_det_s;
            let inv_s21 = -(s00 * s21 - s01 * s20) * inv_det_s;
            let inv_s22 = (s00 * s11 - s01 * s10) * inv_det_s;
            
            // Kalman Gain K = P * H^T * S^-1 (4x3)
            let mut k_mat = [0.0f32; 12];
            for r in 0..4 {
                let p0 = p_next[r*4 + 0];
                let p1 = p_next[r*4 + 1];
                let p2 = p_next[r*4 + 3];
                k_mat[r*3 + 0] = p0 * inv_s00 + p1 * inv_s10 + p2 * inv_s20;
                k_mat[r*3 + 1] = p0 * inv_s01 + p1 * inv_s11 + p2 * inv_s21;
                k_mat[r*3 + 2] = p0 * inv_s02 + p1 * inv_s12 + p2 * inv_s22;
            }
            
            // State Update
            ekf_x = next_ekf_x + k_mat[0] * inn_x + k_mat[1] * inn_y + k_mat[2] * inn_v;
            ekf_y = next_ekf_y + k_mat[3] * inn_x + k_mat[4] * inn_y + k_mat[5] * inn_v;
            ekf_yaw = next_ekf_yaw + k_mat[6] * inn_x + k_mat[7] * inn_y + k_mat[8] * inn_v;
            ekf_v = next_ekf_v + k_mat[9] * inn_x + k_mat[10] * inn_y + k_mat[11] * inn_v;
            
            // Covariance Update P = (I - K * H) * P
            let mut khp = [0.0f32; 16];
            for r in 0..4 {
                let k0 = k_mat[r*3 + 0];
                let k1 = k_mat[r*3 + 1];
                let k2 = k_mat[r*3 + 2];
                for c in 0..4 {
                    khp[r*4 + c] = k0 * p_next[0*4 + c] + k1 * p_next[1*4 + c] + k2 * p_next[3*4 + c];
                }
            }
            for i in 0..16 {
                ekf_p[i] = p_next[i] - khp[i];
            }
        } else {
            ekf_x = next_ekf_x;
            ekf_y = next_ekf_y;
            ekf_yaw = next_ekf_yaw;
            ekf_v = next_ekf_v;
            ekf_p = p_next;
        }
        
        let gg_ekf_divergence = ((pos_x - ekf_x).powi(2) + (pos_y - ekf_y).powi(2)).sqrt();
        if gg_ekf_divergence > max_ekf_drift {
            max_ekf_drift = gg_ekf_divergence;
        }
        
        if gg_ekf_divergence > 1.25f32 {
            consecutive_fail_steps += 1;
        } else {
            consecutive_fail_steps = 0;
        }
        
        let is_logging_step = step % 10 == 0;
        let is_terminal_step = step == steps_count - 1 || consecutive_fail_steps >= 50;

        if is_logging_step || is_terminal_step {
            // Cryptographic running hash chain update
            let mut hasher = Sha256::new();
            hasher.update(&last_hash);
            hasher.update(&t.to_le_bytes());
            hasher.update(&pos_x.to_le_bytes());
            hasher.update(&pos_y.to_le_bytes());
            hasher.update(&pos_z.to_le_bytes());
            hasher.update(&roll.to_le_bytes());
            hasher.update(&pitch.to_le_bytes());
            hasher.update(&yaw.to_le_bytes());
            hasher.update(&vel_x.to_le_bytes());
            hasher.update(&vel_y.to_le_bytes());
            hasher.update(&0.0f32.to_le_bytes()); // placeholder/vz is 0
            hasher.update(&roll_rate.to_le_bytes());
            hasher.update(&pitch_rate.to_le_bytes());
            hasher.update(&yaw_rate.to_le_bytes());
            hasher.update(&wheel_q[0].to_le_bytes());
            hasher.update(&wheel_q[1].to_le_bytes());
            hasher.update(&wheel_q[2].to_le_bytes());
            hasher.update(&wheel_q[3].to_le_bytes());
            hasher.update(&wheel_dq[0].to_le_bytes());
            hasher.update(&wheel_dq[1].to_le_bytes());
            hasher.update(&wheel_dq[2].to_le_bytes());
            hasher.update(&wheel_dq[3].to_le_bytes());
            hasher.update(&torque_fl.to_le_bytes());
            hasher.update(&torque_fr.to_le_bytes());
            hasher.update(&torque_rl.to_le_bytes());
            hasher.update(&torque_rr.to_le_bytes());
            hasher.update(&jacobian_fl.to_le_bytes());
            hasher.update(&jacobian_fr.to_le_bytes());
            hasher.update(&jacobian_rl.to_le_bytes());
            hasher.update(&jacobian_rr.to_le_bytes());
            hasher.update(&normal_load_front.to_le_bytes());
            hasher.update(&normal_load_rear.to_le_bytes());
            hasher.update(&thermal_tax.to_le_bytes());
            hasher.update(&x_water.to_le_bytes());
            hasher.update(&f_y_fl.to_le_bytes());
            hasher.update(&f_y_fr.to_le_bytes());
            hasher.update(&f_y_rl.to_le_bytes());
            hasher.update(&f_y_rr.to_le_bytes());
            hasher.update(&f_x_fl.to_le_bytes());
            hasher.update(&f_x_fr.to_le_bytes());
            hasher.update(&f_x_rl.to_le_bytes());
            hasher.update(&f_x_rr.to_le_bytes());
            hasher.update(&steer_target.to_le_bytes());
            hasher.update(&steer_actual.to_le_bytes());
            hasher.update(&fluid_temp_c.to_le_bytes());
            hasher.update(&actual_viscosity_cst.to_le_bytes());
            hasher.update(&brake_fluid_temp_c.to_le_bytes());
            hasher.update(&d_roll_dynamic.to_le_bytes());
            hasher.update(&d_pitch_dynamic.to_le_bytes());
            
            last_hash = hasher.finalize();
            let sha256_seal = hex::encode(last_hash);

            states.push(VehicleStepState {
                timestamp: t,
                chassis_q_x: pos_x,
                chassis_q_y: pos_y,
                chassis_q_z: pos_z,
                chassis_q_roll: roll,
                chassis_q_pitch: pitch,
                chassis_q_yaw: yaw,
                chassis_dq_x: vel_x,
                chassis_dq_y: vel_y,
                chassis_dq_z: 0.0f32,
                chassis_dq_roll_rate: roll_rate,
                chassis_dq_pitch_rate: pitch_rate,
                chassis_dq_yaw_rate: yaw_rate,
                wheel_q_fl: wheel_q[0],
                wheel_q_fr: wheel_q[1],
                wheel_q_rl: wheel_q[2],
                wheel_q_rr: wheel_q[3],
                wheel_dq_fl: wheel_dq[0],
                wheel_dq_fr: wheel_dq[1],
                wheel_dq_rl: wheel_dq[2],
                wheel_dq_rr: wheel_dq[3],
                wheel_torque_fl: torque_fl,
                wheel_torque_fr: torque_fr,
                wheel_torque_rl: torque_rl,
                wheel_torque_rr: torque_rr,
                pacejka_jacobian_fl: jacobian_fl,
                pacejka_jacobian_fr: jacobian_fr,
                pacejka_jacobian_rl: jacobian_rl,
                pacejka_jacobian_rr: jacobian_rr,
                normal_load_front,
                normal_load_rear,
                thermal_accumulated: thermal_tax,
                mu_friction: mu_actual,
                terrain_moisture,
                x_water,
                gg_ekf_divergence,
                fy_fl: f_y_fl,
                fy_fr: f_y_fr,
                fy_rl: f_y_rl,
                fy_rr: f_y_rr,
                fx_fl: f_x_fl,
                fx_fr: f_x_fr,
                fx_rl: f_x_rl,
                fx_rr: f_x_rr,
                steer_target,
                steer_actual,
                fluid_temp_c,
                actual_viscosity_cst,
                brake_fluid_temp_c,
                d_roll: d_roll_dynamic,
                d_pitch: d_pitch_dynamic,
                scenario: scenario.to_string(),
                sha256_seal,
            });
        }
        
        if consecutive_fail_steps >= 50 {
            break;
        }
    }
    
    let proof_hash = hex::encode(last_hash);
    let ekf_drift_failed = max_ekf_drift > 1.25f32;
    
    VehicleTrajectory {
        trajectory_id: format!("pg_veh_{:05x}", index),
        data: states,
        proof_hash,
        ekf_drift_failed,
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
        .unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/products/autonomous_vehicles_heavy_logistics.parquet").to_string());

    let scenario = args.iter().position(|a| a == "--scenario")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "nominal".to_string());
        
    eprintln!("Generating {} Vehicle trajectories to Parquet...", n_trajectories);
    let start = Instant::now();

    // Define Arrow schema
    let schema = Arc::new(Schema::new(vec![
        Field::new("timestamp", DataType::Float32, false),
        Field::new("chassis_q_x", DataType::Float32, false),
        Field::new("chassis_q_y", DataType::Float32, false),
        Field::new("chassis_q_z", DataType::Float32, false),
        Field::new("chassis_q_roll", DataType::Float32, false),
        Field::new("chassis_q_pitch", DataType::Float32, false),
        Field::new("chassis_q_yaw", DataType::Float32, false),
        Field::new("chassis_dq_x", DataType::Float32, false),
        Field::new("chassis_dq_y", DataType::Float32, false),
        Field::new("chassis_dq_z", DataType::Float32, false),
        Field::new("chassis_dq_roll_rate", DataType::Float32, false),
        Field::new("chassis_dq_pitch_rate", DataType::Float32, false),
        Field::new("chassis_dq_yaw_rate", DataType::Float32, false),
        Field::new("wheel_q_fl", DataType::Float32, false),
        Field::new("wheel_q_fr", DataType::Float32, false),
        Field::new("wheel_q_rl", DataType::Float32, false),
        Field::new("wheel_q_rr", DataType::Float32, false),
        Field::new("wheel_dq_fl", DataType::Float32, false),
        Field::new("wheel_dq_fr", DataType::Float32, false),
        Field::new("wheel_dq_rl", DataType::Float32, false),
        Field::new("wheel_dq_rr", DataType::Float32, false),
        Field::new("wheel_torque_fl", DataType::Float32, false),
        Field::new("wheel_torque_fr", DataType::Float32, false),
        Field::new("wheel_torque_rl", DataType::Float32, false),
        Field::new("wheel_torque_rr", DataType::Float32, false),
        Field::new("pacejka_jacobian_fl", DataType::Float32, false),
        Field::new("pacejka_jacobian_fr", DataType::Float32, false),
        Field::new("pacejka_jacobian_rl", DataType::Float32, false),
        Field::new("pacejka_jacobian_rr", DataType::Float32, false),
        Field::new("normal_load_front", DataType::Float32, false),
        Field::new("normal_load_rear", DataType::Float32, false),
        Field::new("thermal_accumulated", DataType::Float32, false),
        Field::new("mu_friction", DataType::Float32, false),
        Field::new("terrain_moisture", DataType::Float32, false),
        Field::new("x_water", DataType::Float32, false),
        Field::new("gg_ekf_divergence", DataType::Float32, false),
        Field::new("fy_fl", DataType::Float32, false),
        Field::new("fy_fr", DataType::Float32, false),
        Field::new("fy_rl", DataType::Float32, false),
        Field::new("fy_rr", DataType::Float32, false),
        Field::new("fx_fl", DataType::Float32, false),
        Field::new("fx_fr", DataType::Float32, false),
        Field::new("fx_rl", DataType::Float32, false),
        Field::new("fx_rr", DataType::Float32, false),
        Field::new("steer_target", DataType::Float32, false),
        Field::new("steer_actual", DataType::Float32, false),
        Field::new("fluid_temp_c", DataType::Float32, false),
        Field::new("actual_viscosity_cst", DataType::Float32, false),
        Field::new("brake_fluid_temp_c", DataType::Float32, false),
        Field::new("d_roll", DataType::Float32, false),
        Field::new("d_pitch", DataType::Float32, false),
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
    let base_seed = 0xDECD_BAFC_A0FE_1337u64;
    let seed_multiplier = 0x9E37_79B1_85EB_CA87u64;

    // Chunk size to prevent OOM
    let chunk_size = 2000;
    let mut written_count = 0;
    let mut total_rows = 0;

    while written_count < n_trajectories {
        let this_chunk_size = std::cmp::min(chunk_size, n_trajectories - written_count);
        let start_i = written_count;
        let end_i = start_i + this_chunk_size;
        
        let trajectories: Vec<VehicleTrajectory> = (start_i..end_i)
            .into_par_iter()
            .map(|i| {
                let seed = base_seed ^ (i as u64).wrapping_mul(seed_multiplier);
                let scenario_for_traj = if scenario == "sweep" {
                    match i % 5 {
                        0 => "nominal",
                        1 => "loaded",
                        2 => "ice",
                        3 => "mud",
                        _ => "arctic_cold_start",
                    }
                } else {
                    &scenario
                };
                run_single_trajectory(i, seed, scenario_for_traj)
            })
            .collect();
            
        // Columnar buffers for RecordBatch
        let mut timestamp = Vec::new();
        let mut chassis_q_x = Vec::new();
        let mut chassis_q_y = Vec::new();
        let mut chassis_q_z = Vec::new();
        let mut chassis_q_roll = Vec::new();
        let mut chassis_q_pitch = Vec::new();
        let mut chassis_q_yaw = Vec::new();
        let mut chassis_dq_x = Vec::new();
        let mut chassis_dq_y = Vec::new();
        let mut chassis_dq_z = Vec::new();
        let mut chassis_dq_roll_rate = Vec::new();
        let mut chassis_dq_pitch_rate = Vec::new();
        let mut chassis_dq_yaw_rate = Vec::new();
        let mut wheel_q_fl = Vec::new();
        let mut wheel_q_fr = Vec::new();
        let mut wheel_q_rl = Vec::new();
        let mut wheel_q_rr = Vec::new();
        let mut wheel_dq_fl = Vec::new();
        let mut wheel_dq_fr = Vec::new();
        let mut wheel_dq_rl = Vec::new();
        let mut wheel_dq_rr = Vec::new();
        let mut wheel_torque_fl = Vec::new();
        let mut wheel_torque_fr = Vec::new();
        let mut wheel_torque_rl = Vec::new();
        let mut wheel_torque_rr = Vec::new();
        let mut pacejka_jacobian_fl = Vec::new();
        let mut pacejka_jacobian_fr = Vec::new();
        let mut pacejka_jacobian_rl = Vec::new();
        let mut pacejka_jacobian_rr = Vec::new();
        let mut normal_load_front = Vec::new();
        let mut normal_load_rear = Vec::new();
        let mut thermal_accumulated = Vec::new();
        let mut mu_friction = Vec::new();
        let mut terrain_moisture = Vec::new();
        let mut x_water = Vec::new();
        let mut gg_ekf_divergence = Vec::new();
        let mut fy_fl = Vec::new();
        let mut fy_fr = Vec::new();
        let mut fy_rl = Vec::new();
        let mut fy_rr = Vec::new();
        let mut fx_fl = Vec::new();
        let mut fx_fr = Vec::new();
        let mut fx_rl = Vec::new();
        let mut fx_rr = Vec::new();
        let mut steer_target = Vec::new();
        let mut steer_actual = Vec::new();
        let mut fluid_temp_c = Vec::new();
        let mut actual_viscosity_cst = Vec::new();
        let mut brake_fluid_temp_c = Vec::new();
        let mut d_roll = Vec::new();
        let mut d_pitch = Vec::new();
        let mut scenario_vec = Vec::new();
        let mut sha256_seal = Vec::new();
        let mut trajectory_id = Vec::new();

        for traj in trajectories {
            let t_id = traj.trajectory_id;
            for step in traj.data {
                timestamp.push(step.timestamp);
                chassis_q_x.push(step.chassis_q_x);
                chassis_q_y.push(step.chassis_q_y);
                chassis_q_z.push(step.chassis_q_z);
                chassis_q_roll.push(step.chassis_q_roll);
                chassis_q_pitch.push(step.chassis_q_pitch);
                chassis_q_yaw.push(step.chassis_q_yaw);
                chassis_dq_x.push(step.chassis_dq_x);
                chassis_dq_y.push(step.chassis_dq_y);
                chassis_dq_z.push(step.chassis_dq_z);
                chassis_dq_roll_rate.push(step.chassis_dq_roll_rate);
                chassis_dq_pitch_rate.push(step.chassis_dq_pitch_rate);
                chassis_dq_yaw_rate.push(step.chassis_dq_yaw_rate);
                wheel_q_fl.push(step.wheel_q_fl);
                wheel_q_fr.push(step.wheel_q_fr);
                wheel_q_rl.push(step.wheel_q_rl);
                wheel_q_rr.push(step.wheel_q_rr);
                wheel_dq_fl.push(step.wheel_dq_fl);
                wheel_dq_fr.push(step.wheel_dq_fr);
                wheel_dq_rl.push(step.wheel_dq_rl);
                wheel_dq_rr.push(step.wheel_dq_rr);
                wheel_torque_fl.push(step.wheel_torque_fl);
                wheel_torque_fr.push(step.wheel_torque_fr);
                wheel_torque_rl.push(step.wheel_torque_rl);
                wheel_torque_rr.push(step.wheel_torque_rr);
                pacejka_jacobian_fl.push(step.pacejka_jacobian_fl);
                pacejka_jacobian_fr.push(step.pacejka_jacobian_fr);
                pacejka_jacobian_rl.push(step.pacejka_jacobian_rl);
                pacejka_jacobian_rr.push(step.pacejka_jacobian_rr);
                normal_load_front.push(step.normal_load_front);
                normal_load_rear.push(step.normal_load_rear);
                thermal_accumulated.push(step.thermal_accumulated);
                mu_friction.push(step.mu_friction);
                terrain_moisture.push(step.terrain_moisture);
                x_water.push(step.x_water);
                gg_ekf_divergence.push(step.gg_ekf_divergence);
                fy_fl.push(step.fy_fl);
                fy_fr.push(step.fy_fr);
                fy_rl.push(step.fy_rl);
                fy_rr.push(step.fy_rr);
                fx_fl.push(step.fx_fl);
                fx_fr.push(step.fx_fr);
                fx_rl.push(step.fx_rl);
                fx_rr.push(step.fx_rr);
                steer_target.push(step.steer_target);
                steer_actual.push(step.steer_actual);
                fluid_temp_c.push(step.fluid_temp_c);
                actual_viscosity_cst.push(step.actual_viscosity_cst);
                brake_fluid_temp_c.push(step.brake_fluid_temp_c);
                d_roll.push(step.d_roll);
                d_pitch.push(step.d_pitch);
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
                    Arc::new(Float32Array::from(chassis_q_x)),
                    Arc::new(Float32Array::from(chassis_q_y)),
                    Arc::new(Float32Array::from(chassis_q_z)),
                    Arc::new(Float32Array::from(chassis_q_roll)),
                    Arc::new(Float32Array::from(chassis_q_pitch)),
                    Arc::new(Float32Array::from(chassis_q_yaw)),
                    Arc::new(Float32Array::from(chassis_dq_x)),
                    Arc::new(Float32Array::from(chassis_dq_y)),
                    Arc::new(Float32Array::from(chassis_dq_z)),
                    Arc::new(Float32Array::from(chassis_dq_roll_rate)),
                    Arc::new(Float32Array::from(chassis_dq_pitch_rate)),
                    Arc::new(Float32Array::from(chassis_dq_yaw_rate)),
                    Arc::new(Float32Array::from(wheel_q_fl)),
                    Arc::new(Float32Array::from(wheel_q_fr)),
                    Arc::new(Float32Array::from(wheel_q_rl)),
                    Arc::new(Float32Array::from(wheel_q_rr)),
                    Arc::new(Float32Array::from(wheel_dq_fl)),
                    Arc::new(Float32Array::from(wheel_dq_fr)),
                    Arc::new(Float32Array::from(wheel_dq_rl)),
                    Arc::new(Float32Array::from(wheel_dq_rr)),
                    Arc::new(Float32Array::from(wheel_torque_fl)),
                    Arc::new(Float32Array::from(wheel_torque_fr)),
                    Arc::new(Float32Array::from(wheel_torque_rl)),
                    Arc::new(Float32Array::from(wheel_torque_rr)),
                    Arc::new(Float32Array::from(pacejka_jacobian_fl)),
                    Arc::new(Float32Array::from(pacejka_jacobian_fr)),
                    Arc::new(Float32Array::from(pacejka_jacobian_rl)),
                    Arc::new(Float32Array::from(pacejka_jacobian_rr)),
                    Arc::new(Float32Array::from(normal_load_front)),
                    Arc::new(Float32Array::from(normal_load_rear)),
                    Arc::new(Float32Array::from(thermal_accumulated)),
                    Arc::new(Float32Array::from(mu_friction)),
                    Arc::new(Float32Array::from(terrain_moisture)),
                    Arc::new(Float32Array::from(x_water)),
                    Arc::new(Float32Array::from(gg_ekf_divergence)),
                    Arc::new(Float32Array::from(fy_fl)),
                    Arc::new(Float32Array::from(fy_fr)),
                    Arc::new(Float32Array::from(fy_rl)),
                    Arc::new(Float32Array::from(fy_rr)),
                    Arc::new(Float32Array::from(fx_fl)),
                    Arc::new(Float32Array::from(fx_fr)),
                    Arc::new(Float32Array::from(fx_rl)),
                    Arc::new(Float32Array::from(fx_rr)),
                    Arc::new(Float32Array::from(steer_target)),
                    Arc::new(Float32Array::from(steer_actual)),
                    Arc::new(Float32Array::from(fluid_temp_c)),
                    Arc::new(Float32Array::from(actual_viscosity_cst)),
                    Arc::new(Float32Array::from(brake_fluid_temp_c)),
                    Arc::new(Float32Array::from(d_roll)),
                    Arc::new(Float32Array::from(d_pitch)),
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
