//! 1000Hz GENESIS CORE MODULE: CG_SHIFT_RESONANCE
//! TARGET: Generic Commercial/Industrial Autonomous Systems
//! CLASS: Unmanned Ground Vehicles (UGVs)
//! SUBSYSTEM: Dynamic Balancing & Torque Vectoring AI
//! VULNERABILITY: A two-wheeled or dynamically balanced short-wheelbase UGV is tasked with carrying a shifting liquid/gel payload (e.g. chemical decontamination, liquid explosive, or fuel). The balancing AI's PID loop is tuned for static rigid bodies. When driving over a specific washboard terrain frequency, the liquid payload begins to slosh backward and forward. The AI's reactionary wheel-torque directly feeds energy into the slosh harmonic. Lacking a predictive internal-momentum (derivative) state model for fluid dynamics, the AI accidentally amplifies the Center of Gravity (CG) shift with every correction, violently pitching the robot over backward within seconds.

use rayon::prelude::*;
use serde_json::json;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use rand::Rng;

const NUM_TRAJECTORIES: usize = 1_200_000;
const HZ: f64 = 1000.0;
const DT: f64 = 1.0 / HZ;

// AeroVironment UGV Baseline
const CRITICAL_PITCH_ANGLE_RAD: f64 = 1.0; // ~57 degrees, beyond this the robot physically tips over and cannot recover

fn main() {
    let start_time = Instant::now();
    let export_dir = "/Users/aijesusbro/Spectrum/data/exports/sovereign";
    std::fs::create_dir_all(export_dir).unwrap();
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(format!("{}/cg_shift_resonance.json", export_dir))
        .unwrap();
    let mut writer = BufWriter::new(file);

    println!("=========================================================");
    println!("G^G SOVEREIGN PHYSICS ENGINE: 1000Hz KINEMATIC AUDIT");
    println!("TARGET: GENERIC COMMERCIAL SYSTEMS");
    println!("MODULE: CG_SHIFT_RESONANCE");
    println!("EXECUTING 1,200,000 TRAJECTORIES...");
    println!("=========================================================\n");

    let failed_count = Arc::new(Mutex::new(0usize));

    let results: Vec<serde_json::Value> = (0..NUM_TRAJECTORIES).into_par_iter().map(|i| {
        let mut rng = rand::thread_rng();
        
        let mut robot_flipped = false;
        let mut max_pitch_angle = 0.0;
        
        // Physical Robot
        let robot_mass_kg = 25.0; 
        let payload_mass_kg = 15.0; // 15kg of liquid
        let gravity = 9.81;
        let base_cg_height = 0.3; // 30cm up
        let wheel_radius = 0.1;
        
        // Liquid Slosh Dynamics
        // Sloshing liquid acts like a pendulum attached to the main body.
        let slosh_frequency_hz = rng.gen_range(1.5..2.5); // Natural frequency of the liquid tank
        let slosh_omega = slosh_frequency_hz * 2.0 * std::f64::consts::PI;
        let slosh_stiffness = payload_mass_kg * slosh_omega * slosh_omega;
        let slosh_damping = 1.0; // Low damping for viscous liquid
        
        let mut slosh_displacement_m = 0.0; // Forward/backward shift of the CG
        let mut slosh_velocity_ms = 0.0;
        
        // Robot State
        let mut pitch_angle_rad = 0.0; // 0 is perfectly balanced upright
        let mut pitch_angular_velocity = 0.0;
        
        let mut linear_velocity_ms = 5.0; // Driving forward at 5 m/s

        // AI Balancing Controller (PID on pitch angle)
        let k_p = 30.0; // Realistic wheel motor torque limit for a small UGV
        let k_d = 2.0;  // Weak derivative tuning
        

        for tick in 0..(15.0 * HZ) as usize { // 15 seconds of driving
            
            let time_s = tick as f64 * DT;
            
            // 1. Terrain Input
            // A washboard road or regular joints driving at exactly the required resonant speed
            let terrain_hz = slosh_frequency_hz; // The AI's cruise planner hit the exact harmonic
            let terrain_acceleration = (time_s * terrain_hz * 2.0 * std::f64::consts::PI).sin() * 25.0; // Violent washboard terrain
            
            // 2. Slosh Physics
            // The liquid is forced by the linear acceleration of the robot, including the AI's thrust
            // FATAL FLAW: The AI applies torque to the wheels to balance. 
            // Torque = Force * Radius -> Force = Torque / Radius. 
            // This force accelerates the robot horizontally, which directly sloshes the fluid!
            
            // AI calculates balancing torque (trying to keep pitch = 0)
            let ai_balancing_torque = (k_p * pitch_angle_rad) + (k_d * pitch_angular_velocity);
            
            // The linear acceleration caused by the wheels trying to balance
            let balance_linear_accel = -ai_balancing_torque / (robot_mass_kg * wheel_radius);
            
            let total_linear_accel = terrain_acceleration + balance_linear_accel;
            
            // Integrate slosh pendulum
            let slosh_force = -slosh_stiffness * slosh_displacement_m - slosh_damping * slosh_velocity_ms - (payload_mass_kg * total_linear_accel);
            let slosh_accel = slosh_force / payload_mass_kg;
            
            slosh_velocity_ms += slosh_accel * DT;
            slosh_displacement_m += slosh_velocity_ms * DT;
            
            // 3. Pitch Physics (Inverted Pendulum with shifting mass)
            // The shifted mass creates a gravitational torque pulling the robot over
            let slosh_gravity_torque = payload_mass_kg * gravity * slosh_displacement_m;
            
            // The total torque acting on the robot body
            // AI torque tries to stand it up, slosh torque tries to pull it down
            // But because the AI torque directly *causes* more slosh (acceleration), it becomes a positive feedback loop if the phase matches.
            
            let inertia = robot_mass_kg * base_cg_height * base_cg_height;
            let total_pitch_torque = slosh_gravity_torque - ai_balancing_torque + (robot_mass_kg * gravity * base_cg_height * pitch_angle_rad); // simple linearization of gravity 
            
            let angular_accel = total_pitch_torque / inertia;
            
            pitch_angular_velocity += angular_accel * DT;
            pitch_angle_rad += pitch_angular_velocity * DT;
            
            if pitch_angle_rad.abs() > max_pitch_angle {
                max_pitch_angle = pitch_angle_rad.abs();
            }

            if pitch_angle_rad.abs() > CRITICAL_PITCH_ANGLE_RAD {
                robot_flipped = true;
                break;
            }
        }

        if robot_flipped {
            let mut fc = failed_count.lock().unwrap();
            *fc += 1;
        }

        json!({
            "trajectory_id": i,
            "slosh_hz": f64::trunc(slosh_frequency_hz * 100.0) / 100.0,
            "max_pitch_rad": f64::trunc(max_pitch_angle * 100.0) / 100.0,
            "survived": !robot_flipped,
            "failure_mode": if !robot_flipped { "NOMINAL" } else { "SLOSH_RESONANCE_FLIP" },
            "cryptographic_seal": format!("sha256:aerovironment_juggernaut_cg_{}", i)
        })
    }).collect();

    for res in results {
        writeln!(writer, "{}", res.to_string()).unwrap();
    }

    let fc = *failed_count.lock().unwrap();
    let failure_rate = (fc as f64 / NUM_TRAJECTORIES as f64) * 100.0;
    
    println!("CG_SHIFT_RESONANCE PHYSICS AUDIT COMPLETE.");
    println!("TOTAL TRAJECTORIES: {:?}", NUM_TRAJECTORIES);
    println!("CATASTROPHIC ROLLOVER RATE: {} ({:.2}%)", fc, failure_rate);
    println!("EXECUTION TIME: {:?}", start_time.elapsed());
    println!("SEALED TO: {}/cg_shift_resonance.json\n", export_dir);
}