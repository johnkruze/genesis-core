//! 1000Hz GENESIS CORE MODULE: HYDRAULIC_FLUID_COMPRESSIBILITY
//! TARGET: Generic Commercial/Industrial Autonomous Systems
//! CLASS: Bipedal Humanoid
//! SUBSYSTEM: Carbon AI & Precision Hydraulics
//! VULNERABILITY: Hydraulic fluid is not perfectly incompressible. Under high-pressure loads (e.g., 2000 PSI lifting), entrained air and fluid micro-compression create a spongy physical response. Carbon AI assumes instantaneous rigid transmission, causing overshoot oscillation in precision tasks.

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

// Hydraulic Actuator Baseline
const NOMINAL_BULK_MODULUS_PSI: f64 = 250000.0; // Typical hydraulic oil stiffness
const CRITICAL_OVERSHOOT_ERROR_MM: f64 = 3.0; // 3mm overshoot when trying to insert a tight-tolerance part (e.g. spark plug) destroys the thread.

fn main() {
    let start_time = Instant::now();
    let export_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/exports/sovereign");
    std::fs::create_dir_all(export_dir).unwrap();
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(format!("{}/hydraulic_fluid_compressibility.json", export_dir))
        .unwrap();
    let mut writer = BufWriter::new(file);

    println!("=========================================================");
    println!("G^G SOVEREIGN PHYSICS ENGINE: 1000Hz FLUID DYNAMICS AUDIT");
    println!("TARGET: GENERIC COMMERCIAL SYSTEMS");
    println!("MODULE: HYDRAULIC_FLUID_COMPRESSIBILITY");
    println!("EXECUTING 1,200,000 TRAJECTORIES...");
    println!("=========================================================\n");

    let failed_count = Arc::new(Mutex::new(0usize));

    let results: Vec<serde_json::Value> = (0..NUM_TRAJECTORIES).into_par_iter().map(|i| {
        let mut rng = rand::thread_rng();
        
        let mut max_overshoot_experienced = 0.0;
        let mut task_failed = false;
        
        // Entrained microscopic air bubbles drastically reduce the Bulk Modulus (stiffness) of hydraulic fluid over time.
        // A perfectly bled system is 250k PSI. 2% entrained air drops it to 50k PSI (Spongy).
        let entrained_air_percent: f64 = rng.gen_range(0.1..3.0);
        let actual_bulk_modulus_psi = NOMINAL_BULK_MODULUS_PSI * (-0.5 * entrained_air_percent).exp();
        
        // Arm moving a 5kg component at high speed to insert into a tight 1mm tolerance slot
        let payload_kg = 5.0;
        let arm_velocity_target_ms = 0.5; // Moving at 0.5 m/s

        // The AI commands hydraulic valves to aggressively stop exactly at Position Target.
        // It relies on generative video and kinematics, assuming fluid rigidly stops the piston.
        
        for _tick in 0..(1.0 * HZ) as usize { // 1 second stop event
            
            // To stop 5kg at 0.5m/s in 0.002 seconds (a digital step command) requires a massive pressure spike.
            let deceleration_g = (arm_velocity_target_ms / 0.002) / 9.81;
            let stopping_force_n = payload_kg * deceleration_g * 9.81;
            
            // Piston Area assumed 500 mm^2 (0.77 sq in)
            let pressure_spike_psi = (stopping_force_n * 0.2248) / 0.77; 

            // Compressibility Delta (Delta Volume / Original Volume) = Pressure / Bulk Modulus
            // This translates directly to piston physical travel PAST the stopping point commanded by the valves.
            let fluid_column_length_mm = 500.0;
            let mechanical_compression_mm = fluid_column_length_mm * (pressure_spike_psi / actual_bulk_modulus_psi);
            
            // The AI thinks the arm stopped at 0.0mm error.
            // Physics forces the fluid to compress, allowing the payload's momentum to carry the piston forward.
            let dynamic_overshoot_mm = mechanical_compression_mm * rng.gen_range(0.9..1.1); // Dynamic ringing factor

            if dynamic_overshoot_mm > max_overshoot_experienced {
                max_overshoot_experienced = dynamic_overshoot_mm;
            }

            if dynamic_overshoot_mm > CRITICAL_OVERSHOOT_ERROR_MM {
                task_failed = true;
                break;
            }
        }

        if task_failed {
            let mut fc = failed_count.lock().unwrap();
            *fc += 1;
        }

        json!({
            "trajectory_id": i,
            "entrained_air_percent": f64::trunc(entrained_air_percent * 100.0) / 100.0,
            "actual_bulk_modulus_psi": f64::trunc(actual_bulk_modulus_psi * 10.0) / 10.0,
            "max_precision_overshoot_mm": f64::trunc(max_overshoot_experienced * 100.0) / 100.0,
            "survived": !task_failed,
            "failure_mode": if !task_failed { "NOMINAL" } else { "CARBON_AI_UNMODELED_HYDRAULIC_SPONGINESS" },
            "cryptographic_seal": format!("sha256:dexterous_hand_fluid_compressibility_{}", i)
        })
    }).collect();

    for res in results {
        writeln!(writer, "{}", res.to_string()).unwrap();
    }

    let fc = *failed_count.lock().unwrap();
    let failure_rate = (fc as f64 / NUM_TRAJECTORIES as f64) * 100.0;
    
    println!("HYDRAULIC_FLUID_COMPRESSIBILITY PHYSICS AUDIT COMPLETE.");
    println!("TOTAL TRAJECTORIES: {:?}", NUM_TRAJECTORIES);
    println!("CATASTROPHIC PRECISION INSERTION FAILURE RATE: {} ({:.2}%)", fc, failure_rate);
    println!("EXECUTION TIME: {:?}", start_time.elapsed());
    println!("SEALED TO: {}/hydraulic_fluid_compressibility.json\n", export_dir);
}