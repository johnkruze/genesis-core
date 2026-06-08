//! 1000Hz GENESIS CORE MODULE: COMMERCIAL CONTACT FRICTION EDGE CASES
//! TARGET: Generic Commercial/Industrial Autonomous Systems
//! CLASS: Bipedal, Quadrupedal, and Skid-Steer Locomotion
//! SUBSYSTEM: Contact Dynamics, Slip, & Galling

use rayon::prelude::*;
use serde_json::json;
use sha2::{Sha256, Digest};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use rand::Rng;

const NUM_TRAJECTORIES: usize = 1_000_000;
const HZ: f64 = 1000.0;
const DT: f64 = 1.0 / HZ;

fn main() {
    let start_time = Instant::now();
    let export_dir = "/Users/aijesusbro/Spectrum/data/exports/commercial/jsonl";
    std::fs::create_dir_all(export_dir).unwrap();
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(format!("{}/lib_contact.jsonl", export_dir))
        .unwrap();
    let mut writer = BufWriter::new(file);

    println!("=========================================================");
    println!("G^G SOVEREIGN PHYSICS ENGINE: COMMERCIAL WHITE-LABEL");
    println!("LIBRARY: CONTACT FRICTION & SHEAR");
    println!("EXECUTING {} TRAJECTORIES...", NUM_TRAJECTORIES);
    println!("=========================================================\n");

    let failed_count = Arc::new(Mutex::new(0usize));

    let results: Vec<String> = (0..NUM_TRAJECTORIES).into_par_iter().map(|i| {
        let mut rng = rand::thread_rng();
        
        let mu_static = rng.gen_range(0.05..0.5); // Slippery to standard (Ice/Mud to dry tile)
        let mu_kinetic = mu_static * rng.gen_range(0.4..0.8); // Kinetic dropoff is sharp
        let mass_kg = rng.gen_range(50.0..250.0); // Humanoid / quad range
        
        let mut normal_force_n = mass_kg * 9.81;
        
        let mut accumulated_slip_m = 0.0;
        let mut max_slip_velocity_ms = 0.0;
        let mut grip_recovery_latency_ms = 0.0;
        
        let mut catastrophic_loss_of_traction = false;
        let mut is_slipping = false;
        let mut target_propulsion = 0.0;
        let mut current_slip_vel = 0.0;

        for tick in 0..(10.0 * HZ) as usize {
            let time_s = tick as f64 * DT;
            
            // Dynamic normal force
            let dynamic_ground_pressure = 1.0 + (time_s * 4.0 * std::f64::consts::PI).sin() * 0.4;
            normal_force_n = (mass_kg * 9.81) * dynamic_ground_pressure;
            
            let max_static_friction = mu_static * normal_force_n;
            
            if tick % 500 == 0 {
                target_propulsion = rng.gen_range(0.2..1.8); 
            }
            let commanded_propulsive_force_n = (mass_kg * 9.81) * target_propulsion; 
            
            if commanded_propulsive_force_n > max_static_friction || is_slipping {
                is_slipping = true;
                grip_recovery_latency_ms += DT * 1000.0;
                
                // Break traction -> fall to kinetic
                let net_force = commanded_propulsive_force_n - (mu_kinetic * normal_force_n);
                let slip_acceleration = net_force.max(0.0) / mass_kg;
                
                current_slip_vel += slip_acceleration * DT;
                accumulated_slip_m += current_slip_vel * DT;
                
                if current_slip_vel > max_slip_velocity_ms {
                    max_slip_velocity_ms = current_slip_vel;
                }
                
                // Extremely hard to recover static grip once kinetic slip starts under load
                if commanded_propulsive_force_n < (mu_kinetic * normal_force_n) * 0.5 {
                    is_slipping = false;
                }
            } else {
                is_slipping = false;
            }

            if accumulated_slip_m > 0.15 || max_slip_velocity_ms > 1.2 { // 15cm unrecovered slip means fall for a biped
                catastrophic_loss_of_traction = true;
                break;
            }
        }

        if catastrophic_loss_of_traction {
            let mut fc = failed_count.lock().unwrap();
            *fc += 1;
        }

        let mut hasher = Sha256::new();
        let sig_payload = format!("contact_{}_{}_{}", i, mu_static, accumulated_slip_m);
        hasher.update(sig_payload.as_bytes());
        let hash = hex::encode(hasher.finalize());

        let out = json!({
            "trajectory_id": i,
            "static_friction_coeff": f64::trunc(mu_static * 1000.0) / 1000.0,
            "kinetic_friction_coeff": f64::trunc(mu_kinetic * 1000.0) / 1000.0,
            "peak_normal_force_newtons": f64::trunc(normal_force_n * 10.0) / 10.0,
            "peak_slip_velocity_ms": f64::trunc(max_slip_velocity_ms * 1000.0) / 1000.0,
            "grip_recovery_latency_ms": f64::trunc(grip_recovery_latency_ms * 10.0) / 10.0,
            "accumulated_slip_m": f64::trunc(accumulated_slip_m * 1000.0) / 1000.0,
            "survived": !catastrophic_loss_of_traction,
            "failure_mode": if catastrophic_loss_of_traction { "UNRECOVERABLE_SLIP" } else { "NOMINAL" },
            "cryptographic_seal": hash
        });
        
        serde_json::to_string(&out).unwrap()
    }).collect();

    for res in results {
        writeln!(writer, "{}", res).unwrap();
    }

    let fc = *failed_count.lock().unwrap();
    println!("CONTACT LIBRARY GENERATION COMPLETE.");
    println!("TOTAL TRAJECTORIES: {:?}", NUM_TRAJECTORIES);
    println!("FAILURE YIELD: {} ({:.2}%)", fc, (fc as f64 / NUM_TRAJECTORIES as f64) * 100.0);
    println!("EXECUTION TIME: {:?}", start_time.elapsed());
}
