//! Hydroplane. Pacejka + freshwater ρ. 1 kHz chassis, 5 s. Mix dry vs puddle.
//! Dual-regime: Pacejka μ < 0.25 vs missed 500 m arc (|y| < 10 m). reprC 128 cache line.
//! Organ: terran::RHO_FRESHWATER. Do not run museum twin vehicle_hydroplaning.

use genesis_core::output;
use genesis_core::physics::terran::RHO_FRESHWATER;
use genesis_core::proof::{self, ProofChain};
use genesis_core::rng::Rng;
use rayon::prelude::*;
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{BooleanArray, Float64Array, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::ArrowWriter;

const M_CHASSIS: f32 = 2000.0; // kg
const G: f32 = 9.81;
const I_YAW: f32 = 3000.0;     // kg*m^2
const A_FRONT: f32 = 1.4;      // m - CoG to front axle
const B_REAR: f32 = 1.4;       // m - CoG to rear axle
const TRACK_W: f32 = 1.6;      // m - track width
const H_CG: f32 = 0.6;         // m - center of gravity height
const R_WHEEL: f32 = 0.35;     // m - wheel radius
const I_WHEEL: f32 = 2.0;      // kg*m^2 - wheel rotational inertia
const HYDRO_MU: f32 = 0.25;    // Pacejka μ collapse
const CORNER_LOST_M: f32 = 10.0; // missed the ~500 m arc (dry sagitta ~40 m)

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

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    scenario: String,
    v_start_ms: f64,
    x_water_m: f64,
    min_mu: f64,
    max_ekf_drift_m: f64,
    max_abs_yaw_rate: f64,
    max_abs_pos_y_m: f64,
    is_wet: bool,
    is_hydroplane: bool,
    is_corner_lost: bool,
    proof_hash: String,
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

fn run_single_trajectory(index: usize, seed: u64, scenario: &str) -> Run {
    let mut rng = Rng::new(seed);
    let short_id = output::short_id(&mut rng);
    
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
    let wet = rng.chance(0.40);
    let x_water = if wet {
        rng.range(4.0, 12.0) as f32
    } else {
        1.0e6f32
    };
    let water_transition_len = rng.range(0.1, 0.5) as f32;
    
    let dt = 0.001f32; // 1000Hz (1.0ms steps)
    let total_time = 5.0f32; // 5.0 second simulation
    let steps_count = (total_time / dt) as usize;
    
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
    let mut min_mu = 1.0f32;
    let mut max_abs_yaw_rate = 0.0f32;
    let mut max_abs_pos_y = 0.0f32;
    let mut thermal_tax = 0.0f32;
    
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
        let rho_scale = (RHO_FRESHWATER as f32) / 1000.0;
        let mu_wet_dynamic = mu_wet / (1.0f32 + 0.0005f32 * vel_x * vel_x * rho_scale);
        let mu_actual = mu_dry - (mu_dry - mu_wet_dynamic) * terrain_moisture;
        min_mu = min_mu.min(mu_actual);
        
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
            1.0f32 - 0.70f32 * ((brake_fluid_temp_c - 120.0f32) / 80.0f32).min(1.0f32)
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
        max_abs_yaw_rate = max_abs_yaw_rate.max(yaw_rate.abs());
        
        pos_x += (vel_x * yaw.cos() - vel_y * yaw.sin()) * dt;
        pos_y += (vel_x * yaw.sin() + vel_y * yaw.cos()) * dt;
        max_abs_pos_y = max_abs_pos_y.max(pos_y.abs());
        
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
        let tan_steer = steer_actual.tan();
        let l_wheelbase = A_FRONT + B_REAR;
        
        let next_ekf_x = ekf_x + ekf_v * cos_yaw * dt;
        let next_ekf_y = ekf_y + ekf_v * sin_yaw * dt;
        let next_ekf_yaw = ekf_yaw + (ekf_v / l_wheelbase) * tan_steer * dt;
        let _next_ekf_v = ekf_v + lon_accel_filtered * dt;
        
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
        
        // Blind bicycle: no GPS. Speedometer on v. Lateral slip is the cliff.
        ekf_x = next_ekf_x;
        ekf_y = next_ekf_y;
        ekf_yaw = next_ekf_yaw;
        ekf_v = vel_x + rng.range(-0.1, 0.1) as f32;
        ekf_p = p_next;
        
        let gg_ekf_divergence = ((pos_x - ekf_x).powi(2) + (pos_y - ekf_y).powi(2)).sqrt();
        if gg_ekf_divergence > max_ekf_drift {
            max_ekf_drift = gg_ekf_divergence;
        }

        let _ = (
            pos_z,
            jacobian_fl,
            jacobian_fr,
            jacobian_rl,
            jacobian_rr,
            normal_load_front,
            normal_load_rear,
            thermal_tax,
            ekf_steer,
        );
    }

    let hydroplane = min_mu < HYDRO_MU;
    let corner_lost = max_abs_pos_y < CORNER_LOST_M;
    let class = if corner_lost {
        "UNDER"
    } else if hydroplane {
        "HYDRO"
    } else {
        "GRIP"
    };

    let mut proof = ProofChain::new();
    proof.seed(&(index as u32).to_le_bytes());
    proof.feed_f64(v_start as f64);
    proof.feed_f64(x_water as f64);
    proof.feed_f64(min_mu as f64);
    proof.feed_f64(max_abs_pos_y as f64);
    proof.feed_str(class);

    Run {
        id: index as u32,
        short_id,
        scenario: scenario.to_string(),
        v_start_ms: (v_start as f64 * 100.0).round() / 100.0,
        x_water_m: if wet {
            (x_water as f64 * 100.0).round() / 100.0
        } else {
            -1.0
        },
        min_mu: (min_mu as f64 * 1000.0).round() / 1000.0,
        max_ekf_drift_m: (max_ekf_drift as f64 * 1000.0).round() / 1000.0,
        max_abs_yaw_rate: (max_abs_yaw_rate as f64 * 1000.0).round() / 1000.0,
        max_abs_pos_y_m: (max_abs_pos_y as f64 * 100.0).round() / 100.0,
        is_wet: wet,
        is_hydroplane: hydroplane,
        is_corner_lost: corner_lost,
        proof_hash: proof.seal(),
    }
}

fn main() {
    const DEFAULT_N: usize = 2500;
    let args: Vec<String> = std::env::args().collect();
    let n: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(DEFAULT_N);
    let out = args
        .iter()
        .position(|a| a == "--parquet" || a == "--out")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| {
            format!(
                "{}/../../data/exports/sovereign/vehicle_hydroplane.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    let scenario = args
        .iter()
        .position(|a| a == "--scenario")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "sweep".to_string());

    println!("====================================================================");
    println!("  G^G: HYDROPLANE  (Pacejka + ρ_fresh, 1 kHz, 5 s)");
    println!("  n={n}  scenario={scenario}");
    println!("====================================================================\n");
    let t0 = Instant::now();
    let base_seed = 0xDECD_BAFC_A0FE_1337u64;
    let seed_multiplier = 0x9E37_79B1_85EB_CA87u64;
    let rows: Vec<Run> = (0..n)
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
                scenario.as_str()
            };
            run_single_trajectory(i, seed, scenario_for_traj)
        })
        .collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("scenario", DataType::Utf8, false),
        Field::new("v_start_ms", DataType::Float64, false),
        Field::new("x_water_m", DataType::Float64, false),
        Field::new("min_mu", DataType::Float64, false),
        Field::new("max_ekf_drift_m", DataType::Float64, false),
        Field::new("max_abs_yaw_rate", DataType::Float64, false),
        Field::new("max_abs_pos_y_m", DataType::Float64, false),
        Field::new("is_wet", DataType::Boolean, false),
        Field::new("is_hydroplane", DataType::Boolean, false),
        Field::new("is_corner_lost", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.scenario.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.v_start_ms).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.x_water_m).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.min_mu).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.max_ekf_drift_m).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.max_abs_yaw_rate).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.max_abs_pos_y_m).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_wet).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_hydroplane).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_corner_lost).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G hydroplane dual-regime v1.1");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let nf = n as f64;
    let wet = rows.iter().filter(|r| r.is_wet).count();
    let hydro = rows.iter().filter(|r| r.is_hydroplane).count();
    let lost = rows.iter().filter(|r| r.is_corner_lost).count();
    let grip = rows.iter().filter(|r| !r.is_hydroplane && !r.is_corner_lost).count();
    println!(
        "  wet {wet} ({:.1}%)  hydro {hydro} ({:.1}%)  corner-lost {lost} ({:.1}%)  grip {grip} ({:.1}%)",
        100.0 * wet as f64 / nf,
        100.0 * hydro as f64 / nf,
        100.0 * lost as f64 / nf,
        100.0 * grip as f64 / nf
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
