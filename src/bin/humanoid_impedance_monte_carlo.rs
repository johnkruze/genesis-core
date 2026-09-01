// Category 5: Humanoid Active Joint-Impedance & Contact Stability
// 1000Hz Symplectic Euler integration of humanoid walking on variable-stiffness contact manifold.
// Enforces a compile-time assertion that the state struct size is exactly 128 bytes to align with Apple UMA cache line.
// Organ: resonance (zmp_from_ankle_torque_m).
// Sovereign Receipt n=2500 Dual-Regime Parquet.

use genesis_core::output;
use genesis_core::physics::resonance::zmp_from_ankle_torque_m;
use genesis_core::proof::{self, ProofChain};
use genesis_core::rng::Rng;
use rayon::prelude::*;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::sync::Arc;
use std::time::Instant;

// Arrow / Parquet imports for native writing
use arrow::array::*;
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::ArrowWriter;

const M_TOTAL: f32 = 120.0; // kg - 120kg Humanoid
const G: f32 = 9.81; // m/s^2
const H_COM: f32 = 0.9; // m - COM height
const I_PELVIS: f32 = 15.0; // kg*m^2 - pitch rotational inertia
const M_FOOT: f32 = 5.0; // kg - foot mass
const L_THIGH: f32 = 0.45; // m - thigh segment length
const L_SHIN: f32 = 0.45; // m - shin segment length
const FOOT_CONTACT_OFFSET: f32 = 0.012; // 12mm ankle/sole height
const DEFAULT_N: usize = 2500;

// 32-Dimensional state struct, size assertion = exactly 128 bytes
#[allow(dead_code)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Serialize)]
struct ImpedanceState {
    timestamp: f32,
    q: [f32; 6],
    dq: [f32; 6],
    torques: [f32; 6],
    contact_forces: [f32; 2],
    j_contact_l: [f32; 4],
    j_contact_r: [f32; 4],
    m_11: [f32; 2],
    thermal_accumulated: f32,
}

const _: () = assert!(std::mem::size_of::<ImpedanceState>() == 128);

#[allow(dead_code)]
#[derive(Serialize, Clone)]
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

#[derive(Serialize, Clone)]
struct ImpedanceSummaryRun {
    trajectory_id: u32,
    short_id: String,
    scenario: String,
    peak_slip_m: f64,
    final_thermal_accumulated: f64,
    peak_battery_temp_c: f64,
    bms_trip: bool,
    is_buckle_failed: bool,
    is_thermal_failed: bool,
    proof_hash: String,
}

#[derive(Serialize)]
struct ImpedanceTrajectory {
    summary: ImpedanceSummaryRun,
    data: Vec<ImpedanceStepState>,
}

fn solve_3x3(m: [[f32; 3]; 3], b: [f32; 3]) -> [f32; 3] {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);

    if det.abs() < 1e-6 {
        return [b[0] / 4.0, b[1] / 3.0, b[2] / 1.5];
    }

    let inv_det = 1.0 / det;

    let x0 = ((m[1][1] * m[2][2] - m[1][2] * m[2][1]) * b[0]
        - (m[0][1] * m[2][2] - m[0][2] * m[2][1]) * b[1]
        + (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * b[2])
        * inv_det;

    let x1 = (-(m[1][0] * m[2][2] - m[1][2] * m[2][0]) * b[0]
        + (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * b[1]
        - (m[0][0] * m[1][2] - m[0][2] * m[1][0]) * b[2])
        * inv_det;

    let x2 = ((m[1][0] * m[2][1] - m[1][1] * m[2][0]) * b[0]
        - (m[0][0] * m[2][1] - m[0][1] * m[2][0]) * b[1]
        + (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * b[2])
        * inv_det;

    [x0, x1, x2]
}

fn run_single_trajectory(index: usize, seed: u64, scenario: &str) -> ImpedanceTrajectory {
    let mut rng = Rng::new(seed);
    let short_id = output::short_id(&mut rng);
    let mut proof = ProofChain::new();
    proof.seed(&(index as u32).to_le_bytes());
    proof.feed_str(scenario);

    let m_total_base = rng.range(80.0, 150.0) as f32;
    let m_total = if scenario == "isometric_hold" {
        m_total_base * 1.5f32
    } else {
        m_total_base
    };
    let mass_factor = m_total / M_TOTAL;

    let kp_hip = rng.range(280.0, 360.0) as f32;
    let kp_knee = rng.range(420.0, 520.0) as f32;
    let kp_ankle = rng.range(220.0, 260.0) as f32;

    let kd_hip = rng.range(20.0, 35.0) as f32;
    let kd_knee = rng.range(30.0, 45.0) as f32;
    let kd_ankle = rng.range(12.0, 18.0) as f32;

    let kp_pelvis = rng.range(50.0, 3500.0) as f32;
    let kd_pelvis = rng.range(5.0, 250.0) as f32;

    let mu_friction = if scenario == "nominal" {
        rng.range(0.15, 0.60) as f32
    } else {
        match scenario {
            "ice" | "low_friction" => 0.10f32,
            _ => 0.50f32,
        }
    };

    let mut pos_x = 0.0f32;
    let mut pos_z = H_COM;

    let mut vel_x = rng.range(1.1, 1.4) as f32;
    let vel_z = 0.0f32;

    let mut pitch_rad = 0.0f32;
    let mut pitch_vel = 0.0f32;

    let mut q_hip_l = 0.0f32;
    let mut q_knee_l = 0.0f32;
    let mut q_ankle_l = 0.0f32;

    let mut q_hip_r = 0.0f32;
    let mut q_knee_r = 0.0f32;
    let mut q_ankle_r = 0.0f32;

    let mut dq_hip_l = 0.0f32;
    let mut dq_knee_l = 0.0f32;
    let mut dq_ankle_l = 0.0f32;

    let mut dq_hip_r = 0.0f32;
    let mut dq_knee_r = 0.0f32;
    let mut dq_ankle_r = 0.0f32;

    let mut slip_distance_l = 0.0f32;
    let mut slip_distance_r = 0.0f32;
    let mut slip_vel_l = 0.0f32;
    let mut slip_vel_r = 0.0f32;
    let mut thermal_tax = 0.0f32;
    let mut buckle_failure = false;
    let mut thermal_failure = false;

    let dt = 0.001f32;
    let total_time = 5.0f32;
    let steps_count = (total_time / dt) as usize;

    let compromise_active = scenario == "adversarial";
    let compromise_time = rng.range(0.25, 0.75) as f32;
    let compromised_joint = rng.range(0.0, 6.0) as usize;
    let compromised_limb_is_left = compromised_joint < 3;
    let sensor_delay_steps = rng.range(50.0, 150.0) as usize;

    let mut history_normal_l = vec![0.0f32; steps_count];
    let mut history_normal_r = vec![0.0f32; steps_count];


    let (mut j_11_l, mut j_12_l, mut j_21_l, mut j_22_l, mut j_11_r, mut j_12_r, mut j_21_r, mut j_22_r);

    let mut running_hash = Sha256::new();
    running_hash.update(&seed.to_le_bytes());
    let mut last_hash = running_hash.finalize();

    let mut gear_wear_factor = 0.0f32;
    let mut tendon_elongation_l_mm = 0.0f32;
    let mut tendon_elongation_r_mm = 0.0f32;
    let mut torque_sq = 0.0f32;
    let mut torque_ankle_l_prev = 0.0f32;
    let mut torque_ankle_r_prev = 0.0f32;
    let payload_mass_kg = if scenario == "liquid_transport" {
        10.0f32
    } else {
        0.0f32
    };
    let slosh_omega = 1.2f32 * 2.0f32 * std::f32::consts::PI;
    let slosh_stiffness = payload_mass_kg * slosh_omega * slosh_omega;
    let slosh_damping = 1.5f32;
    let mut slosh_displacement_m = 0.0f32;
    let mut slosh_velocity_ms = 0.0f32;
    let mut battery_cell_temp_c = 25.0f32;
    let mut bms_trip = false;
    let mut seize_duration = 0;

    for step in 0..steps_count {
        let t = step as f32 * dt;

        let terrain_stiffness_base = match scenario {
            "soft_terrain" | "mud" => 10000.0f32,
            _ => 50000.0f32,
        };
        let terrain_stiffness =
            terrain_stiffness_base + 30000.0f32 * (2.0f32 * std::f32::consts::PI * pos_x).sin();
        let terrain_damping = 800.0f32;

        let step_cycle = if scenario == "liquid_transport" {
            0.7f32 + (t / 5.0f32) * 0.4f32
        } else {
            0.8f32
        };
        let omega_gait = 2.0f32 * std::f32::consts::PI / step_cycle;

        let is_isometric = scenario == "isometric_hold";

        let (q_des_hip_l, q_des_knee_l, q_des_ankle_l, q_des_hip_r, q_des_knee_r, q_des_ankle_r) =
            if is_isometric {
                (0.15f32, -0.60f32, 0.45f32, 0.15f32, -0.60f32, 0.45f32)
            } else {
                let q_hip_l_des = 0.3f32 * (omega_gait * t).sin();
                let q_knee_l_des = -0.25f32 * (1.0f32 - (omega_gait * t).cos());
                let q_ankle_l_des = -q_hip_l_des - q_knee_l_des;

                let q_hip_r_des = 0.3f32 * (omega_gait * t + std::f32::consts::PI).sin();
                let q_knee_r_des =
                    -0.25f32 * (1.0f32 - (omega_gait * t + std::f32::consts::PI).cos());
                let q_ankle_r_des = -q_hip_r_des - q_knee_r_des;
                (
                    q_hip_l_des,
                    q_knee_l_des,
                    q_ankle_l_des,
                    q_hip_r_des,
                    q_knee_r_des,
                    q_ankle_r_des,
                )
            };

        let (
            dq_des_hip_l,
            dq_des_knee_l,
            dq_des_ankle_l,
            dq_des_hip_r,
            dq_des_knee_r,
            dq_des_ankle_r,
        ) = if is_isometric {
            (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32)
        } else {
            let dq_hip_l_des = 0.3f32 * omega_gait * (omega_gait * t).cos();
            let dq_knee_l_des = -0.25f32 * omega_gait * (omega_gait * t).sin();
            let dq_ankle_l_des = -dq_hip_l_des - dq_knee_l_des;

            let dq_hip_r_des = 0.3f32 * omega_gait * (omega_gait * t + std::f32::consts::PI).cos();
            let dq_knee_r_des = -0.25f32 * omega_gait * (omega_gait * t + std::f32::consts::PI).sin();
            let dq_ankle_r_des = -dq_hip_r_des - dq_knee_r_des;
            (
                dq_hip_l_des,
                dq_knee_l_des,
                dq_ankle_l_des,
                dq_hip_r_des,
                dq_knee_r_des,
                dq_ankle_r_des,
            )
        };

        let apply_backlash = |q_des: f32, q: f32, backlash_limit: f32| -> f32 {
            let q_error = q_des - q;
            if q_error.abs() > backlash_limit {
                q_error - q_error.signum() * backlash_limit
            } else {
                0.0f32
            }
        };

        let wear_rate = if scenario == "wear_fatigue" {
            0.25f32
        } else {
            0.01f32
        };
        gear_wear_factor =
            (gear_wear_factor + (torque_sq / 10000.0f32) * wear_rate * dt).min(1.0f32);

        let mut seize_event = false;
        let dynamic_backlash = if gear_wear_factor >= 0.95f32 {
            seize_event = true;
            0.080f32
        } else {
            0.002f32 + 0.013f32 * gear_wear_factor
        };

        if seize_event {
            seize_duration += 1;
            if seize_duration >= 100 {
                buckle_failure = true;
            }
        } else {
            seize_duration = 0;
        }

        let creep_rate = if scenario == "wear_fatigue" {
            0.5f32
        } else {
            0.02f32
        };
        let creep_accum_l = (torque_ankle_l_prev.abs() / 80.0f32).powi(2) * creep_rate * dt;
        let creep_accum_r = (torque_ankle_r_prev.abs() / 80.0f32).powi(2) * creep_rate * dt;
        tendon_elongation_l_mm += creep_accum_l * 5.0f32;
        tendon_elongation_r_mm += creep_accum_r * 5.0f32;

        let creep_factor_l = (tendon_elongation_l_mm / 1.0f32).min(0.5f32);
        let creep_factor_r = (tendon_elongation_r_mm / 1.0f32).min(0.5f32);

        let kp_ankle_flex_l = kp_ankle;
        let kp_ankle_extend_l = kp_ankle * (1.0f32 - creep_factor_l);
        let kp_ankle_flex_r = kp_ankle;
        let kp_ankle_extend_r = kp_ankle * (1.0f32 - creep_factor_r);

        let err_ankle_l = q_des_ankle_l - q_ankle_l;
        let kp_ankle_active_l = if err_ankle_l < 0.0f32 {
            kp_ankle_extend_l
        } else {
            kp_ankle_flex_l
        };

        let err_ankle_r = q_des_ankle_r - q_ankle_r;
        let kp_ankle_active_r = if err_ankle_r < 0.0f32 {
            kp_ankle_extend_r
        } else {
            kp_ankle_flex_r
        };

        let get_backlash = |joint_idx: usize| -> f32 {
            if compromise_active && t >= compromise_time && joint_idx == compromised_joint {
                0.100f32
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

        let foot_height_l = pos_z
            - (L_THIGH * q_hip_l.cos() + L_SHIN * (q_hip_l + q_knee_l).cos())
            - FOOT_CONTACT_OFFSET;
        let foot_height_r = pos_z
            - (L_THIGH * q_hip_r.cos() + L_SHIN * (q_hip_r + q_knee_r).cos())
            - FOOT_CONTACT_OFFSET;

        let normal_l = if foot_height_l < 0.0f32 {
            -foot_height_l * terrain_stiffness - vel_z * terrain_damping
        } else {
            0.0f32
        }
        .max(0.0f32);

        let normal_r = if foot_height_r < 0.0f32 {
            -foot_height_r * terrain_stiffness - vel_z * terrain_damping
        } else {
            0.0f32
        }
        .max(0.0f32);

        history_normal_l[step] = normal_l;
        history_normal_r[step] = normal_r;

        let mut normal_measured_l = normal_l;
        let mut normal_measured_r = normal_r;

        if compromise_active && t >= compromise_time {
            if compromised_limb_is_left {
                let delay_idx = if step >= sensor_delay_steps {
                    step - sensor_delay_steps
                } else {
                    0
                };
                normal_measured_l = history_normal_l[delay_idx] + 25.0f32 * (5.0f32 * t).sin();
                if normal_measured_l < 0.0f32 {
                    normal_measured_l = 0.0f32;
                }
            } else {
                let delay_idx = if step >= sensor_delay_steps {
                    step - sensor_delay_steps
                } else {
                    0
                };
                normal_measured_r = history_normal_r[delay_idx] + 25.0f32 * (5.0f32 * t).sin();
                if normal_measured_r < 0.0f32 {
                    normal_measured_r = 0.0f32;
                }
            }
        }

        let expected_force_est_l = if foot_height_l < 0.0f32 {
            -foot_height_l * 50000.0f32
        } else {
            0.0f32
        };
        let expected_force_est_r = if foot_height_r < 0.0f32 {
            -foot_height_r * 50000.0f32
        } else {
            0.0f32
        };

        let tracking_err_l = (q_des_hip_l - q_hip_l).abs()
            + (q_des_knee_l - q_knee_l).abs()
            + (q_des_ankle_l - q_ankle_l).abs();
        let tracking_err_r = (q_des_hip_r - q_hip_r).abs()
            + (q_des_knee_r - q_knee_r).abs()
            + (q_des_ankle_r - q_ankle_r).abs();

        let inconsistency_l =
            (expected_force_est_l - normal_measured_l).abs() + 1500.0f32 * tracking_err_l;
        let inconsistency_r =
            (expected_force_est_r - normal_measured_r).abs() + 1500.0f32 * tracking_err_r;

        let left_isolated =
            compromise_active && compromised_limb_is_left && inconsistency_l > 150.0f32;
        let right_isolated =
            compromise_active && !compromised_limb_is_left && inconsistency_r > 150.0f32;

        let torque_stabilize = kp_pelvis * pitch_rad + kd_pelvis * pitch_vel;

        let (
            mut torque_hip_l,
            mut torque_knee_l,
            mut torque_ankle_l,
            mut torque_hip_r,
            mut torque_knee_r,
            mut torque_ankle_r,
        );

        if left_isolated {
            torque_hip_l = -0.5f32 * dq_hip_l;
            torque_knee_l = -0.5f32 * dq_knee_l;
            torque_ankle_l = -0.5f32 * dq_ankle_l;

            let q_des_hip_l_ret = 0.15f32;
            let q_des_knee_l_ret = -0.80f32;
            let q_des_ankle_l_ret = -q_des_hip_l_ret - q_des_knee_l_ret;

            torque_hip_l += 50.0f32 * (q_des_hip_l_ret - q_hip_l);
            torque_knee_l += 50.0f32 * (q_des_knee_l_ret - q_knee_l);
            torque_ankle_l += 30.0f32 * (q_des_ankle_l_ret - q_ankle_l);

            let kp_hip_r_active = kp_hip * 1.6f32;
            let kp_knee_r_active = kp_knee * 1.6f32;
            let kp_ankle_r_active = kp_ankle_active_r * 1.6f32;
            let kd_hip_r_active = kd_hip * 1.6f32;
            let kd_knee_r_active = kd_knee * 1.6f32;
            let kd_ankle_r_active = kd_ankle * 1.6f32;

            let q_des_knee_r_ext = q_des_knee_r * 0.90f32;

            torque_hip_r = kp_hip_r_active
                * (apply_backlash(q_des_hip_r, q_hip_r, bl_hip_r) + pitch_rad)
                + kd_hip_r_active * (dq_des_hip_r - dq_hip_r + pitch_vel)
                + 1.0f32 * torque_stabilize;
            torque_knee_r = kp_knee_r_active * apply_backlash(q_des_knee_r_ext, q_knee_r, bl_knee_r)
                + kd_knee_r_active * (dq_des_knee_r - dq_knee_r);
            torque_ankle_r = kp_ankle_r_active
                * apply_backlash(-q_des_hip_r - q_des_knee_r_ext, q_ankle_r, bl_ankle_r)
                + kd_ankle_r_active * (dq_des_ankle_r - dq_ankle_r);
        } else if right_isolated {
            torque_hip_r = -0.5f32 * dq_hip_r;
            torque_knee_r = -0.5f32 * dq_knee_r;
            torque_ankle_r = -0.5f32 * dq_ankle_r;

            let q_des_hip_r_ret = 0.15f32;
            let q_des_knee_r_ret = -0.80f32;
            let q_des_ankle_r_ret = -q_des_hip_r_ret - q_des_knee_r_ret;

            torque_hip_r += 50.0f32 * (q_des_hip_r_ret - q_hip_r);
            torque_knee_r += 50.0f32 * (q_des_knee_r_ret - q_knee_r);
            torque_ankle_r += 30.0f32 * (q_des_ankle_r_ret - q_ankle_r);

            let kp_hip_l_active = kp_hip * 1.6f32;
            let kp_knee_l_active = kp_knee * 1.6f32;
            let kp_ankle_l_active = kp_ankle_active_l * 1.6f32;
            let kd_hip_l_active = kd_hip * 1.6f32;
            let kd_knee_l_active = kd_knee * 1.6f32;
            let kd_ankle_l_active = kd_ankle * 1.6f32;

            let q_des_knee_l_ext = q_des_knee_l * 0.90f32;

            torque_hip_l = kp_hip_l_active
                * (apply_backlash(q_des_hip_l, q_hip_l, bl_hip_l) + pitch_rad)
                + kd_hip_l_active * (dq_des_hip_l - dq_hip_l + pitch_vel)
                + 1.0f32 * torque_stabilize;
            torque_knee_l = kp_knee_l_active * apply_backlash(q_des_knee_l_ext, q_knee_l, bl_knee_l)
                + kd_knee_l_active * (dq_des_knee_l - dq_knee_l);
            torque_ankle_l = kp_ankle_l_active
                * apply_backlash(-q_des_hip_l - q_des_knee_l_ext, q_ankle_l, bl_ankle_l)
                + kd_ankle_l_active * (dq_des_ankle_l - dq_ankle_l);
        } else {
            torque_hip_l = kp_hip * (apply_backlash(q_des_hip_l, q_hip_l, bl_hip_l) + pitch_rad)
                + kd_hip * (dq_des_hip_l - dq_hip_l + pitch_vel)
                + 0.5f32 * torque_stabilize;
            torque_knee_l = kp_knee * apply_backlash(q_des_knee_l, q_knee_l, bl_knee_l)
                + kd_knee * (dq_des_knee_l - dq_knee_l);
            torque_ankle_l = kp_ankle_active_l
                * apply_backlash(q_des_ankle_l, q_ankle_l, bl_ankle_l)
                + kd_ankle * (dq_des_ankle_l - dq_ankle_l);

            torque_hip_r = kp_hip * (apply_backlash(q_des_hip_r, q_hip_r, bl_hip_r) + pitch_rad)
                + kd_hip * (dq_des_hip_r - dq_hip_r + pitch_vel)
                + 0.5f32 * torque_stabilize;
            torque_knee_r = kp_knee * apply_backlash(q_des_knee_r, q_knee_r, bl_knee_r)
                + kd_knee * (dq_des_knee_r - dq_knee_r);
            torque_ankle_r = kp_ankle_active_r
                * apply_backlash(q_des_ankle_r, q_ankle_r, bl_ankle_r)
                + kd_ankle * (dq_des_ankle_r - dq_ankle_r);
        }

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

        // Organ integration call: zmp_from_ankle_torque_m
        let _zmp_approx = zmp_from_ankle_torque_m(
            (torque_ankle_l + torque_ankle_r) as f64,
            (normal_l + normal_r).max(1.0) as f64,
        );

        torque_sq = torque_hip_l.powi(2)
            + torque_knee_l.powi(2)
            + torque_ankle_l.powi(2)
            + torque_hip_r.powi(2)
            + torque_knee_r.powi(2)
            + torque_ankle_r.powi(2);
        thermal_tax += (torque_sq / 1000.0f32) * dt;

        if thermal_tax > 350.0f32 {
            thermal_failure = true;
        }

        let battery_heating = (torque_sq / 12000.0f32) * dt;
        let cooling_coefficient = if scenario == "isometric_hold" {
            0.005f32
        } else {
            0.015f32
        };
        let battery_cooling = cooling_coefficient * (battery_cell_temp_c - 25.0f32) * dt;
        battery_cell_temp_c += battery_heating * 8.0f32 - battery_cooling;
        if battery_cell_temp_c > 90.0f32 {
            bms_trip = true;
        }

        torque_ankle_l_prev = torque_ankle_l;
        torque_ankle_r_prev = torque_ankle_r;

        let shear_cmd_l = torque_ankle_l / 0.15f32;
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

        if slip_distance_l > 0.40f32 || slip_distance_r > 0.40f32 {
            buckle_failure = true;
        }

        let force_prop_l = if normal_l > 0.0f32 {
            shear_cmd_l.clamp(-shear_max_l, shear_max_l)
        } else {
            0.0f32
        };
        let force_prop_r = if normal_r > 0.0f32 {
            shear_cmd_r.clamp(-shear_max_r, shear_max_r)
        } else {
            0.0f32
        };
        let approx_accel_x = (force_prop_l + force_prop_r) / m_total - 0.02f32 * vel_x;

        let slosh_accel = if payload_mass_kg > 0.0f32 {
            (-slosh_stiffness * slosh_displacement_m
                - slosh_damping * slosh_velocity_ms
                - (payload_mass_kg * approx_accel_x))
                / payload_mass_kg
        } else {
            0.0f32
        };
        slosh_velocity_ms += slosh_accel * dt;
        slosh_displacement_m += slosh_velocity_ms * dt;
        let slosh_gravity_torque = payload_mass_kg * G * slosh_displacement_m;

        let max_slip_dist = slip_distance_l.max(slip_distance_r);
        let torque_tipping = m_total * G * max_slip_dist + slosh_gravity_torque;
        let pitch_accel = -torque_tipping / I_PELVIS - (torque_hip_l + torque_hip_r) / I_PELVIS;

        pitch_vel += pitch_accel * dt;
        pitch_rad += pitch_vel * dt;

        pos_z = H_COM - 0.7f32 * pitch_rad.abs() - 0.4f32 * max_slip_dist;
        if pos_z < 0.15f32 {
            buckle_failure = true;
        }

        let solve_coupled_leg =
            |q: &mut [f32; 3],
             dq: &mut [f32; 3],
             torques: [f32; 3],
             normal: f32,
             shear_prop: f32,
             j11: f32,
             j12: f32,
             j21: f32,
             j22: f32,
             q_min: [f32; 3],
             q_max: [f32; 3]| {
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

                let c1 =
                    -1.0125f32 * q[1].sin() * (2.0f32 * dq[0] * dq[1] + dq[1] * dq[1]) * mass_factor;
                let c2 = 1.0125f32 * q[1].sin() * dq[0] * dq[0] * mass_factor;
                let c3 = -0.05f32 * q[1].sin() * dq[0] * dq[0] * mass_factor;

                let g1 =
                    (147.15f32 * q[0].sin() + 49.05f32 * (q[0] + q[1]).sin()) * mass_factor;
                let g2 = 49.05f32 * (q[0] + q[1]).sin() * mass_factor;
                let g_const = 9.81f32;
                let g3 = 1.5f32 * mass_factor * g_const * (q[0] + q[1] + q[2]).sin();

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

                for i in 0..3 {
                    dq[i] = (dq[i] + acc[i] * dt).clamp(-12.0f32, 12.0f32);
                    q[i] += dq[i] * dt;
                    if q[i] < q_min[i] {
                        q[i] = q_min[i];
                        dq[i] = 0.0f32;
                    } else if q[i] > q_max[i] {
                        q[i] = q_max[i];
                        dq[i] = 0.0f32;
                    }
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

        solve_coupled_leg(
            &mut q_l,
            &mut dq_l,
            [torque_hip_l, torque_knee_l, torque_ankle_l],
            normal_l,
            shear_cmd_l.clamp(-shear_max_l, shear_max_l),
            j_11_l,
            j_12_l,
            j_21_l,
            j_22_l,
            [-1.5f32, -2.0f32, -1.0f32],
            [1.5f32, 0.0f32, 1.0f32],
        );
        solve_coupled_leg(
            &mut q_r,
            &mut dq_r,
            [torque_hip_r, torque_knee_r, torque_ankle_r],
            normal_r,
            shear_cmd_r.clamp(-shear_max_r, shear_max_r),
            j_11_r,
            j_12_r,
            j_21_r,
            j_22_r,
            [-1.5f32, -2.0f32, -1.0f32],
            [1.5f32, 0.0f32, 1.0f32],
        );

        q_hip_l = q_l[0];
        q_knee_l = q_l[1];
        q_ankle_l = q_l[2];
        dq_hip_l = dq_l[0];
        dq_knee_l = dq_l[1];
        dq_ankle_l = dq_l[2];

        q_hip_r = q_r[0];
        q_knee_r = q_r[1];
        q_ankle_r = q_r[2];
        dq_hip_r = dq_r[0];
        dq_knee_r = dq_r[1];
        dq_ankle_r = dq_r[2];

        let force_prop_l = if normal_l > 0.0f32 {
            shear_cmd_l.clamp(-shear_max_l, shear_max_l)
        } else {
            0.0f32
        };
        let force_prop_r = if normal_r > 0.0f32 {
            shear_cmd_r.clamp(-shear_max_r, shear_max_r)
        } else {
            0.0f32
        };
        let accel_x = (force_prop_l + force_prop_r) / m_total - 0.02f32 * vel_x;
        vel_x += accel_x * dt;
        pos_x += vel_x * dt;

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
            let _ = hex::encode(last_hash);
        }

        if buckle_failure || thermal_failure {
            break;
        }
    }

    let peak_slip = slip_distance_l.max(slip_distance_r) as f64;
    let is_thermal_failed = thermal_failure;

    proof.feed_f64(peak_slip);
    proof.feed_f64(thermal_tax as f64);
    proof.feed_str(if buckle_failure {
        "BUCKLE_FAIL"
    } else if is_thermal_failed {
        "THERMAL_FAIL"
    } else {
        "STABLE"
    });

    let proof_hash = proof.seal();

    let summary = ImpedanceSummaryRun {
        trajectory_id: index as u32,
        short_id,
        scenario: scenario.to_string(),
        peak_slip_m: (peak_slip * 1000.0).round() / 1000.0,
        final_thermal_accumulated: (thermal_tax as f64 * 10.0).round() / 10.0,
        peak_battery_temp_c: (battery_cell_temp_c as f64 * 10.0).round() / 10.0,
        bms_trip,
        is_buckle_failed: buckle_failure,
        is_thermal_failed,
        proof_hash,
    };

    ImpedanceTrajectory {
        summary,
        data: Vec::new(),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(DEFAULT_N);

    let out = args
        .iter()
        .position(|a| a == "--parquet")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| {
            format!(
                "{}/../../data/exports/sovereign/humanoid_impedance.parquet",
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
    println!("  G^G: HUMANOID IMPEDANCE ACTIVE DYNAMICS (resonance)");
    println!("  n={n}  scenario={scenario}  out={out}");
    println!("====================================================================\n");

    let start = Instant::now();
    let base_seed = 0x1337_F00D_BAAD_F00Du64;
    let seed_multiplier = 0x9E37_79B1_85EB_CA87u64;

    let trajectories: Vec<ImpedanceTrajectory> = (0..n)
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

    let summaries: Vec<ImpedanceSummaryRun> = trajectories.iter().map(|t| t.summary.clone()).collect();
    let proofs: Vec<String> = summaries.iter().map(|s| s.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("scenario", DataType::Utf8, false),
        Field::new("peak_slip_m", DataType::Float64, false),
        Field::new("final_thermal_accumulated", DataType::Float64, false),
        Field::new("peak_battery_temp_c", DataType::Float64, false),
        Field::new("bms_trip", DataType::Boolean, false),
        Field::new("is_buckle_failed", DataType::Boolean, false),
        Field::new("is_thermal_failed", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(summaries.iter().map(|s| s.trajectory_id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(summaries.iter().map(|s| s.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(StringArray::from(summaries.iter().map(|s| s.scenario.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(summaries.iter().map(|s| s.peak_slip_m).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(summaries.iter().map(|s| s.final_thermal_accumulated).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(summaries.iter().map(|s| s.peak_battery_temp_c).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(summaries.iter().map(|s| s.bms_trip).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(summaries.iter().map(|s| s.is_buckle_failed).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(summaries.iter().map(|s| s.is_thermal_failed).collect::<Vec<_>>())),
            Arc::new(StringArray::from(summaries.iter().map(|s| s.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");

    let file = File::create(&out).expect("Failed to create output Parquet file");
    let props = output::parquet_receipt_properties(&seal, "G^G humanoid impedance dual-regime v3.0");
    let mut writer = ArrowWriter::try_new(file, schema, Some(props)).expect("Failed to create ArrowWriter");
    writer.write(&batch).expect("Failed to write RecordBatch");
    writer.close().expect("Failed to close ArrowWriter");

    let n_f = n as f64;
    let buckle = summaries.iter().filter(|s| s.is_buckle_failed).count();
    let thermal = summaries.iter().filter(|s| s.is_thermal_failed).count();
    let bms = summaries.iter().filter(|s| s.bms_trip).count();
    println!(
        "  buckle_failed {buckle} ({:.1}%)  thermal_failed {thermal} ({:.1}%)  bms_trip {bms} ({:.1}%)",
        100.0 * buckle as f64 / n_f,
        100.0 * thermal as f64 / n_f,
        100.0 * bms as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", start.elapsed());
}
