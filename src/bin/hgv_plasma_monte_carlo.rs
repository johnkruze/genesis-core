// Category 5: Hypersonic Aerothermal Ablation (The Fire-and-Metal Boundary)
// 1000Hz Euler integration of HGV dynamics under Sutton-Graves heating and ablation.
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

const MACH_1: f32 = 295.0f32; // Speed of sound at 30km altitude (m/s)
const PLASMA_BLACKOUT_MIN_VEL: f32 = MACH_1 * 10.0f32; // Blackout begins at Mach 10

// 32-Dimensional state struct, size assertion = exactly 128 bytes
#[allow(dead_code)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Serialize)]
struct HypersonicDynamicsState {
    timestamp: f32,                // 1. Time (4 bytes)
    pos: [f32; 3],                 // 2-4. Vehicle position: [x, y, z] (12 bytes)
    vel: [f32; 3],                 // 5-7. Velocity vector (Mach) (12 bytes)
    quat: [f32; 4],                // 8-11. Flight attitude quaternion: [w, x, y, z] (16 bytes)
    ang_vel: [f32; 3],             // 12-14. Angular rates: [p, q, r] (12 bytes)
    actuator_deflections: [f32; 4],// 15-18. Fin control surface deflections: [del_1, del_2, del_3, del_4] (16 bytes)
    stability_jacobians: [f32; 8], // 19-26. Analytical lift/drag derivatives: [CL_a, CD_a, CL_d, CD_d, ... ] (32 bytes)
    cog_migration: [f32; 3],       // 27-29. Dynamic CoG center offset: [x, y, z] (12 bytes)
    aeroshell_thickness: f32,      // 30. Ground truth nose radius/ablation (4 bytes)
    freestream_density: f32,       // 31. Ground truth air density (4 bytes)
    thermal_accumulated: f32,      // 32. Integrated heat tax metric (4 bytes)
}

// Compile-time assertion of exactly 128 bytes for L1/L2 cache line alignment
const _: () = assert!(std::mem::size_of::<HypersonicDynamicsState>() == 128);

#[derive(Debug, Clone, Copy, PartialEq)]
enum FailureMode {
    Nominal,
    AsymmetricAblation,  // Heat shield burns unevenly, creating lateral torque
    AeroshellErosion,    // Rapid mass loss shifts CoG past aerodynamic center
    PlasmaDensitySpike,  // High-altitude atmospheric density pocket
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Exoatmospheric,
    PlasmaBlackout,
    TerminalGlide,
    DepartureSpin,       // Fatal aerodynamic stall/spin
}

impl Phase {
    fn as_str(&self) -> &'static str {
        match self {
            Phase::Exoatmospheric => "EXOATMOSPHERIC",
            Phase::PlasmaBlackout => "PLASMA_BLACKOUT",
            Phase::TerminalGlide => "TERMINAL_GLIDE",
            Phase::DepartureSpin => "DEPARTURE_SPIN_FATAL",
        }
    }
}

// ─── HGV OBSERVER (EKF) ───
// Autopilot assumes static mass and static CoG during GPS-denied glide
struct HgvObserver {
    est_pitch: f32,
    est_pitch_rate: f32,
    #[allow(dead_code)]
    assumed_mass: f32,
    assumed_cog_offset: f32,
    p_cov: [f32; 4], // 2x2 covariance matrix [p00, p01, p10, p11]
}

impl HgvObserver {
    fn new(initial_mass: f32) -> Self {
        HgvObserver {
            est_pitch: 0.0f32,
            est_pitch_rate: 0.0f32,
            assumed_mass: initial_mass,
            assumed_cog_offset: 1.2f32, // meters forward of aerocenter
            p_cov: [0.1f32, 0.0f32, 0.0f32, 0.1f32],
        }
    }

    fn predict_and_update(&mut self, dt: f32, measured_pitch_rate: f32, control_deflection: f32, dynamic_pressure: f32, has_gps: bool) {
        let deflection_rad = control_deflection.to_radians();
        let expected_moment = dynamic_pressure * 0.5f32 * deflection_rad * self.assumed_cog_offset;
        let assumed_i_yy = 2000.0f32; // Assumed pitch moment of inertia, not mass!
        let expected_accel = expected_moment / assumed_i_yy;
        
        self.est_pitch_rate += expected_accel * dt;
        self.est_pitch += self.est_pitch_rate * dt;
        
        // EKF Predict Covariance: P_next = F * P * F^T + Q
        // F = [[1, dt], [0, 1]]
        let p00 = self.p_cov[0] + 2.0f32 * self.p_cov[1] * dt + self.p_cov[3] * dt * dt;
        let p01 = self.p_cov[1] + self.p_cov[3] * dt;
        let p10 = p01;
        let p11 = self.p_cov[3];
        
        self.p_cov = [p00 + 1e-4f32 * dt, p01, p10, p11 + 1e-3f32 * dt];
        
        if has_gps {
            // EKF Update Covariance (if GPS is active)
            // H = [0, 1] (measuring pitch rate)
            // S = H * P * H^T + R = P11 + R
            let r_noise = 0.05f32;
            let s_val = self.p_cov[3] + r_noise;
            let k0 = self.p_cov[1] / s_val;
            let k1 = self.p_cov[3] / s_val;
            
            let innovation = measured_pitch_rate - self.est_pitch_rate;
            self.est_pitch_rate += k1 * innovation;
            self.est_pitch += k0 * innovation;
            
            // P = (I - K*H) * P
            let new_p00 = self.p_cov[0] - k0 * self.p_cov[2];
            let new_p01 = self.p_cov[1] - k0 * self.p_cov[3];
            let new_p10 = self.p_cov[2] - k1 * self.p_cov[2];
            let new_p11 = self.p_cov[3] - k1 * self.p_cov[3];
            
            self.p_cov = [new_p00, new_p01, new_p10, new_p11];
        } else {
            // Under GPS denial, innovation is ignored, but we still apply a simple lag filter to estimate:
            let innovation = measured_pitch_rate - self.est_pitch_rate;
            self.est_pitch_rate += 0.02f32 * innovation; // heavily detuned update
            self.est_pitch += self.est_pitch_rate * dt;
        }
    }

    fn get_control_command(&self, target_pitch: f32) -> f32 {
        let error = target_pitch - self.est_pitch; // Both are in radians
        let cmd_rad = error * 12.0f32 - self.est_pitch_rate * 4.0f32;
        cmd_rad.to_degrees() // Output in degrees
    }
}

#[derive(Serialize)]
struct HypersonicStepState {
    timestamp: f32,
    pos_x: f32,
    pos_y: f32,
    pos_z: f32,
    vel_x: f32,
    vel_y: f32,
    vel_z: f32,
    quat_w: f32,
    quat_x: f32,
    quat_y: f32,
    quat_z: f32,
    ang_vel_x: f32,
    ang_vel_y: f32,
    ang_vel_z: f32,
    actuator_deflection_1: f32,
    actuator_deflection_2: f32,
    actuator_deflection_3: f32,
    actuator_deflection_4: f32,
    stability_jacobian_1: f32,
    stability_jacobian_2: f32,
    stability_jacobian_3: f32,
    stability_jacobian_4: f32,
    stability_jacobian_5: f32,
    stability_jacobian_6: f32,
    stability_jacobian_7: f32,
    stability_jacobian_8: f32,
    cog_migration_x: f32,
    cog_migration_y: f32,
    cog_migration_z: f32,
    aeroshell_thickness: f32,
    freestream_density: f32,
    thermal_accumulated: f32,
    hgv_velocity_mach: f32,
    hgv_mass_kg: f32,
    hgv_cog_offset_m: f32,
    hgv_nose_radius_m: f32,
    hgv_material_density_g_cm3: f32,
    hgv_attitude_error_deg: f32,
    hgv_flight_phase: String,
    hgv_surface_temp_k: f32,
    hgv_drag_coefficient: f32,
    hgv_mechanical_erosion_rate_kg_s: f32,
    wing_tip_displacement_m: f32,
    wing_tip_velocity_ms: f32,
    flutter_eigenvalue_real: f32,
    ekf_covariance_trace: f32,
    guidance_lockout: bool,
    scenario: String,
    sha256_seal: String,
}

#[derive(Serialize)]
struct HypersonicTrajectory {
    trajectory_id: String,
    data: Vec<HypersonicStepState>,
    proof_hash: String,
    survived: bool,
}

fn run_single_trajectory(index: usize, seed: u64, scenario: &str) -> HypersonicTrajectory {
    let mut rng = Rng::new(seed);
    
    // Initial velocity and position based on scenario (low altitude for transonic flutter sweep)
    let mut true_vel = if scenario == "transonic_flutter" {
        MACH_1 * 2.5f32
    } else {
        rng.range((MACH_1 * 8.0f32) as f64, (MACH_1 * 18.0f32) as f64) as f32
    };
    let initial_mass = 1500.0f32;
    let mut true_mass = initial_mass;
    
    let mut pos = if scenario == "transonic_flutter" {
        [0.0f32, 0.0f32, 35000.0f32]
    } else {
        [0.0f32, 0.0f32, 80000.0f32]
    };
    let mut vel = [true_vel, 0.0f32, -150.0f32];
    let mut quat = [1.0f32, 0.0f32, 0.0f32, 0.0f32];
    let mut ang_vel = [0.0f32; 3];

    // Pick-a-Part local state parameters
    let mut last_control_deflection = 0.0f32;
    let mut wing_tip_displacement_m = if scenario == "transonic_flutter" { 0.015f32 } else { 0.0f32 };
    let mut wing_tip_velocity_ms = 0.0f32;
    let mut flutter_eigenvalue_real = -0.1f32;
    
    let mut true_pitch = 0.0f32;
    let mut true_pitch_rate = 0.0f32;
    let mut true_cog_offset = 1.2f32; // Initial stable CoG lever arm
    let material_density = if let Ok(d_str) = std::env::var("DENSITY_FIXED") {
        d_str.parse::<f32>().unwrap_or(1.8f32)
    } else {
        rng.range(1.7, 2.1) as f32
    };
    
    let failure = match scenario {
        "asymmetric_ablation" | "asymmetric" => FailureMode::AsymmetricAblation,
        "aeroshell_erosion" | "erosion" => FailureMode::AeroshellErosion,
        "plasma_density_spike" | "spike" => FailureMode::PlasmaDensitySpike,
        "nominal" => FailureMode::Nominal,
        _ => if rng.chance(0.07) { FailureMode::AsymmetricAblation }
             else if rng.chance(0.04) { FailureMode::AeroshellErosion }
             else if rng.chance(0.03) { FailureMode::PlasmaDensitySpike }
             else { FailureMode::Nominal },
    };
    
    let mut observer = HgvObserver::new(initial_mass);
    
    let dt = 0.001f32;
    let max_time_s = 360.0f32;
    let max_steps = (max_time_s / dt) as usize;
    
    let mut states = Vec::with_capacity(max_steps / 250);
    
    let mut running_hash = Sha256::new();
    running_hash.update(&seed.to_le_bytes());
    let mut last_hash = running_hash.finalize();
    
    let mut phase = Phase::Exoatmospheric;
    let mut step = 0;
    let mut thermal_tax = 0.0f32;
    let mut time_in_plasma = 0.0f32;
    let mut survived = true;
    
    while step < max_steps {
        let t = step as f32 * dt;
        
        let altitude_km = pos[2] / 1000.0f32;
        let temp_k = if altitude_km > 51.0f32 {
            270.65f32 - 2.8f32 * (altitude_km - 51.0f32)
        } else if altitude_km > 47.0f32 {
            270.65f32
        } else {
            228.65f32 + 2.8f32 * (altitude_km - 32.0f32)
        };
        let local_speed_of_sound = (1.4f32 * 287.05f32 * temp_k).sqrt();

        let mut rho = (-altitude_km / 7.0f32).exp() * 1.225f32;
        if failure == FailureMode::PlasmaDensitySpike && t > 40.0f32 && t < 45.0f32 {
            rho *= 2.5f32; // density shear layer pocket
        }
        
        let q_pressure = 0.5f32 * rho * true_vel * true_vel;
        
        let is_plasma = true_vel > PLASMA_BLACKOUT_MIN_VEL && altitude_km < 85.0f32;
        let has_gps = !is_plasma;
        let ekf_covariance_trace = observer.p_cov[0] + observer.p_cov[3];
        let guidance_lockout = ekf_covariance_trace > 0.5f32;

        // Autopilot feedback control
        let measured_pitch_rate = true_pitch_rate + rng.range(-0.01, 0.01) as f32;
        let target_pitch = 5.0f32.to_radians(); // Convert to radians
        
        let mut control_deflection = if guidance_lockout {
            last_control_deflection
        } else {
            let cmd = observer.get_control_command(target_pitch);
            last_control_deflection = cmd;
            cmd
        };
        observer.predict_and_update(dt, measured_pitch_rate, control_deflection, q_pressure, has_gps);
        
        // Active WBC Guidance Inversion recovery
        let attitude_error = (true_pitch - observer.est_pitch).abs();
        if scenario == "nominal" && attitude_error > 2.0f32.to_radians() { // Convert to radians
            // Trim/boost elevon control deflection to counteract asymmetric ablation moments
            let trim_compensate = 8.0f32 * (0.95f32 - true_cog_offset).max(0.0f32);
            control_deflection += if true_pitch > target_pitch { -trim_compensate } else { trim_compensate };
        }
        
        control_deflection = control_deflection.clamp(-25.0f32, 25.0f32); // saturate elevons at 25 degrees
        
        // Wing Bending/Torsion Flutter Divergence
        let mach = true_vel / local_speed_of_sound;
        let is_transonic = mach >= 0.85f32 && mach <= 1.2f32;
        
        let structural_k_bend = 2000000.0f32;
        let structural_c_bend = 15000.0f32;
        let wing_mass = 300.0f32;

        let aero_stiffness_force = (q_pressure * 0.1f32) * wing_tip_displacement_m;
        let aero_damping_force = (q_pressure * 0.01f32) * wing_tip_velocity_ms;
        let natural_restoring_force = -structural_k_bend * wing_tip_displacement_m;
        let natural_damping_force = -structural_c_bend * wing_tip_velocity_ms;

        // If in transonic regime, active aileron control lag adds negative damping:
        let lagging_aileron_force = if is_transonic {
            wing_tip_velocity_ms * 100000.0f32 * (q_pressure / 50000.0f32)
        } else {
            0.0f32
        };

        let total_wing_force = natural_restoring_force + aero_stiffness_force + natural_damping_force + aero_damping_force + lagging_aileron_force;
        let wing_accel = total_wing_force / wing_mass;
        wing_tip_velocity_ms += wing_accel * dt;
        wing_tip_displacement_m += wing_tip_velocity_ms * dt;

        flutter_eigenvalue_real = if is_transonic {
            (q_pressure - 40000.0f32) * 1e-6f32 - 0.05f32
        } else {
            -0.1f32
        };

        // Shear structural failure trigger
        if scenario == "transonic_flutter" && wing_tip_displacement_m.abs() > 0.025f32 {
            phase = Phase::DepartureSpin;
            survived = false;
        }

        let mut surface_temp = 300.0f32;
        let mut mechanical_erosion_rate = 0.0f32;
        
        let nose_radius = 0.15f32 + 0.50f32 * (1.0f32 - (true_mass / initial_mass));
        let cd_dynamic = 0.10f32 + 0.05f32 * (nose_radius - 0.15f32) / 0.15f32;
        
        // Newtonian hypersonic lift and drag formulations
        let cl_a = 2.0f32 * true_pitch.sin().powi(2) * true_pitch.cos();
        let cd_a = 2.0f32 * true_pitch.sin().powi(3);
        let cl_d = 0.8f32 * control_deflection.to_radians().sin();
        let cd_d = 0.1f32 * control_deflection.to_radians().sin().powi(2);
        let stability_jacobians = [cl_a, cd_a, cl_d, cd_d, 0.1f32 * cl_a, 0.1f32 * cd_a, 0.0f32, 0.0f32];
        
        if is_plasma {
            phase = Phase::PlasmaBlackout;
            time_in_plasma += dt;
            
            // Sutton-Graves stagnation heat flux (W/m^2)
            let heat_flux_q = 1.7415e-4f32 * (rho / nose_radius).sqrt() * true_vel.powi(3);
            
            // Radiation equilibrium temperature
            let epsilon = 0.85f32;
            let sigma_sb = 5.670374e-8f32;
            surface_temp = (heat_flux_q / (epsilon * sigma_sb)).powf(0.25f32);
            
            // Thermal tax accumulation
            thermal_tax += (surface_temp * dt) / 1000.0f32;
            
            // Continuous Porosity model
            let max_density = 2.26f32; // Updated to theoretical graphite density
            let porosity = 1.0f32 - (material_density / max_density);
            let open_porosity_fraction = 1.0f32 / (1.0f32 + (150.0f32 * (material_density - 1.98f32)).exp());
            
            let k_boundary = 0.8e-7f32;
            let k_open = 4.5e-7f32 * (1.7f32 / material_density).powi(2);
            let ablation_constant = k_boundary + (k_open - k_boundary) * open_porosity_fraction;
            
            mechanical_erosion_rate = 0.05f32 * q_pressure * (surface_temp / 3000.0f32).powi(2) * porosity;
            
            let mut mass_burn_rate = 1.0f32 * ((heat_flux_q * ablation_constant) + mechanical_erosion_rate);
            if failure == FailureMode::AeroshellErosion {
                mass_burn_rate *= 2.2f32;
            }
            
            true_mass = (true_mass - mass_burn_rate * dt).max(100.0f32);
            true_cog_offset = 1.2f32 - 0.31f32 * (1.0f32 - (true_mass / initial_mass));
            
            if failure == FailureMode::AsymmetricAblation {
                true_pitch_rate += 0.5f32 * (heat_flux_q * 1.0e-6f32) * dt;
            }
        } else if time_in_plasma > 0.0f32 && true_vel <= PLASMA_BLACKOUT_MIN_VEL {
            phase = Phase::TerminalGlide;
        }
        
        // True physical moment (fin authority drops to 0.0 in the near-vacuum > 75km)
        let fin_authority = if altitude_km > 75.0f32 {
            0.0f32
        } else {
            1.0f32
        };
        let mut true_moment = q_pressure * 0.5f32 * (control_deflection.to_radians() * fin_authority) * true_cog_offset;
        if true_cog_offset < 0.95f32 {
            // Static stability inversion
            true_moment += q_pressure * 0.5f32 * true_pitch * (0.95f32 - true_cog_offset) * 25.0f32;
        }
        
        // Correct pitch acceleration using pitch moment of inertia I_yy (kg*m^2) instead of mass (kg)
        let i_yy = (2000.0f32 * (true_mass / initial_mass)).max(200.0f32);
        let true_pitch_accel = true_moment / i_yy;
        
        true_pitch_rate += true_pitch_accel * dt;
        true_pitch += true_pitch_rate * dt;
        
        // Aerodynamic forces acceleration: lift and drag coupling
        let drag_coefficient = cd_dynamic + cd_a + cd_d;
        let drag_accel = (q_pressure * drag_coefficient) / true_mass;
        true_vel -= drag_accel * dt;
        
        let lift_coefficient = cl_a + cl_d;
        let lift_accel = (q_pressure * lift_coefficient) / true_mass;
        
        let g_const = 9.81f32;
        vel[0] = true_vel;
        vel[2] += (lift_accel - g_const) * dt;
        
        // Closed-loop integration of coordinates from vertical/horizontal velocities
        pos[0] += vel[0] * dt;
        pos[1] += vel[1] * dt;
        pos[2] += vel[2] * dt;
        
        // Quaternion attitude integration (pitch angular rate mapped to body axis q)
        ang_vel[1] = true_pitch_rate;
        let qw = quat[0]; let qx = quat[1]; let qy = quat[2]; let qz = quat[3];
        let dq_w = -0.5f32 * (qx * ang_vel[0] + qy * ang_vel[1] + qz * ang_vel[2]);
        let dq_x =  0.5f32 * (qw * ang_vel[0] + qy * ang_vel[2] - qz * ang_vel[1]);
        let dq_y =  0.5f32 * (qw * ang_vel[1] - qx * ang_vel[2] + qz * ang_vel[0]);
        let dq_z =  0.5f32 * (qw * ang_vel[2] + qx * ang_vel[1] - qy * ang_vel[0]);
        quat[0] += dq_w * dt;
        quat[1] += dq_x * dt;
        quat[2] += dq_y * dt;
        quat[3] += dq_z * dt;
        
        let q_norm = (quat[0].powi(2) + quat[1].powi(2) + quat[2].powi(2) + quat[3].powi(2)).sqrt();
        if q_norm > 0.0f32 {
            quat[0] /= q_norm; quat[1] /= q_norm; quat[2] /= q_norm; quat[3] /= q_norm;
        }
        
        let attitude_error_deg = attitude_error * (180.0f32 / std::f32::consts::PI);
        
        // Instability threshold: attitude error > 15 degrees and CoG goes aft
        let is_unstable = true_cog_offset < 0.95f32;
        let is_spinning = attitude_error_deg > 15.0f32;
        
        if is_unstable && is_spinning {
            phase = Phase::DepartureSpin;
        }
        
        let is_logging_step = step % 250 == 0;
        let is_terminal_step = step == max_steps - 1 || phase == Phase::DepartureSpin || pos[2] <= 0.0f32;

        let aeroshell_thickness = (0.05f32 - (initial_mass - true_mass) / (material_density * 1000.0f32)).max(0.005f32);

        if is_logging_step || is_terminal_step {
            // Cryptographic hash chain step seal update
            let mut hasher = Sha256::new();
            hasher.update(&last_hash);
            hasher.update(&t.to_le_bytes());
            hasher.update(&pos[0].to_le_bytes());
            hasher.update(&pos[1].to_le_bytes());
            hasher.update(&pos[2].to_le_bytes());
            hasher.update(&vel[0].to_le_bytes());
            hasher.update(&vel[1].to_le_bytes());
            hasher.update(&vel[2].to_le_bytes());
            hasher.update(&quat[0].to_le_bytes());
            hasher.update(&quat[1].to_le_bytes());
            hasher.update(&quat[2].to_le_bytes());
            hasher.update(&quat[3].to_le_bytes());
            hasher.update(&ang_vel[0].to_le_bytes());
            hasher.update(&ang_vel[1].to_le_bytes());
            hasher.update(&ang_vel[2].to_le_bytes());
            hasher.update(&control_deflection.to_le_bytes());
            hasher.update(&0.0f32.to_le_bytes()); // placeholder fin 2
            hasher.update(&0.0f32.to_le_bytes()); // placeholder fin 3
            hasher.update(&0.0f32.to_le_bytes()); // placeholder fin 4
            hasher.update(&stability_jacobians[0].to_le_bytes());
            hasher.update(&stability_jacobians[1].to_le_bytes());
            hasher.update(&stability_jacobians[2].to_le_bytes());
            hasher.update(&stability_jacobians[3].to_le_bytes());
            hasher.update(&stability_jacobians[4].to_le_bytes());
            hasher.update(&stability_jacobians[5].to_le_bytes());
            hasher.update(&stability_jacobians[6].to_le_bytes());
            hasher.update(&stability_jacobians[7].to_le_bytes());
            hasher.update(&true_cog_offset.to_le_bytes()); // CoG offset x
            hasher.update(&0.0f32.to_le_bytes()); // CoG y
            hasher.update(&0.0f32.to_le_bytes()); // CoG z
            hasher.update(&aeroshell_thickness.to_le_bytes());
            hasher.update(&rho.to_le_bytes());
            hasher.update(&thermal_tax.to_le_bytes());
            hasher.update(&wing_tip_displacement_m.to_le_bytes());
            hasher.update(&wing_tip_velocity_ms.to_le_bytes());
            hasher.update(&flutter_eigenvalue_real.to_le_bytes());
            hasher.update(&ekf_covariance_trace.to_le_bytes());
            hasher.update(&[guidance_lockout as u8]);
            
            last_hash = hasher.finalize();
            let sha256_seal = hex::encode(last_hash);

            states.push(HypersonicStepState {
                timestamp: t,
                pos_x: pos[0],
                pos_y: pos[1],
                pos_z: pos[2],
                vel_x: vel[0],
                vel_y: vel[1],
                vel_z: vel[2],
                quat_w: quat[0],
                quat_x: quat[1],
                quat_y: quat[2],
                quat_z: quat[3],
                ang_vel_x: ang_vel[0],
                ang_vel_y: ang_vel[1],
                ang_vel_z: ang_vel[2],
                actuator_deflection_1: control_deflection,
                actuator_deflection_2: 0.0f32,
                actuator_deflection_3: 0.0f32,
                actuator_deflection_4: 0.0f32,
                stability_jacobian_1: stability_jacobians[0],
                stability_jacobian_2: stability_jacobians[1],
                stability_jacobian_3: stability_jacobians[2],
                stability_jacobian_4: stability_jacobians[3],
                stability_jacobian_5: stability_jacobians[4],
                stability_jacobian_6: stability_jacobians[5],
                stability_jacobian_7: stability_jacobians[6],
                stability_jacobian_8: stability_jacobians[7],
                cog_migration_x: true_cog_offset,
                cog_migration_y: 0.0f32,
                cog_migration_z: 0.0f32,
                aeroshell_thickness,
                freestream_density: rho,
                thermal_accumulated: thermal_tax,
                hgv_velocity_mach: true_vel / local_speed_of_sound,
                hgv_mass_kg: true_mass,
                hgv_cog_offset_m: true_cog_offset,
                hgv_nose_radius_m: nose_radius,
                hgv_material_density_g_cm3: material_density,
                hgv_attitude_error_deg: attitude_error_deg,
                hgv_flight_phase: phase.as_str().to_string(),
                hgv_surface_temp_k: surface_temp,
                hgv_drag_coefficient: cd_dynamic,
                hgv_mechanical_erosion_rate_kg_s: mechanical_erosion_rate,
                wing_tip_displacement_m,
                wing_tip_velocity_ms,
                flutter_eigenvalue_real,
                ekf_covariance_trace,
                guidance_lockout,
                scenario: scenario.to_string(),
                sha256_seal,
            });
        }

        if phase == Phase::DepartureSpin {
            survived = false;
            break;
        }

        if pos[2] <= 0.0f32 {
            break;
        }
        
        step += 1;
    }
    
    let proof_hash = hex::encode(last_hash);
    
    HypersonicTrajectory {
        trajectory_id: format!("pg_hgv_{:05x}", index),
        data: states,
        proof_hash,
        survived,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n_trajectories: usize = args.get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10000);
    
    let mut total_survived = 0;
        
    let out_path = args.iter().position(|a| a == "--out")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "/Users/aijesusbro/Spectrum/data/products/hypersonic_aerothermal_ablation.parquet".to_string());

    let scenario = args.iter().position(|a| a == "--scenario")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "nominal".to_string());
        
    eprintln!("Generating {} Hypersonic trajectories to Parquet...", n_trajectories);
    let start = Instant::now();

    // Define Arrow schema
    let schema = Arc::new(Schema::new(vec![
        Field::new("timestamp", DataType::Float32, false),
        Field::new("pos_x", DataType::Float32, false),
        Field::new("pos_y", DataType::Float32, false),
        Field::new("pos_z", DataType::Float32, false),
        Field::new("vel_x", DataType::Float32, false),
        Field::new("vel_y", DataType::Float32, false),
        Field::new("vel_z", DataType::Float32, false),
        Field::new("quat_w", DataType::Float32, false),
        Field::new("quat_x", DataType::Float32, false),
        Field::new("quat_y", DataType::Float32, false),
        Field::new("quat_z", DataType::Float32, false),
        Field::new("ang_vel_x", DataType::Float32, false),
        Field::new("ang_vel_y", DataType::Float32, false),
        Field::new("ang_vel_z", DataType::Float32, false),
        Field::new("actuator_deflection_1", DataType::Float32, false),
        Field::new("actuator_deflection_2", DataType::Float32, false),
        Field::new("actuator_deflection_3", DataType::Float32, false),
        Field::new("actuator_deflection_4", DataType::Float32, false),
        Field::new("stability_jacobian_1", DataType::Float32, false),
        Field::new("stability_jacobian_2", DataType::Float32, false),
        Field::new("stability_jacobian_3", DataType::Float32, false),
        Field::new("stability_jacobian_4", DataType::Float32, false),
        Field::new("stability_jacobian_5", DataType::Float32, false),
        Field::new("stability_jacobian_6", DataType::Float32, false),
        Field::new("stability_jacobian_7", DataType::Float32, false),
        Field::new("stability_jacobian_8", DataType::Float32, false),
        Field::new("cog_migration_x", DataType::Float32, false),
        Field::new("cog_migration_y", DataType::Float32, false),
        Field::new("cog_migration_z", DataType::Float32, false),
        Field::new("aeroshell_thickness", DataType::Float32, false),
        Field::new("freestream_density", DataType::Float32, false),
        Field::new("thermal_accumulated", DataType::Float32, false),
        Field::new("hgv_velocity_mach", DataType::Float32, false),
        Field::new("hgv_mass_kg", DataType::Float32, false),
        Field::new("hgv_cog_offset_m", DataType::Float32, false),
        Field::new("hgv_nose_radius_m", DataType::Float32, false),
        Field::new("hgv_material_density_g_cm3", DataType::Float32, false),
        Field::new("hgv_attitude_error_deg", DataType::Float32, false),
        Field::new("hgv_flight_phase", DataType::Utf8, false),
        Field::new("hgv_surface_temp_k", DataType::Float32, false),
        Field::new("hgv_drag_coefficient", DataType::Float32, false),
        Field::new("hgv_mechanical_erosion_rate_kg_s", DataType::Float32, false),
        Field::new("wing_tip_displacement_m", DataType::Float32, false),
        Field::new("wing_tip_velocity_ms", DataType::Float32, false),
        Field::new("flutter_eigenvalue_real", DataType::Float32, false),
        Field::new("ekf_covariance_trace", DataType::Float32, false),
        Field::new("guidance_lockout", DataType::Boolean, false),
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
    let chunk_size = 1000;
    let mut written_count = 0;
    let mut total_rows = 0;
    
    while written_count < n_trajectories {
        let this_chunk_size = std::cmp::min(chunk_size, n_trajectories - written_count);
        let start_i = written_count;
        let end_i = start_i + this_chunk_size;
        
        let trajectories: Vec<HypersonicTrajectory> = (start_i..end_i)
            .into_par_iter()
            .map(|i| {
                let seed = base_seed ^ (i as u64).wrapping_mul(seed_multiplier);
                let scenario_for_traj = if scenario == "sweep" {
                    match i % 5 {
                        0 => "nominal",
                        1 => "asymmetric_ablation",
                        2 => "aeroshell_erosion",
                        3 => "plasma_density_spike",
                        _ => "transonic_flutter",
                    }
                } else {
                    &scenario
                };
                run_single_trajectory(i, seed, scenario_for_traj)
            })
            .collect();
            
        // Columnar buffers for RecordBatch
        let mut timestamp = Vec::new();
        let mut pos_x = Vec::new();
        let mut pos_y = Vec::new();
        let mut pos_z = Vec::new();
        let mut vel_x = Vec::new();
        let mut vel_y = Vec::new();
        let mut vel_z = Vec::new();
        let mut quat_w = Vec::new();
        let mut quat_x = Vec::new();
        let mut quat_y = Vec::new();
        let mut quat_z = Vec::new();
        let mut ang_vel_x = Vec::new();
        let mut ang_vel_y = Vec::new();
        let mut ang_vel_z = Vec::new();
        let mut actuator_deflection_1 = Vec::new();
        let mut actuator_deflection_2 = Vec::new();
        let mut actuator_deflection_3 = Vec::new();
        let mut actuator_deflection_4 = Vec::new();
        let mut stability_jacobian_1 = Vec::new();
        let mut stability_jacobian_2 = Vec::new();
        let mut stability_jacobian_3 = Vec::new();
        let mut stability_jacobian_4 = Vec::new();
        let mut stability_jacobian_5 = Vec::new();
        let mut stability_jacobian_6 = Vec::new();
        let mut stability_jacobian_7 = Vec::new();
        let mut stability_jacobian_8 = Vec::new();
        let mut cog_migration_x = Vec::new();
        let mut cog_migration_y = Vec::new();
        let mut cog_migration_z = Vec::new();
        let mut aeroshell_thickness = Vec::new();
        let mut freestream_density = Vec::new();
        let mut thermal_accumulated = Vec::new();
        let mut hgv_velocity_mach = Vec::new();
        let mut hgv_mass_kg = Vec::new();
        let mut hgv_cog_offset_m = Vec::new();
        let mut hgv_nose_radius_m = Vec::new();
        let mut hgv_material_density_g_cm3 = Vec::new();
        let mut hgv_attitude_error_deg = Vec::new();
        let mut hgv_flight_phase = Vec::new();
        let mut hgv_surface_temp_k = Vec::new();
        let mut hgv_drag_coefficient = Vec::new();
        let mut hgv_mechanical_erosion_rate_kg_s = Vec::new();
        let mut wing_tip_displacement_m = Vec::new();
        let mut wing_tip_velocity_ms = Vec::new();
        let mut flutter_eigenvalue_real = Vec::new();
        let mut ekf_covariance_trace = Vec::new();
        let mut guidance_lockout = Vec::new();
        let mut scenario_vec = Vec::new();
        let mut sha256_seal = Vec::new();
        let mut trajectory_id = Vec::new();

        for traj in trajectories {
            if traj.survived {
                total_survived += 1;
            }
            let t_id = traj.trajectory_id;
            for step in traj.data {
                timestamp.push(step.timestamp);
                pos_x.push(step.pos_x);
                pos_y.push(step.pos_y);
                pos_z.push(step.pos_z);
                vel_x.push(step.vel_x);
                vel_y.push(step.vel_y);
                vel_z.push(step.vel_z);
                quat_w.push(step.quat_w);
                quat_x.push(step.quat_x);
                quat_y.push(step.quat_y);
                quat_z.push(step.quat_z);
                ang_vel_x.push(step.ang_vel_x);
                ang_vel_y.push(step.ang_vel_y);
                ang_vel_z.push(step.ang_vel_z);
                actuator_deflection_1.push(step.actuator_deflection_1);
                actuator_deflection_2.push(step.actuator_deflection_2);
                actuator_deflection_3.push(step.actuator_deflection_3);
                actuator_deflection_4.push(step.actuator_deflection_4);
                stability_jacobian_1.push(step.stability_jacobian_1);
                stability_jacobian_2.push(step.stability_jacobian_2);
                stability_jacobian_3.push(step.stability_jacobian_3);
                stability_jacobian_4.push(step.stability_jacobian_4);
                stability_jacobian_5.push(step.stability_jacobian_5);
                stability_jacobian_6.push(step.stability_jacobian_6);
                stability_jacobian_7.push(step.stability_jacobian_7);
                stability_jacobian_8.push(step.stability_jacobian_8);
                cog_migration_x.push(step.cog_migration_x);
                cog_migration_y.push(step.cog_migration_y);
                cog_migration_z.push(step.cog_migration_z);
                aeroshell_thickness.push(step.aeroshell_thickness);
                freestream_density.push(step.freestream_density);
                thermal_accumulated.push(step.thermal_accumulated);
                hgv_velocity_mach.push(step.hgv_velocity_mach);
                hgv_mass_kg.push(step.hgv_mass_kg);
                hgv_cog_offset_m.push(step.hgv_cog_offset_m);
                hgv_nose_radius_m.push(step.hgv_nose_radius_m);
                hgv_material_density_g_cm3.push(step.hgv_material_density_g_cm3);
                hgv_attitude_error_deg.push(step.hgv_attitude_error_deg);
                hgv_flight_phase.push(step.hgv_flight_phase);
                hgv_surface_temp_k.push(step.hgv_surface_temp_k);
                hgv_drag_coefficient.push(step.hgv_drag_coefficient);
                hgv_mechanical_erosion_rate_kg_s.push(step.hgv_mechanical_erosion_rate_kg_s);
                wing_tip_displacement_m.push(step.wing_tip_displacement_m);
                wing_tip_velocity_ms.push(step.wing_tip_velocity_ms);
                flutter_eigenvalue_real.push(step.flutter_eigenvalue_real);
                ekf_covariance_trace.push(step.ekf_covariance_trace);
                guidance_lockout.push(step.guidance_lockout);
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
                    Arc::new(Float32Array::from(pos_x)),
                    Arc::new(Float32Array::from(pos_y)),
                    Arc::new(Float32Array::from(pos_z)),
                    Arc::new(Float32Array::from(vel_x)),
                    Arc::new(Float32Array::from(vel_y)),
                    Arc::new(Float32Array::from(vel_z)),
                    Arc::new(Float32Array::from(quat_w)),
                    Arc::new(Float32Array::from(quat_x)),
                    Arc::new(Float32Array::from(quat_y)),
                    Arc::new(Float32Array::from(quat_z)),
                    Arc::new(Float32Array::from(ang_vel_x)),
                    Arc::new(Float32Array::from(ang_vel_y)),
                    Arc::new(Float32Array::from(ang_vel_z)),
                    Arc::new(Float32Array::from(actuator_deflection_1)),
                    Arc::new(Float32Array::from(actuator_deflection_2)),
                    Arc::new(Float32Array::from(actuator_deflection_3)),
                    Arc::new(Float32Array::from(actuator_deflection_4)),
                    Arc::new(Float32Array::from(stability_jacobian_1)),
                    Arc::new(Float32Array::from(stability_jacobian_2)),
                    Arc::new(Float32Array::from(stability_jacobian_3)),
                    Arc::new(Float32Array::from(stability_jacobian_4)),
                    Arc::new(Float32Array::from(stability_jacobian_5)),
                    Arc::new(Float32Array::from(stability_jacobian_6)),
                    Arc::new(Float32Array::from(stability_jacobian_7)),
                    Arc::new(Float32Array::from(stability_jacobian_8)),
                    Arc::new(Float32Array::from(cog_migration_x)),
                    Arc::new(Float32Array::from(cog_migration_y)),
                    Arc::new(Float32Array::from(cog_migration_z)),
                    Arc::new(Float32Array::from(aeroshell_thickness)),
                    Arc::new(Float32Array::from(freestream_density)),
                    Arc::new(Float32Array::from(thermal_accumulated)),
                    Arc::new(Float32Array::from(hgv_velocity_mach)),
                    Arc::new(Float32Array::from(hgv_mass_kg)),
                    Arc::new(Float32Array::from(hgv_cog_offset_m)),
                    Arc::new(Float32Array::from(hgv_nose_radius_m)),
                    Arc::new(Float32Array::from(hgv_material_density_g_cm3)),
                    Arc::new(Float32Array::from(hgv_attitude_error_deg)),
                    Arc::new(StringArray::from(hgv_flight_phase)),
                    Arc::new(Float32Array::from(hgv_surface_temp_k)),
                    Arc::new(Float32Array::from(hgv_drag_coefficient)),
                    Arc::new(Float32Array::from(hgv_mechanical_erosion_rate_kg_s)),
                    Arc::new(Float32Array::from(wing_tip_displacement_m)),
                    Arc::new(Float32Array::from(wing_tip_velocity_ms)),
                    Arc::new(Float32Array::from(flutter_eigenvalue_real)),
                    Arc::new(Float32Array::from(ekf_covariance_trace)),
                    Arc::new(BooleanArray::from(guidance_lockout)),
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
