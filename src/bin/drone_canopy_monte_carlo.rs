// Category 3: Drones & Adversarial Dynamics (Aerodynamics & Ground/Terrain)
// 1000Hz Euler-Boussinesq wind-shear tensor coupling and attitude recovery.
// Enforces compile-time assertion that the state struct size is exactly 128 bytes to align with UMA cache lines.

use genesis_core::rng::Rng;
use rayon::prelude::*;
use serde::Serialize;
use std::fs::File;
use std::time::Instant;
use sha2::{Sha256, Digest};
use arrow::array::{Float32Array, StringArray, BooleanArray};
use arrow::record_batch::RecordBatch;
use arrow::datatypes::{Schema, Field, DataType};
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::file::properties::WriterProperties;
use parquet::basic::Compression;
use std::sync::Arc;
use std::collections::VecDeque;
use genesis_core::physics::atheric::{AthericSystem, dbm_to_watts};

const G: f32 = 9.80665f32;
const ARM_LEN: f32 = 0.25f32; // drone arm length (m)
const C_M: f32 = 0.01f32;    // drag moment coefficient (m)

// Strictly 128 bytes - aligned to Apple Silicon UMA cache lines
#[repr(C, align(128))]
struct DroneDynamicsState {
    timestamp: f32,                       // 4 bytes
    pos: [f32; 3],                        // 12 bytes
    vel: [f32; 3],                        // 12 bytes
    quat: [f32; 4],                       // 16 bytes
    ang_vel: [f32; 3],                    // 12 bytes
    motor_torques: [f32; 4],              // 16 bytes
    inertia_tensor: [f32; 3],             // 12 bytes
    wind_velocity: [f32; 3],              // 12 bytes
    thermal_accumulated: f32,             // 4 bytes
    gg_ekf_divergence: f32,               // 4 bytes
    cog_offset: [f32; 3],                 // 12 bytes
    control_delay: f32,                   // 4 bytes
    icing_severity: f32,                  // 4 bytes
    acoustic_resonance_g: f32,            // 4 bytes
}

// Compile-time size assertion to prevent layout drift
const _: () = {
    let size = std::mem::size_of::<DroneDynamicsState>();
    if size != 128 {
        panic!("DroneDynamicsState must be exactly 128 bytes");
    }
};

#[derive(Serialize)]
struct DroneStepState {
    timestamp: f32,
    drone_pos_x: f32,
    drone_pos_y: f32,
    drone_pos_z: f32,
    drone_vel_x: f32,
    drone_vel_y: f32,
    drone_vel_z: f32,
    quat_w: f32,
    quat_x: f32,
    quat_y: f32,
    quat_z: f32,
    ang_vel_x: f32,
    ang_vel_y: f32,
    ang_vel_z: f32,
    wind_velocity_x: f32,
    wind_velocity_y: f32,
    wind_velocity_z: f32,
    rotor_thrust_1: f32,
    rotor_thrust_2: f32,
    rotor_thrust_3: f32,
    rotor_thrust_4: f32,
    air_density_rho: f32,
    atheric_coherence: f32,
    lateral_position_drift_m: f32,
    is_in_vrs: bool,
    scenario: String,
    sha256_seal: String,
    thermal_accumulated: f32,
    gg_ekf_divergence: f32,
    cog_offset_x: f32,
    cog_offset_y: f32,
    cog_offset_z: f32,
    control_delay: f32,
    inertia_tensor_xx: f32,
    inertia_tensor_yy: f32,
    inertia_tensor_zz: f32,
    inertia_tensor_xy: f32,
    inertia_tensor_yz: f32,
    inertia_tensor_xz: f32,
    rotor_icing_1: f32,
    rotor_icing_2: f32,
    rotor_icing_3: f32,
    rotor_icing_4: f32,
    acoustic_resonance_g: f32,
    is_jammed: bool,
    is_spoofed: bool,
    ew_jamming_dbm: f32,
    gps_spoofing_bias_x: f32,
    gps_spoofing_bias_y: f32,
    gps_spoofing_bias_z: f32,
    voltage_sag: f32,
    atheric_capacity: f32,
    clock_drift: f32,
    is_in_pio: bool,
    pio_amplitude_rad: f32,
    rotor_icing_drag_torque_delta_1: f32,
    rotor_icing_drag_torque_delta_2: f32,
    rotor_icing_drag_torque_delta_3: f32,
    rotor_icing_drag_torque_delta_4: f32,
    payload_slosh_displacement_x: f32,
    payload_slosh_displacement_y: f32,
}

#[derive(Serialize)]
struct DroneTrajectory {
    trajectory_id: String,
    data: Vec<DroneStepState>,
    proof_hash: String,
    drift_failed: bool,
}

fn run_single_trajectory(index: usize, seed: u64, n_drones: usize, scenario: &str) -> DroneTrajectory {
    let mut rng = Rng::new(seed);
    
    let is_jammed = if scenario == "high_winds" && rng.chance(0.60) {
        true
    } else if scenario != "nominal" && rng.chance(0.25) {
        true
    } else {
        false
    };

    // Sweep EW Jamming power per trajectory (in dBm: -100.0 is noise floor, -30.0 is severe jamming)
    let ew_jamming_dbm = if is_jammed {
        rng.range(-65.0, -30.0) as f32
    } else {
        rng.range(-100.0, -85.0) as f32
    };

    // GPS Spoofing Vector sweep (adversarial injection bias in meters)
    let has_gps_spoofing = scenario != "nominal" && rng.chance(0.40);
    let gps_spoof_bias_x = if has_gps_spoofing { rng.range(-15.0, 15.0) as f32 } else { 0.0f32 };
    let gps_spo_bias_y = if has_gps_spoofing { rng.range(-15.0, 15.0) as f32 } else { 0.0f32 };
    let gps_spo_bias_z = if has_gps_spoofing { rng.range(-5.0, 10.0) as f32 } else { 0.0f32 };
    
    // Canopy density parameter swept per trajectory (for wind-shear coupling)
    let canopy_density = rng.range(0.3, 0.9) as f32;

    // Swept per-rotor icing severity for asymmetric lift degradation
    let mut rotor_icing_severities = [0.0f32; 4];
    if scenario == "high_winds" || scenario == "vortex_ring_state" {
        for i in 0..4 {
            let asymmetry_factor = if i % 2 == 0 { 1.2f32 } else { 0.8f32 };
            rotor_icing_severities[i] = (rng.range(0.1, 0.8) as f32 * asymmetry_factor).clamp(0.0f32, 1.0f32);
        }
    } else if scenario != "nominal" {
        for i in 0..4 {
            rotor_icing_severities[i] = rng.range(0.0, 0.4) as f32;
        }
    }

    
    // Confined space parameter for acoustic resonance
    let wall_proximity = if scenario == "vortex_ring_state" && rng.chance(0.50) {
        rng.range(0.5, 2.5) as f32
    } else if scenario == "nominal" {
        5.0f32
    } else {
        rng.range(1.0, 5.0) as f32
    };

    // Scenario-based mass and moments of inertia (with continuous sweeps as promised)
    let (mass, i_xx, i_yy, i_zz) = match scenario {
        "heavy_payload" => {
            let m = rng.range(8.0, 35.0) as f32; // starts slightly heavier
            (m, 0.02f32 * (m / 2.0), 0.02f32 * (m / 2.0), 0.04f32 * (m / 2.0))
        },
        "nominal" | "high_winds" | "vortex_ring_state" | "sweep" => {
            let m = rng.range(1.5, 5.0) as f32;
            (m, 0.02f32 * (m / 2.0), 0.02f32 * (m / 2.0), 0.04f32 * (m / 2.0))
        },
        _ => {
            let m = 2.0f32;
            (m, 0.02f32, 0.02f32, 0.04f32)
        }
    };

    #[allow(unused_assignments)]
    let mut current_i_xx = i_xx;
    #[allow(unused_assignments)]
    let mut current_i_yy = i_yy;
    #[allow(unused_assignments)]
    let mut current_i_zz = i_zz;
    #[allow(unused_assignments)]
    let mut current_i_xy = 0.0f32;
    #[allow(unused_assignments)]
    let mut current_i_yz = 0.0f32;
    #[allow(unused_assignments)]
    let mut current_i_xz = 0.0f32;

    // Grid coordinate placement
    let side = (n_drones as f32).sqrt().floor() as usize;
    let side = if side == 0 { 1 } else { side };
    let row = index / side;
    let col = index % side;
    let max_idx = (side - 1) as f32;
    let x_0 = if side > 1 { -2.0 + (col as f32 / max_idx) * 4.0 } else { 0.0 };
    let y_0 = if side > 1 { -2.0 + (row as f32 / max_idx) * 4.0 } else { 0.0 };

    // Initial states
    let mut pos = [x_0, y_0, 4.0f32]; // Hover height starting at 4.0 m
    let start_vel_z = match scenario {
        "vortex_ring_state" => rng.range(-7.0, -1.0) as f32,
        "nominal" | "heavy_payload" | "high_winds" => -1.0f32,
        _ => if rng.chance(0.20) { rng.range(-7.0, -1.0) as f32 } else { -1.0f32 }, // fallback
    };
    let mut vel = [0.0f32, 0.0f32, start_vel_z];
    let mut quat = [1.0f32, 0.0f32, 0.0f32, 0.0f32];
    let mut ang_vel = [0.0f32; 3];
    
    let dt = 0.001f32;
    let total_time = 5.0f32;
    let steps_count = (total_time / dt) as usize;
    
    let mut states = Vec::with_capacity(steps_count / 10);
    
    let mut running_hash = Sha256::new();
    running_hash.update(&seed.to_le_bytes());
    let mut last_hash = running_hash.finalize();
    
    let mut max_drift = 0.0f32;
    
    // Environment wind parameters (Sim-to-Real mismatch: estimator assumes stationary sea-level density 1.225)
    let rho_est = 1.225f32;
    let rho_actual = if scenario == "nominal" {
        rng.range(0.95, 1.25) as f32
    } else {
        1.10f32
    };
    
    // Initialize motor RPMs to nominal hover speed (hover thrust per rotor = mass * G / 4.0)
    let hover_thrust = (mass * G) / 4.0f32;
    let max_thrust = mass * 5.0f32;
    let hover_rpm = 10000.0f32 * (hover_thrust / max_thrust).sqrt();
    let mut motor_rpms = [hover_rpm; 4];
    let mut thrusts = [hover_thrust; 4];
    let mut consecutive_fail_steps = 0;

    // Cross-Scenario Additions
    let t_onset = rng.range(0.2, 0.8) as f32; // randomized wind shear trigger time
    let mut motor_temps = [0.0f32; 4]; // motor temperature state
    let mut thermal_accumulated = 0.0f32;
    let mut pos_est = pos; // EKF position estimation
    let mut vel_est = vel; // EKF velocity estimation
    let mut ekf_p = [[0.1f32, 0.0f32, 0.0f32, 0.1f32]; 3]; // State covariance for 3-axis KF
    let mut cog_offset = [0.0f32; 3]; // Center of Gravity offset
    let (control_delay_base, kp_att_base) = if scenario == "pio_resonance" {
        (rng.range(20.0, 120.0) as f32, rng.range(10.0, 90.0) as f32)
    } else if scenario == "heavy_payload" {
        (rng.range(35.0, 55.0) as f32, 30.0f32)
    } else {
        (rng.range(10.0, 30.0) as f32, 30.0f32)
    };
    #[allow(unused_assignments)]
    let mut control_delay = control_delay_base;
    let mut cmd_history: VecDeque<[f32; 4]> = VecDeque::new(); // History of commanded thrusts
    
    // Initialize Atheric Link System
    let mut atheric_link = AthericSystem::new(
        8, // 8 channels
        dbm_to_watts(20.0), // 20 dBm tx power (100mW)
        -120.0, // -120 dBm noise floor
        4.0 / 1000.0, // initial distance is 4.0m (0.004 km)
    );
    let mut hop_seed = [0u8; 32];
    hop_seed[..8].copy_from_slice(&seed.to_le_bytes());
    atheric_link.hop_seed = hop_seed;
    let mut clock_drift = 0.0f32;
    let mut rolling_packets: VecDeque<bool> = VecDeque::with_capacity(100);
    for _ in 0..100 {
        rolling_packets.push_back(true);
    }
    let mut last_valid_thrust_cmds = [hover_thrust; 4];
    
    // Pick-a-Part dynamic states
    let mut is_in_pio = false;
    let mut pio_amplitude_rad = 0.0f32;
    let mut pitch_rate_error_history: VecDeque<(f32, f32)> = VecDeque::new(); // stores (time, pitch_rate_error)
    let mut rotor_icing_drag_torque_delta = [0.0f32; 4];
    let mut payload_slosh_displacement_x = 0.0f32;
    let mut payload_slosh_displacement_y = 0.0f32;
    let mut payload_slosh_vx = 0.0f32;
    let mut payload_slosh_vy = 0.0f32;
    let mut acc_x_prev = 0.0f32;
    let mut acc_y_prev = 0.0f32;
    
    for step in 0..steps_count {
        let t = step as f32 * dt;
        
        // 1. Aerodynamic Icing Accumulation over time per rotor
        let mut current_icing_severities = [0.0f32; 4];
        for i in 0..4 {
            current_icing_severities[i] = if t >= t_onset {
                (rotor_icing_severities[i] * (t - t_onset) / (total_time - t_onset)).clamp(0.0f32, 1.0f32)
            } else {
                0.0f32
            };
        }
        let current_icing = (current_icing_severities[0] + current_icing_severities[1] + 
                             current_icing_severities[2] + current_icing_severities[3]) / 4.0f32;

        // 2. Confined Space Vibro-Acoustic IMU Resonance physically derived
        let propeller_rpm = motor_rpms[0] + motor_rpms[1] + motor_rpms[2] + motor_rpms[3];
        let motor_rpm_average = propeller_rpm / 4.0f32;
        let f_blade_pass = (motor_rpm_average / 60.0f32) * 3.0f32; // 3-blade propellers
        
        // Proximity factor: reverberant surfaces amplify resonance
        let proximity_factor = if wall_proximity < 4.0f32 { (4.0f32 - wall_proximity) / 4.0f32 } else { 0.0f32 };
        
        // MEMS accelerometer tuning fork resonance at 25 kHz structural eigenmode excited by blade-pass harmonics
        let mems_eigenmode = 25000.0f32;
        let harmonic_n = (mems_eigenmode / f_blade_pass).round();
        let frequency_deviation = (mems_eigenmode - harmonic_n * f_blade_pass).abs();
        let mems_resonance_gain = (100.0f32 / (1.0f32 + 0.05f32 * frequency_deviation.powi(2))).max(1.0f32);
        
        let acoustic_resonance_g = if pos[2] <= 12.0f32 && wall_proximity < 4.0f32 {
            0.08f32 * mems_resonance_gain * proximity_factor * (1.0f32 + 0.2f32 * (100.0f32 * t).sin())
        } else {
            0.0f32
        };
        
        // ─── Atheric Communication Link Dynamic Propagation ───
        // 1. Distance-based attenuation (Friis path loss) relative to home base
        let dist_m = ((pos[0] - x_0).powi(2) + (pos[1] - y_0).powi(2) + pos[2].powi(2)).sqrt();
        let dist_km = (dist_m / 1000.0f32) as f64;
        let tx_watts = dbm_to_watts(20.0);
        for (i, c) in atheric_link.channels.iter_mut().enumerate() {
            let freq = genesis_core::physics::atheric::BASE_FREQUENCY * (i + 1) as f64;
            let rx = genesis_core::physics::atheric::free_space_received(tx_watts, freq, dist_km);
            c.signal_power = rx;
            c.jammed = false;
        }

        // 2. Crystal oscillator clock drift driven by motor temperature (cubic temperature curve) and acoustic vibration
        let mean_motor_temp = (motor_temps[0] + motor_temps[1] + motor_temps[2] + motor_temps[3]) / 4.0f32;
        let t_osc = (25.0f32 + 0.02f32 * mean_motor_temp).min(105.0f32); // FC board TCXO temperature capped at 105C physical operating limit
        let t_diff = t_osc - 25.0f32; // deviation from 25C reference
        let temp_drift_rate = 1.0e-7 * t_diff + 2.0e-11 * t_diff.powi(3); // cubic oscillator frequency curve (Bechmann polynomial)
        let vib_drift_rate = 1.0e-8 * acoustic_resonance_g; // 10 ppb per g of vibration
        let delta_drift = (temp_drift_rate + vib_drift_rate) * dt;
        clock_drift += delta_drift;

        if clock_drift > 1e-4 {
            atheric_link.apply_clock_drift();
        }

        // 3. Apply Electronic Warfare jamming (broadband and narrowband)
        if is_jammed {
            let noise_increase = dbm_to_watts(ew_jamming_dbm as f64) / dbm_to_watts(-120.0);
            atheric_link.apply_broadband(noise_increase);
            if ew_jamming_dbm > -60.0 {
                atheric_link.apply_jamming(3, &mut rng);
            }
        }

        // 4. Packet transmission check (Shannon threshold, frequency hopping desync, and canopy multipath Rayleigh/Rician fading)
        let (packet_ok, _chan_idx, _snr) = atheric_link.transmit_packet(3.0);
        
        let canopy_depth = (10.0f32 - pos[2]).max(0.0f32);
        let is_fading = canopy_depth > 0.0f32 && canopy_density > 0.1f32;
        let fading_factor = if is_fading {
            let rayleigh_noise = rng.range(0.1, 1.0) as f32; // Rayleigh fading approximation
            (1.0f32 - (1.0f32 - rayleigh_noise) * (1.0f32 - (-canopy_density * canopy_depth / 4.0f32).exp())).clamp(0.05f32, 1.0f32)
        } else {
            1.0f32
        };
        
        let packet_success = packet_ok && !atheric_link.desync && (rng.range(0.0, 1.0) as f32 <= fading_factor);

        rolling_packets.push_back(packet_success);
        if rolling_packets.len() > 100 {
            rolling_packets.pop_front();
        }
        let success_count = rolling_packets.iter().filter(|&&ok| ok).count();
        let rolling_success_rate = success_count as f32 / 100.0f32;

        // 5. Dynamic control delay inflation based on Atheric link quality
        control_delay = control_delay_base + (1.0f32 - rolling_success_rate) * 140.0f32;
        
        
        // Canopy wind shielding / exposure model
        // Above the canopy (z >= 10m), the drone is fully exposed to horizontal winds.
        // Below the canopy (z < 10m), the canopy branches and foliage shield the drone,
        // exponentially reducing the horizontal wind speed based on canopy density.
        let wind_exposure = if pos[2] >= 10.0f32 {
            1.0f32
        } else {
            (-canopy_density * (10.0f32 - pos[2]).max(0.0f32) / 2.0f32).exp()
        };

        // Wind-Shear entry (canopy turbulence street) - decoupled from ground cohesion, coupled to canopy density
        let omega_wind = 1.0f32 * std::f32::consts::PI;
        let canopy_wind_coupling = 1.0f32 + 2.5f32 * canopy_density;
        // Canopy sway coupling: if the drone is at or above canopy level (z >= 10.0m),
        // wind shear and turbulence are dynamically modulated by canopy density
        let canopy_sway_coupling = if pos[2] >= 10.0f32 {
            canopy_wind_coupling
        } else {
            1.0f32
        };
        let wind_scale = if scenario == "nominal" { 0.20f32 } else { 1.0f32 };
        let shear_xy = 5.0f32 * (omega_wind * t).sin() * canopy_sway_coupling * wind_scale;
        let shear_xz = 3.0f32 * (omega_wind * t).cos() * canopy_sway_coupling * wind_scale;
        let shear_yz = 4.0f32 * (omega_wind * t).sin() * canopy_sway_coupling * wind_scale;
        
        // Horizontal winds are shielded under the canopy, vertical shear is less shielded
        let mut wind_vx = (6.0f32 * (omega_wind * t).sin() * canopy_sway_coupling * wind_scale + pos[1] * shear_xy) * wind_exposure;
        let mut wind_vy = (6.0f32 * (omega_wind * t).cos() * canopy_sway_coupling * wind_scale + pos[2] * shear_yz) * wind_exposure;
        let mut wind_vz = (2.0f32 * (omega_wind * t + 0.5f32).sin() * canopy_sway_coupling * wind_scale + pos[0] * shear_xz) * wind_scale;
        
        // Sudden micro-burst vortex under the canopy (randomized trigger onset time)
        // A downdraft entering a canopy gap can accelerate downwards, creating a severe localized vertical force
        if scenario != "nominal" && t >= t_onset {
            let t_active = t - t_onset;
            wind_vz = -15.0f32 * (2.0f32 * t_active).min(1.0f32) * canopy_wind_coupling;
            wind_vx += 8.0f32 * (omega_wind * t).sin() * wind_exposure;
            wind_vy += 8.0f32 * (omega_wind * t).cos() * wind_exposure;
        }
        
        if scenario == "high_winds" {
            // Scale wind velocities up to severe gale force boundary conditions (35 m/s ~ 78 mph)
            wind_vx = (wind_vx * 6.5f32).clamp(-35.0f32, 35.0f32);
            wind_vy = (wind_vy * 6.5f32).clamp(-35.0f32, 35.0f32);
            wind_vz = (wind_vz * 6.5f32).clamp(-35.0f32, 35.0f32);
        } else if scenario == "heavy_payload" {
            // Heavy payload drones deploy cargo/dispensers under moderate wind shear (up to 20 m/s)
            wind_vx = (wind_vx * 3.2f32).clamp(-20.0f32, 20.0f32);
            wind_vy = (wind_vy * 3.2f32).clamp(-20.0f32, 20.0f32);
            wind_vz = (wind_vz * 3.2f32).clamp(-20.0f32, 20.0f32);
        }
        
        let wind_velocity = [wind_vx, wind_vy, wind_vz];
        
        // Dynamic Hover Induced Velocity: vi = sqrt( Thrust / (2 * rho * Area) )
        let rotor_area = 0.785f32; // total rotor area
        let hover_induced_vel = ((mass * G) / (2.0f32 * rho_actual * rotor_area).max(0.1f32)).sqrt();

        // Continuous VRS model using Glauert empirical curve drop-off (down to 10% efficiency)
        let descent_rate = -vel[2];
        let hover_induced_vel_val = hover_induced_vel;
        let velocity_ratio = descent_rate / hover_induced_vel_val.max(0.1f32);
        let is_in_vrs = velocity_ratio > 0.5f32;
        let vrs_factor = if velocity_ratio > 0.5f32 && velocity_ratio < 1.25f32 {
            let vrs_penetration = (velocity_ratio - 0.5f32) / (1.25f32 - 0.5f32);
            1.0f32 - (vrs_penetration * 0.90f32)
        } else if velocity_ratio >= 1.25f32 {
            0.10f32
        } else {
            1.0f32
        };
        // Sigmoid mapping for dynamic air density drop in vortex turbulent wake
        let vrs_width = 0.25f32 * hover_induced_vel_val;
        let vrs_sigmoid = 1.0f32 / (1.0f32 + (-(descent_rate - 0.5f32 * hover_induced_vel_val) / vrs_width).exp());
        let mut dynamic_rho = rho_actual - (rho_actual - 0.95f32) * vrs_sigmoid;
        
        let vrs_onset = 0.50f32 * hover_induced_vel_val;
        let vrs_strength = (vel[2].abs() - vrs_onset).max(0.0f32);
        let vrs_moment_x = 0.15f32 * (18.0f32 * t).sin() * vrs_strength * vrs_sigmoid;
        let vrs_moment_y = 0.20f32 * (15.0f32 * t).cos() * vrs_strength * vrs_sigmoid;
        
        // Coupling rotor downwash with ground cushion aerodynamics and dust obscuration
        let mut dust_drag_z = 0.0f32;
        let mut dust_obscuration = 0.0f32;
        if pos[2] > 0.0f32 && pos[2] < 3.0f32 {
            // Ground cushion effect: increases local air density near the ground due to pressure buildup
            // Decays quadratically with altitude: 1 + 0.15 * (1 - z/3)^2
            let ground_cushion = 1.0f32 + 0.15f32 * (1.0f32 - pos[2] / 3.0f32).powi(2);
            dynamic_rho = rho_actual * ground_cushion;

            // Downwash dynamic pressure creates dust cloud on dry soil/sand surfaces
            let v_induced = ((thrusts[0] + thrusts[1] + thrusts[2] + thrusts[3]) / (2.0f32 * rho_actual * rotor_area).max(0.1f32)).sqrt();
            let v_downwash = v_induced * (-pos[2] / 0.5f32).exp(); // broader decay rate for jet expansion
            
            // Obscuration scales with downwash velocity at the ground
            dust_obscuration = (v_downwash / 15.0f32).min(1.0f32);
            dust_drag_z = 0.05f32 * dust_obscuration * vel[2].abs();
        }
        
        let rel_vx = vel[0] - wind_vx;
        let rel_vy = vel[1] - wind_vy;
        let rel_vz = vel[2] - wind_vz;
        
        // 2. Flight Attitude Controller (estimates rho_est = 1.225)
        let roll = (2.0f32 * (quat[0] * quat[1] + quat[2] * quat[3])).atan2(1.0f32 - 2.0f32 * (quat[1].powi(2) + quat[2].powi(2)));
        let pitch = (2.0f32 * (quat[0] * quat[2] - quat[3] * quat[1])).asin().clamp(-1.5f32, 1.5f32);
        let yaw = (2.0f32 * (quat[0] * quat[3] + quat[1] * quat[2])).atan2(1.0f32 - 2.0f32 * (quat[2].powi(2) + quat[3].powi(2)));
        
        // Drag increases with tilt angle (pitch and roll deviations) representing attitude-dependent frontal surface area
        let angle_of_attack = (pitch.abs() + roll.abs()).clamp(0.0f32, 1.5f32);
        let cd = 0.15f32 + 0.35f32 * angle_of_attack.sin();
        let drag_x = -0.5f32 * cd * dynamic_rho * 0.08f32 * rel_vx * rel_vx.abs();
        let drag_y = -0.5f32 * cd * dynamic_rho * 0.08f32 * rel_vy * rel_vy.abs();
        let drag_z = -0.5f32 * cd * dynamic_rho * 0.08f32 * rel_vz * rel_vz.abs() - dust_drag_z * vel[2].signum();
        
        // Position feedback loops - Closed-loop control using EKF estimated state rather than ground truth
        let kp_pos = 12.0f32;
        let kd_pos = 4.0f32;
        let target_ax = kp_pos * (x_0 - pos_est[0]) - kd_pos * vel_est[0];
        let target_ay = kp_pos * (y_0 - pos_est[1]) - kd_pos * vel_est[1];
        
        // Pitch/roll angle targets to drive horizontal position
        let target_pitch = (target_ax / G).clamp(-0.4f32, 0.4f32);
        let target_roll = (-target_ay / G).clamp(-0.4f32, 0.4f32);
        
        // Attitude rates
        let kp_att = kp_att_base;
        let kd_att = 8.0f32;
        let mut u_roll = kp_att * (target_roll - roll) - kd_att * ang_vel[0];
        let mut u_pitch = kp_att * (target_pitch - pitch) - kd_att * ang_vel[1];
        
        // PIO Detection Loop
        let pitch_rate_error = target_pitch - pitch;
        pitch_rate_error_history.push_back((t, pitch_rate_error));
        while pitch_rate_error_history.front().map_or(false, |&(time, _)| t - time > 0.200f32) {
            pitch_rate_error_history.pop_front();
        }
        let mut zero_crossings = 0;
        let mut history_iter = pitch_rate_error_history.iter();
        if let Some(&(_, mut prev_val)) = history_iter.next() {
            for &(_, val) in history_iter {
                if (prev_val < 0.0f32 && val >= 0.0f32) || (prev_val >= 0.0f32 && val < 0.0f32) {
                    zero_crossings += 1;
                }
                prev_val = val;
            }
        }
        if control_delay > 55.0f32 && zero_crossings >= 3 {
            is_in_pio = true;
            pio_amplitude_rad = (pio_amplitude_rad + pitch_rate_error.abs() * dt).min(0.8f32);
        } else {
            is_in_pio = false;
            pio_amplitude_rad = (pio_amplitude_rad - 0.200f32 * dt).max(0.0f32);
        }
        if is_in_pio {
            let f_pio = 1.0f32 / (4.0f32 * (control_delay * 0.001f32)).max(0.01f32);
            u_pitch += pio_amplitude_rad * (2.0f32 * std::f32::consts::PI * f_pio * t).sin() * kp_att;
        }
        
        let u_yaw = kp_att * (0.0f32 - yaw) - kd_att * ang_vel[2];
        
        // Altitude loop: target circular flight inside the forest canopy (z = 20.0m)
        let target_z = 20.0f32;
        let kp_z = 25.0f32;
        let kd_z = 8.0f32;
        let mut u_z = kp_z * (target_z - pos_est[2]) - kd_z * vel_est[2] + mass * G;
        
        // Active WBC wind shear detector
        // Expected vertical force vs measured acceleration
        let expected_f_z = u_z / (dynamic_rho / rho_est);
        let _measured_a_z = (drag_z + expected_f_z) / mass - G;
        let vertical_deviation = (vel_est[2] - 0.0f32).abs();
        
        let wind_shear_detected = vertical_deviation > 0.5f32 || (pos_est[2] - target_z).abs() > 3.0f32;
        
        // WBC Recovery Loop: boost thrust and increase attitude gains to counteract vortex
        if wind_shear_detected {
            u_z += 20.0f32; // Boost thrust output
            u_roll *= 2.5f32; // Double roll stiffness
            u_pitch *= 2.5f32; // Double pitch stiffness
        }
        
        // Motor thrust allocation commands
        let thrust_cmds = [
            0.25f32 * u_z + 0.25f32 * u_pitch / ARM_LEN + 0.25f32 * u_yaw / C_M,
            0.25f32 * u_z - 0.25f32 * u_roll / ARM_LEN - 0.25f32 * u_yaw / C_M,
            0.25f32 * u_z - 0.25f32 * u_pitch / ARM_LEN + 0.25f32 * u_yaw / C_M,
            0.25f32 * u_z + 0.25f32 * u_roll / ARM_LEN - 0.25f32 * u_yaw / C_M,
        ];

        // If the Atheric link was successful, accept new command. Otherwise, repeat the last successfully received command.
        if packet_success {
            last_valid_thrust_cmds = thrust_cmds;
        }

        // Push commands to history ring buffer for control delay simulation
        cmd_history.push_back(last_valid_thrust_cmds);
        if cmd_history.len() > 160 {
            cmd_history.pop_front();
        }
        let delay_steps = (control_delay).round() as usize;
        let active_delay_steps = delay_steps.clamp(1, 150);
        let delayed_thrust_cmds = if cmd_history.len() >= active_delay_steps {
            cmd_history[cmd_history.len() - active_delay_steps]
        } else {
            last_valid_thrust_cmds
        };
        
        // Battery Voltage Sag
        let thermal_heating_factor = 0.05f32;
        let thermal_cooling_factor = 0.02f32;
        let temp_limit = 500.0f32;

        let total_thrust_cmd = delayed_thrust_cmds[0] + delayed_thrust_cmds[1] + delayed_thrust_cmds[2] + delayed_thrust_cmds[3];
        let r_internal = 0.005f32;
        let k_temp = 0.0001f32;
        let voltage_sag = (total_thrust_cmd * r_internal + k_temp * thermal_accumulated).min(4.0f32); // max 4V drop
        let voltage_sag_factor = (1.0f32 - voltage_sag / 16.8f32).max(0.6f32); // nominal 4S LiPo is 16.8V

        thrusts = [0.0f32; 4];
        for i in 0..4 {
            let cmd_thrust = delayed_thrust_cmds[i];
            
            // Temperature rise
            let temp_rise = (cmd_thrust.powi(2) * thermal_heating_factor - thermal_cooling_factor * motor_temps[i]) * dt;
            motor_temps[i] = (motor_temps[i] + temp_rise).max(0.0f32);
            
            // Individual rotor lift loss due to asymmetric icing
            let lift_loss_factor_i = 1.0f32 - 0.20f32 * current_icing_severities[i];

            // Apply thermal, ice, and voltage sag limits
            let max_thrust_i = if motor_temps[i] > temp_limit {
                let factor = (1.0f32 - 0.001f32 * (motor_temps[i] - temp_limit)).max(0.2f32);
                max_thrust * factor * lift_loss_factor_i * voltage_sag_factor
            } else {
                max_thrust * lift_loss_factor_i * voltage_sag_factor
            };
            
            let cmd_thrust_clamped = cmd_thrust.clamp(0.0f32, max_thrust_i);
            // Motor back-EMF is proportional to speed (10000 RPM corresponds to ~3.0V back-EMF)
            let back_emf = 0.0003f32 * motor_rpms[i];
            let max_available_voltage = (16.8f32 - voltage_sag).max(5.0f32);
            let effective_voltage = (max_available_voltage - back_emf).max(0.0f32);
            // Torque limit reduces motor response bandwidth under voltage sag (back-EMF and torque limits)
            let torque_limit_factor = (effective_voltage / 16.8f32).clamp(0.1f32, 1.0f32);
            let motor_time_constant = 0.015f32 / torque_limit_factor;
            let target_rpm = 10000.0f32 * (cmd_thrust_clamped / max_thrust).sqrt();
            motor_rpms[i] += (target_rpm - motor_rpms[i]) * (dt / motor_time_constant);
            motor_rpms[i] = motor_rpms[i].clamp(0.0f32, 10000.0f32);
            thrusts[i] = max_thrust * (motor_rpms[i] / 10000.0f32).powi(2);
        }
        
        thermal_accumulated = motor_temps[0] + motor_temps[1] + motor_temps[2] + motor_temps[3];

        // Dynamic Payload release & Center of Gravity migration (for heavy_payload scenario)
        let is_deploying_payload = pos[2] < 12.0f32 && scenario == "heavy_payload" && t > t_onset;
        let dynamic_mass = if is_deploying_payload {
            let mass_decay = 0.5f32 * dt;
            (mass - mass_decay).max(1.5f32)
        } else {
            mass
        };

        if is_deploying_payload {
            // Asymmetric CoG shift (shifting left/forward)
            cog_offset[0] = (cog_offset[0] + 0.05f32 * dt).min(0.15f32);
            cog_offset[1] = (cog_offset[1] - 0.03f32 * dt).max(-0.10f32);
            cog_offset[2] = (cog_offset[2] - 0.015f32 * dt).max(-0.05f32);
        }

        // Payload slosh dynamics (coupled pendulum) in X and Y
        let slosh_omega = 1.8f32 * 2.0f32 * std::f32::consts::PI;
        let slosh_stiffness = slosh_omega * slosh_omega;
        let slosh_damping = 0.5f32;
        let slosh_mass_ratio = 0.15f32; // 15% of mass is fluid sloshing
        
        let slosh_accel_x = if scenario == "heavy_payload" {
            -slosh_stiffness * payload_slosh_displacement_x - slosh_damping * payload_slosh_vx - acc_x_prev
        } else {
            0.0f32
        };
        payload_slosh_vx += slosh_accel_x * dt;
        payload_slosh_displacement_x += payload_slosh_vx * dt;
        payload_slosh_displacement_x = payload_slosh_displacement_x.clamp(-0.10f32, 0.10f32);
        
        let slosh_accel_y = if scenario == "heavy_payload" {
            -slosh_stiffness * payload_slosh_displacement_y - slosh_damping * payload_slosh_vy - acc_y_prev
        } else {
            0.0f32
        };
        payload_slosh_vy += slosh_accel_y * dt;
        payload_slosh_displacement_y += payload_slosh_vy * dt;
        payload_slosh_displacement_y = payload_slosh_displacement_y.clamp(-0.10f32, 0.10f32);

        let active_cog_offset_x = cog_offset[0] + slosh_mass_ratio * payload_slosh_displacement_x;
        let active_cog_offset_y = cog_offset[1] + slosh_mass_ratio * payload_slosh_displacement_y;
        let active_cog_offset_z = cog_offset[2];

        // Apply Parallel Axis Theorem to calculate the dynamic inertia tensor
        let mass_ratio = dynamic_mass / mass;
        current_i_xx = i_xx * mass_ratio + dynamic_mass * (active_cog_offset_y.powi(2) + active_cog_offset_z.powi(2));
        current_i_yy = i_yy * mass_ratio + dynamic_mass * (active_cog_offset_x.powi(2) + active_cog_offset_z.powi(2));
        current_i_zz = i_zz * mass_ratio + dynamic_mass * (active_cog_offset_x.powi(2) + active_cog_offset_y.powi(2));
        
        current_i_xy = -dynamic_mass * active_cog_offset_x * active_cog_offset_y;
        current_i_yz = -dynamic_mass * active_cog_offset_y * active_cog_offset_z;
        current_i_xz = -dynamic_mass * active_cog_offset_x * active_cog_offset_z;
        
        // Body forces & moments taking Center of Gravity migration into account
        let f_thrust = (thrusts[0] + thrusts[1] + thrusts[2] + thrusts[3]) * vrs_factor;
        let sum_thrusts = thrusts[0] + thrusts[1] + thrusts[2] + thrusts[3];
        
        // Compute rotor icing drag torque delta
        for i in 0..4 {
            rotor_icing_drag_torque_delta[i] = 0.50f32 * current_icing_severities[i] * thrusts[i] * C_M;
        }

        let m_x = ARM_LEN * (thrusts[3] - thrusts[1]) - active_cog_offset_y * sum_thrusts + vrs_moment_x
            + (rotor_icing_drag_torque_delta[3] - rotor_icing_drag_torque_delta[1]) * 0.1f32;
        let m_y = ARM_LEN * (thrusts[0] - thrusts[2]) + active_cog_offset_x * sum_thrusts + vrs_moment_y
            + (rotor_icing_drag_torque_delta[0] - rotor_icing_drag_torque_delta[2]) * 0.1f32;
        let m_z = C_M * (thrusts[0] - thrusts[1] + thrusts[2] - thrusts[3])
            + (rotor_icing_drag_torque_delta[0] - rotor_icing_drag_torque_delta[1] + rotor_icing_drag_torque_delta[2] - rotor_icing_drag_torque_delta[3]);
        
        // Rotate thrust to world frame (corrected body-to-world mapping)
        let tx = 2.0f32 * (quat[1] * quat[3] + quat[0] * quat[2]);
        let ty = 2.0f32 * (quat[2] * quat[3] - quat[0] * quat[1]);
        let tz = 1.0f32 - 2.0f32 * (quat[1] * quat[1] + quat[2] * quat[2]);
        
        // Apply actual density correction to thrust force
        let actual_f_thrust = f_thrust * (dynamic_rho / rho_est);
        let force_world = [tx * actual_f_thrust, ty * actual_f_thrust, tz * actual_f_thrust];
        
        // Integrate translation
        let acc_x = (drag_x + force_world[0]) / dynamic_mass;
        let acc_y = (drag_y + force_world[1]) / dynamic_mass;
        let acc_z = (drag_z + force_world[2]) / dynamic_mass - G;
        
        acc_x_prev = acc_x;
        acc_y_prev = acc_y;
        
        vel[0] += acc_x * dt;
        vel[1] += acc_y * dt;
        vel[2] += acc_z * dt;
        
        pos[0] += vel[0] * dt;
        pos[1] += vel[1] * dt;
        pos[2] += vel[2] * dt;
        
        // Autopilot EKF Estimator & GPS Multipath/Vibration/Dust degradation
        let is_obscured = pos[2] <= 15.0f32;
        let imu_saturated = acoustic_resonance_g > 8.0f32;
        let mut gps_noise_scale = if imu_saturated {
            5.0f32
        } else if is_obscured {
            if scenario == "nominal" { 0.15f32 } else { 1.5f32 }
        } else {
            0.1f32
        };
        // Dust cloud obscuration blinding erodes landing site visibility and GPS/Optical locks
        gps_noise_scale += 4.5f32 * dust_obscuration;
        let _imu_noise_scale = if wind_shear_detected { 2.0f32 } else { 0.05f32 };
        
        let gps_meas_x = pos[0] + gps_spoof_bias_x + rng.range(-gps_noise_scale as f64, gps_noise_scale as f64) as f32;
        let gps_meas_y = pos[1] + gps_spo_bias_y + rng.range(-gps_noise_scale as f64, gps_noise_scale as f64) as f32;
        let gps_meas_z = pos[2] + gps_spo_bias_z + rng.range(-gps_noise_scale as f64, gps_noise_scale as f64) as f32;
        
        // 3-Axis Independent Kalman Filter (Rigorous position/velocity tracking filter)
        let z_meas = [gps_meas_x, gps_meas_y, gps_meas_z];
        for i in 0..3 {
            // 1. Predict state
            let pos_est_pred = pos_est[i] + vel_est[i] * dt;
            let vel_est_pred = vel_est[i];
 
            // 2. Predict covariance P^- = F * P * F^T + Q
            let p_prev = ekf_p[i];
            let mut p_pred = [0.0f32; 4];
            p_pred[0] = p_prev[0] + (p_prev[1] + p_prev[2]) * dt + p_prev[3] * dt * dt + 0.001f32;
            p_pred[1] = p_prev[1] + p_prev[3] * dt;
            p_pred[2] = p_prev[2] + p_prev[3] * dt;
            p_pred[3] = p_prev[3] + 0.01f32;
 
            // 3. Innovation covariance S = H * P^- * H^T + R = p_pred[0] + R
            let r_cov = gps_noise_scale.powi(2);
            let s_cov = p_pred[0] + r_cov;
            
            if s_cov.abs() > 1e-6 {
                let inv_s = 1.0f32 / s_cov;
                // Kalman Gain K = P^- * H^T * S^-1 = [p_pred[0], p_pred[2]]^T * inv_s
                let k_gain = [p_pred[0] * inv_s, p_pred[2] * inv_s];
 
                // 4. Update state
                let inn = z_meas[i] - pos_est_pred;
                pos_est[i] = pos_est_pred + k_gain[0] * inn;
                vel_est[i] = vel_est_pred + k_gain[1] * inn;
 
                // 5. Update covariance P = (I - K * H) * P^-
                ekf_p[i][0] = (1.0f32 - k_gain[0]) * p_pred[0];
                ekf_p[i][1] = (1.0f32 - k_gain[0]) * p_pred[1];
                ekf_p[i][2] = p_pred[2] - k_gain[1] * p_pred[0];
                ekf_p[i][3] = p_pred[3] - k_gain[1] * p_pred[1];
            } else {
                pos_est[i] = pos_est_pred;
                vel_est[i] = vel_est_pred;
                ekf_p[i] = p_pred;
            }
        }
        
        let gg_ekf_divergence = ((pos[0] - pos_est[0]).powi(2) + 
                                 (pos[1] - pos_est[1]).powi(2) + 
                                 (pos[2] - pos_est[2]).powi(2)).sqrt();
 
        // Rotational dynamics with full 3D inertia tensor cross-coupling (Parallel Axis Theorem)
        // Compute angular momentum vector: h = I * w
        let h_x = current_i_xx * ang_vel[0] + current_i_xy * ang_vel[1] + current_i_xz * ang_vel[2];
        let h_y = current_i_xy * ang_vel[0] + current_i_yy * ang_vel[1] + current_i_yz * ang_vel[2];
        let h_z = current_i_xz * ang_vel[0] + current_i_yz * ang_vel[1] + current_i_zz * ang_vel[2];

        // Compute gyroscopic moment: g = w x h
        let g_x = ang_vel[1] * h_z - ang_vel[2] * h_y;
        let g_y = ang_vel[2] * h_x - ang_vel[0] * h_z;
        let g_z = ang_vel[0] * h_y - ang_vel[1] * h_x;

        // Net moment: M_net = M - g
        let m_net_x = m_x - g_x;
        let m_net_y = m_y - g_y;
        let m_net_z = m_z - g_z;

        // Invert symmetric 3x3 inertia matrix algebraically
        let det_i = current_i_xx * (current_i_yy * current_i_zz - current_i_yz.powi(2)) -
                    current_i_xy * (current_i_xy * current_i_zz - current_i_yz * current_i_xz) +
                    current_i_xz * (current_i_xy * current_i_yz - current_i_yy * current_i_xz);

        let (ang_accel_x, ang_accel_y, ang_accel_z) = if det_i.abs() > 1e-9f32 {
            let adj_xx = current_i_yy * current_i_zz - current_i_yz.powi(2);
            let adj_xy = current_i_yz * current_i_xz - current_i_xy * current_i_zz;
            let adj_xz = current_i_xy * current_i_yz - current_i_yy * current_i_xz;
            let adj_yy = current_i_xx * current_i_zz - current_i_xz.powi(2);
            let adj_yz = current_i_xy * current_i_xz - current_i_xx * current_i_yz;
            let adj_zz = current_i_xx * current_i_yy - current_i_xy.powi(2);

            let ax = (adj_xx * m_net_x + adj_xy * m_net_y + adj_xz * m_net_z) / det_i;
            let ay = (adj_xy * m_net_x + adj_yy * m_net_y + adj_yz * m_net_z) / det_i;
            let az = (adj_xz * m_net_x + adj_yz * m_net_y + adj_zz * m_net_z) / det_i;
            (ax, ay, az)
        } else {
            // Fallback to uncoupled Euler equations if singular
            let ax = m_net_x / current_i_xx;
            let ay = m_net_y / current_i_yy;
            let az = m_net_z / current_i_zz;
            (ax, ay, az)
        };

        ang_vel[0] += ang_accel_x * dt;
        ang_vel[1] += ang_accel_y * dt;
        ang_vel[2] += ang_accel_z * dt;
        
        // Quaternion derivative
        let qw = quat[0]; let qx = quat[1]; let qy = quat[2]; let qz = quat[3];
        let wx = ang_vel[0]; let wy = ang_vel[1]; let wz = ang_vel[2];
        let dq_w = 0.5f32 * (-qx * wx - qy * wy - qz * wz);
        let dq_x = 0.5f32 * (qw * wx + qy * wz - qz * wy);
        let dq_y = 0.5f32 * (qw * wy - qx * wz + qz * wx);
        let dq_z = 0.5f32 * (qw * wz + qx * wy - qy * wx);
        quat[0] += dq_w * dt;
        quat[1] += dq_x * dt;
        quat[2] += dq_y * dt;
        quat[3] += dq_z * dt;
        
        let q_len = (quat[0].powi(2) + quat[1].powi(2) + quat[2].powi(2) + quat[3].powi(2)).sqrt();
        if q_len > 0.0f32 {
            quat[0] /= q_len; quat[1] /= q_len; quat[2] /= q_len; quat[3] /= q_len;
        }
        
        let lateral_drift = ((pos[0] - x_0).powi(2) + (pos[1] - y_0).powi(2)).sqrt();
        if lateral_drift > max_drift {
            max_drift = lateral_drift;
        }
        
        if lateral_drift > 0.80f32 {
            consecutive_fail_steps += 1;
        } else {
            consecutive_fail_steps = 0;
        }
        
        let is_logging_step = step % 10 == 0;
        let is_terminal_step = step == steps_count - 1 || pos[2] <= 0.0f32 || consecutive_fail_steps >= 10;
 
        if is_logging_step || is_terminal_step {
            // Memory alignment representation of physical state (DroneDynamicsState)
            let dynamics_state = DroneDynamicsState {
                timestamp: t,
                pos,
                vel,
                quat,
                ang_vel,
                motor_torques: thrusts,
                inertia_tensor: [current_i_xx, current_i_yy, current_i_zz],
                wind_velocity,
                thermal_accumulated,
                gg_ekf_divergence,
                cog_offset,
                control_delay,
                icing_severity: current_icing,
                acoustic_resonance_g,
            };
            
            // Cast struct to raw bytes for cryptographic hashing (ZTP local registers proof chain)
            let bytes: &[u8] = unsafe {
                std::slice::from_raw_parts(
                    &dynamics_state as *const DroneDynamicsState as *const u8,
                    std::mem::size_of::<DroneDynamicsState>()
                )
            };
             let mut hasher = Sha256::new();
            hasher.update(&last_hash);
            hasher.update(bytes);
            hasher.update(&[is_in_pio as u8]);
            hasher.update(&pio_amplitude_rad.to_le_bytes());
            hasher.update(&rotor_icing_drag_torque_delta[0].to_le_bytes());
            hasher.update(&rotor_icing_drag_torque_delta[1].to_le_bytes());
            hasher.update(&rotor_icing_drag_torque_delta[2].to_le_bytes());
            hasher.update(&rotor_icing_drag_torque_delta[3].to_le_bytes());
            hasher.update(&payload_slosh_displacement_x.to_le_bytes());
            hasher.update(&payload_slosh_displacement_y.to_le_bytes());
            
            last_hash = hasher.finalize();
            let sha256_seal = hex::encode(last_hash);
 
             states.push(DroneStepState {
                timestamp: t,
                drone_pos_x: pos[0],
                drone_pos_y: pos[1],
                drone_pos_z: pos[2],
                drone_vel_x: vel[0],
                drone_vel_y: vel[1],
                drone_vel_z: vel[2],
                quat_w: quat[0],
                quat_x: quat[1],
                quat_y: quat[2],
                quat_z: quat[3],
                ang_vel_x: ang_vel[0],
                ang_vel_y: ang_vel[1],
                ang_vel_z: ang_vel[2],
                wind_velocity_x: wind_velocity[0],
                wind_velocity_y: wind_velocity[1],
                wind_velocity_z: wind_velocity[2],
                rotor_thrust_1: thrusts[0],
                rotor_thrust_2: thrusts[1],
                rotor_thrust_3: thrusts[2],
                rotor_thrust_4: thrusts[3],
                air_density_rho: dynamic_rho,
                atheric_coherence: atheric_link.coherence(3.0) as f32,
                lateral_position_drift_m: lateral_drift,
                is_in_vrs,
                scenario: scenario.to_string(),
                sha256_seal,
                thermal_accumulated,
                gg_ekf_divergence,
                cog_offset_x: cog_offset[0],
                cog_offset_y: cog_offset[1],
                cog_offset_z: cog_offset[2],
                control_delay,
                inertia_tensor_xx: current_i_xx,
                inertia_tensor_yy: current_i_yy,
                inertia_tensor_zz: current_i_zz,
                inertia_tensor_xy: current_i_xy,
                inertia_tensor_yz: current_i_yz,
                inertia_tensor_xz: current_i_xz,
                rotor_icing_1: current_icing_severities[0],
                rotor_icing_2: current_icing_severities[1],
                rotor_icing_3: current_icing_severities[2],
                rotor_icing_4: current_icing_severities[3],
                acoustic_resonance_g,
                is_jammed,
                is_spoofed: has_gps_spoofing,
                ew_jamming_dbm,
                gps_spoofing_bias_x: gps_spoof_bias_x,
                gps_spoofing_bias_y: gps_spo_bias_y,
                gps_spoofing_bias_z: gps_spo_bias_z,
                voltage_sag,
                atheric_capacity: atheric_link.total_capacity() as f32,
                clock_drift,
                is_in_pio,
                pio_amplitude_rad,
                rotor_icing_drag_torque_delta_1: rotor_icing_drag_torque_delta[0],
                rotor_icing_drag_torque_delta_2: rotor_icing_drag_torque_delta[1],
                rotor_icing_drag_torque_delta_3: rotor_icing_drag_torque_delta[2],
                rotor_icing_drag_torque_delta_4: rotor_icing_drag_torque_delta[3],
                payload_slosh_displacement_x,
                payload_slosh_displacement_y,
            });
        }
        
        if pos[2] <= 0.0f32 || consecutive_fail_steps >= 10 {
            break;
        }
    }
    
    let proof_hash = hex::encode(last_hash);
    let drift_failed = max_drift > 0.80f32 || pos[2] <= 0.0f32; // failure envelope
    
    DroneTrajectory {
        trajectory_id: format!("pg_drn_{:05x}", index),
        data: states,
        proof_hash,
        drift_failed,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n_drones: usize = args.get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10000);
        
    let out_path = args.iter().position(|a| a == "--out")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/products/drones_adversarial_contested_dynamics.parquet").to_string());

    let scenario = args.iter().position(|a| a == "--scenario")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "nominal".to_string());
        
    eprintln!("Generating {} Drone trajectories to Parquet...", n_drones);
    let start = Instant::now();

    // Define Arrow schema
    let schema = Arc::new(Schema::new(vec![
        Field::new("timestamp", DataType::Float32, false),
        Field::new("drone_pos_x", DataType::Float32, false),
        Field::new("drone_pos_y", DataType::Float32, false),
        Field::new("drone_pos_z", DataType::Float32, false),
        Field::new("drone_vel_x", DataType::Float32, false),
        Field::new("drone_vel_y", DataType::Float32, false),
        Field::new("drone_vel_z", DataType::Float32, false),
        Field::new("quat_w", DataType::Float32, false),
        Field::new("quat_x", DataType::Float32, false),
        Field::new("quat_y", DataType::Float32, false),
        Field::new("quat_z", DataType::Float32, false),
        Field::new("ang_vel_x", DataType::Float32, false),
        Field::new("ang_vel_y", DataType::Float32, false),
        Field::new("ang_vel_z", DataType::Float32, false),
        Field::new("wind_velocity_x", DataType::Float32, false),
        Field::new("wind_velocity_y", DataType::Float32, false),
        Field::new("wind_velocity_z", DataType::Float32, false),
        Field::new("rotor_thrust_1", DataType::Float32, false),
        Field::new("rotor_thrust_2", DataType::Float32, false),
        Field::new("rotor_thrust_3", DataType::Float32, false),
        Field::new("rotor_thrust_4", DataType::Float32, false),
        Field::new("air_density_rho", DataType::Float32, false),
        Field::new("atheric_coherence", DataType::Float32, false),
        Field::new("lateral_position_drift_m", DataType::Float32, false),
        Field::new("is_in_vrs", DataType::Boolean, false),
        Field::new("scenario", DataType::Utf8, false),
        Field::new("sha256_seal", DataType::Utf8, false),
        Field::new("trajectory_id", DataType::Utf8, false),
        Field::new("thermal_accumulated", DataType::Float32, false),
        Field::new("gg_ekf_divergence", DataType::Float32, false),
        Field::new("cog_offset_x", DataType::Float32, false),
        Field::new("cog_offset_y", DataType::Float32, false),
        Field::new("cog_offset_z", DataType::Float32, false),
        Field::new("control_delay", DataType::Float32, false),
        Field::new("inertia_tensor_xx", DataType::Float32, false),
        Field::new("inertia_tensor_yy", DataType::Float32, false),
        Field::new("inertia_tensor_zz", DataType::Float32, false),
        Field::new("inertia_tensor_xy", DataType::Float32, false),
        Field::new("inertia_tensor_yz", DataType::Float32, false),
        Field::new("inertia_tensor_xz", DataType::Float32, false),
        Field::new("rotor_icing_1", DataType::Float32, false),
        Field::new("rotor_icing_2", DataType::Float32, false),
        Field::new("rotor_icing_3", DataType::Float32, false),
        Field::new("rotor_icing_4", DataType::Float32, false),
        Field::new("acoustic_resonance_g", DataType::Float32, false),
        Field::new("is_jammed", DataType::Boolean, false),
        Field::new("is_spoofed", DataType::Boolean, false),
        Field::new("ew_jamming_dbm", DataType::Float32, false),
        Field::new("gps_spoofing_bias_x", DataType::Float32, false),
        Field::new("gps_spoofing_bias_y", DataType::Float32, false),
        Field::new("gps_spoofing_bias_z", DataType::Float32, false),
        Field::new("voltage_sag", DataType::Float32, false),
        Field::new("atheric_capacity", DataType::Float32, false),
        Field::new("clock_drift", DataType::Float32, false),
        Field::new("is_in_pio", DataType::Boolean, false),
        Field::new("pio_amplitude_rad", DataType::Float32, false),
        Field::new("rotor_icing_drag_torque_delta_1", DataType::Float32, false),
        Field::new("rotor_icing_drag_torque_delta_2", DataType::Float32, false),
        Field::new("rotor_icing_drag_torque_delta_3", DataType::Float32, false),
        Field::new("rotor_icing_drag_torque_delta_4", DataType::Float32, false),
        Field::new("payload_slosh_displacement_x", DataType::Float32, false),
        Field::new("payload_slosh_displacement_y", DataType::Float32, false),
    ]));

    let file = File::create(&out_path).expect("Failed to create output Parquet file");
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .build();
    let mut writer = ArrowWriter::try_new(file, schema.clone(), Some(props))
        .expect("Failed to create ArrowWriter");
    
    // Seed generator
    let base_seed = 0x505E_FE1C_A6E2_4387u64;
    let seed_multiplier = 0x9E37_79B1_85EB_CA87u64;
    
    // Chunk size to prevent OOM
    let chunk_size = 2000;
    let mut written_count = 0;
    let mut total_rows = 0;
    
    while written_count < n_drones {
        let this_chunk_size = std::cmp::min(chunk_size, n_drones - written_count);
        let start_i = written_count;
        let end_i = start_i + this_chunk_size;
        
        let trajectories: Vec<DroneTrajectory> = (start_i..end_i)
            .into_par_iter()
            .map(|i| {
                let seed = base_seed ^ (i as u64).wrapping_mul(seed_multiplier);
                let scenario_for_traj = if scenario == "sweep" {
                    match i % 5 {
                        0 => "nominal",
                        1 => "heavy_payload",
                        2 => "high_winds",
                        3 => "vortex_ring_state",
                        _ => "pio_resonance",
                    }
                } else {
                    &scenario
                };
                run_single_trajectory(i, seed, n_drones, scenario_for_traj)
            })
            .collect();
            
        // Columnar buffers for RecordBatch
        let mut timestamp = Vec::new();
        let mut drone_pos_x = Vec::new();
        let mut drone_pos_y = Vec::new();
        let mut drone_pos_z = Vec::new();
        let mut drone_vel_x = Vec::new();
        let mut drone_vel_y = Vec::new();
        let mut drone_vel_z = Vec::new();
        let mut quat_w = Vec::new();
        let mut quat_x = Vec::new();
        let mut quat_y = Vec::new();
        let mut quat_z = Vec::new();
        let mut ang_vel_x = Vec::new();
        let mut ang_vel_y = Vec::new();
        let mut ang_vel_z = Vec::new();
        let mut wind_velocity_x = Vec::new();
        let mut wind_velocity_y = Vec::new();
        let mut wind_velocity_z = Vec::new();
        let mut rotor_thrust_1 = Vec::new();
        let mut rotor_thrust_2 = Vec::new();
        let mut rotor_thrust_3 = Vec::new();
        let mut rotor_thrust_4 = Vec::new();
        let mut air_density_rho = Vec::new();
        let mut atheric_coherence = Vec::new();
        let mut lateral_position_drift_m = Vec::new();
        let mut is_in_vrs = Vec::new();
        let mut scenario_vec = Vec::new();
        let mut sha256_seal = Vec::new();
        let mut trajectory_id = Vec::new();
        let mut thermal_accumulated = Vec::new();
        let mut gg_ekf_divergence = Vec::new();
        let mut cog_offset_x = Vec::new();
        let mut cog_offset_y = Vec::new();
        let mut cog_offset_z = Vec::new();
        let mut control_delay = Vec::new();
        let mut inertia_tensor_xx = Vec::new();
        let mut inertia_tensor_yy = Vec::new();
        let mut inertia_tensor_zz = Vec::new();
        let mut inertia_tensor_xy = Vec::new();
        let mut inertia_tensor_yz = Vec::new();
        let mut inertia_tensor_xz = Vec::new();
        let mut rotor_icing_1 = Vec::new();
        let mut rotor_icing_2 = Vec::new();
        let mut rotor_icing_3 = Vec::new();
        let mut rotor_icing_4 = Vec::new();
        let mut acoustic_resonance_g_arr = Vec::new();
        let mut is_jammed_arr = Vec::new();
        let mut is_spoofed_arr = Vec::new();
        let mut ew_jamming_dbm_arr = Vec::new();
        let mut gps_spoofing_bias_x_arr = Vec::new();
        let mut gps_spoofing_bias_y_arr = Vec::new();
        let mut gps_spoofing_bias_z_arr = Vec::new();
        let mut voltage_sag_arr = Vec::new();
        let mut atheric_capacity = Vec::new();
        let mut clock_drift = Vec::new();
        let mut is_in_pio_arr = Vec::new();
        let mut pio_amplitude_rad_arr = Vec::new();
        let mut rotor_icing_drag_torque_delta_1 = Vec::new();
        let mut rotor_icing_drag_torque_delta_2 = Vec::new();
        let mut rotor_icing_drag_torque_delta_3 = Vec::new();
        let mut rotor_icing_drag_torque_delta_4 = Vec::new();
        let mut payload_slosh_displacement_x = Vec::new();
        let mut payload_slosh_displacement_y = Vec::new();

        for traj in trajectories {
            let t_id = traj.trajectory_id;
            for step in traj.data {
                timestamp.push(step.timestamp);
                drone_pos_x.push(step.drone_pos_x);
                drone_pos_y.push(step.drone_pos_y);
                drone_pos_z.push(step.drone_pos_z);
                drone_vel_x.push(step.drone_vel_x);
                drone_vel_y.push(step.drone_vel_y);
                drone_vel_z.push(step.drone_vel_z);
                quat_w.push(step.quat_w);
                quat_x.push(step.quat_x);
                quat_y.push(step.quat_y);
                quat_z.push(step.quat_z);
                ang_vel_x.push(step.ang_vel_x);
                ang_vel_y.push(step.ang_vel_y);
                ang_vel_z.push(step.ang_vel_z);
                wind_velocity_x.push(step.wind_velocity_x);
                wind_velocity_y.push(step.wind_velocity_y);
                wind_velocity_z.push(step.wind_velocity_z);
                rotor_thrust_1.push(step.rotor_thrust_1);
                rotor_thrust_2.push(step.rotor_thrust_2);
                rotor_thrust_3.push(step.rotor_thrust_3);
                rotor_thrust_4.push(step.rotor_thrust_4);
                air_density_rho.push(step.air_density_rho);
                atheric_coherence.push(step.atheric_coherence);
                lateral_position_drift_m.push(step.lateral_position_drift_m);
                is_in_vrs.push(step.is_in_vrs);
                scenario_vec.push(step.scenario.clone());
                sha256_seal.push(step.sha256_seal);
                trajectory_id.push(t_id.clone());
                thermal_accumulated.push(step.thermal_accumulated);
                gg_ekf_divergence.push(step.gg_ekf_divergence);
                cog_offset_x.push(step.cog_offset_x);
                cog_offset_y.push(step.cog_offset_y);
                cog_offset_z.push(step.cog_offset_z);
                control_delay.push(step.control_delay);
                inertia_tensor_xx.push(step.inertia_tensor_xx);
                inertia_tensor_yy.push(step.inertia_tensor_yy);
                inertia_tensor_zz.push(step.inertia_tensor_zz);
                inertia_tensor_xy.push(step.inertia_tensor_xy);
                inertia_tensor_yz.push(step.inertia_tensor_yz);
                inertia_tensor_xz.push(step.inertia_tensor_xz);
                rotor_icing_1.push(step.rotor_icing_1);
                rotor_icing_2.push(step.rotor_icing_2);
                rotor_icing_3.push(step.rotor_icing_3);
                rotor_icing_4.push(step.rotor_icing_4);
                acoustic_resonance_g_arr.push(step.acoustic_resonance_g);
                is_jammed_arr.push(step.is_jammed);
                is_spoofed_arr.push(step.is_spoofed);
                ew_jamming_dbm_arr.push(step.ew_jamming_dbm);
                gps_spoofing_bias_x_arr.push(step.gps_spoofing_bias_x);
                gps_spoofing_bias_y_arr.push(step.gps_spoofing_bias_y);
                gps_spoofing_bias_z_arr.push(step.gps_spoofing_bias_z);
                voltage_sag_arr.push(step.voltage_sag);
                atheric_capacity.push(step.atheric_capacity);
                clock_drift.push(step.clock_drift);
                is_in_pio_arr.push(step.is_in_pio);
                pio_amplitude_rad_arr.push(step.pio_amplitude_rad);
                rotor_icing_drag_torque_delta_1.push(step.rotor_icing_drag_torque_delta_1);
                rotor_icing_drag_torque_delta_2.push(step.rotor_icing_drag_torque_delta_2);
                rotor_icing_drag_torque_delta_3.push(step.rotor_icing_drag_torque_delta_3);
                rotor_icing_drag_torque_delta_4.push(step.rotor_icing_drag_torque_delta_4);
                payload_slosh_displacement_x.push(step.payload_slosh_displacement_x);
                payload_slosh_displacement_y.push(step.payload_slosh_displacement_y);
            }
        }
        
        let rows_in_batch = timestamp.len();
        if rows_in_batch > 0 {
            let batch = RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(Float32Array::from(timestamp)),
                    Arc::new(Float32Array::from(drone_pos_x)),
                    Arc::new(Float32Array::from(drone_pos_y)),
                    Arc::new(Float32Array::from(drone_pos_z)),
                    Arc::new(Float32Array::from(drone_vel_x)),
                    Arc::new(Float32Array::from(drone_vel_y)),
                    Arc::new(Float32Array::from(drone_vel_z)),
                    Arc::new(Float32Array::from(quat_w)),
                    Arc::new(Float32Array::from(quat_x)),
                    Arc::new(Float32Array::from(quat_y)),
                    Arc::new(Float32Array::from(quat_z)),
                    Arc::new(Float32Array::from(ang_vel_x)),
                    Arc::new(Float32Array::from(ang_vel_y)),
                    Arc::new(Float32Array::from(ang_vel_z)),
                    Arc::new(Float32Array::from(wind_velocity_x)),
                    Arc::new(Float32Array::from(wind_velocity_y)),
                    Arc::new(Float32Array::from(wind_velocity_z)),
                    Arc::new(Float32Array::from(rotor_thrust_1)),
                    Arc::new(Float32Array::from(rotor_thrust_2)),
                    Arc::new(Float32Array::from(rotor_thrust_3)),
                    Arc::new(Float32Array::from(rotor_thrust_4)),
                    Arc::new(Float32Array::from(air_density_rho)),
                    Arc::new(Float32Array::from(atheric_coherence)),
                    Arc::new(Float32Array::from(lateral_position_drift_m)),
                    Arc::new(BooleanArray::from(is_in_vrs)),
                    Arc::new(StringArray::from(scenario_vec)),
                    Arc::new(StringArray::from(sha256_seal)),
                    Arc::new(StringArray::from(trajectory_id)),
                    Arc::new(Float32Array::from(thermal_accumulated)),
                    Arc::new(Float32Array::from(gg_ekf_divergence)),
                    Arc::new(Float32Array::from(cog_offset_x)),
                    Arc::new(Float32Array::from(cog_offset_y)),
                    Arc::new(Float32Array::from(cog_offset_z)),
                    Arc::new(Float32Array::from(control_delay)),
                    Arc::new(Float32Array::from(inertia_tensor_xx)),
                    Arc::new(Float32Array::from(inertia_tensor_yy)),
                    Arc::new(Float32Array::from(inertia_tensor_zz)),
                    Arc::new(Float32Array::from(inertia_tensor_xy)),
                    Arc::new(Float32Array::from(inertia_tensor_yz)),
                    Arc::new(Float32Array::from(inertia_tensor_xz)),
                    Arc::new(Float32Array::from(rotor_icing_1)),
                    Arc::new(Float32Array::from(rotor_icing_2)),
                    Arc::new(Float32Array::from(rotor_icing_3)),
                    Arc::new(Float32Array::from(rotor_icing_4)),
                    Arc::new(Float32Array::from(acoustic_resonance_g_arr)),
                    Arc::new(BooleanArray::from(is_jammed_arr)),
                    Arc::new(BooleanArray::from(is_spoofed_arr)),
                    Arc::new(Float32Array::from(ew_jamming_dbm_arr)),
                    Arc::new(Float32Array::from(gps_spoofing_bias_x_arr)),
                    Arc::new(Float32Array::from(gps_spoofing_bias_y_arr)),
                    Arc::new(Float32Array::from(gps_spoofing_bias_z_arr)),
                    Arc::new(Float32Array::from(voltage_sag_arr)),
                    Arc::new(Float32Array::from(atheric_capacity)),
                    Arc::new(Float32Array::from(clock_drift)),
                    Arc::new(BooleanArray::from(is_in_pio_arr)),
                    Arc::new(Float32Array::from(pio_amplitude_rad_arr)),
                    Arc::new(Float32Array::from(rotor_icing_drag_torque_delta_1)),
                    Arc::new(Float32Array::from(rotor_icing_drag_torque_delta_2)),
                    Arc::new(Float32Array::from(rotor_icing_drag_torque_delta_3)),
                    Arc::new(Float32Array::from(rotor_icing_drag_torque_delta_4)),
                    Arc::new(Float32Array::from(payload_slosh_displacement_x)),
                    Arc::new(Float32Array::from(payload_slosh_displacement_y)),
                ],
            ).expect("Failed to create RecordBatch");

            writer.write(&batch).expect("Failed to write RecordBatch");
            total_rows += rows_in_batch;
        }

        written_count += this_chunk_size;
        eprintln!("  Generated {}/{} trajectories...", written_count, n_drones);
    }
    
    writer.close().expect("Failed to close ArrowWriter");
    
    eprintln!("Successfully generated dataset ({} total rows). Total time: {:.2?}", total_rows, start.elapsed());
}
