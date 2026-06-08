//! 1000Hz GENESIS CORE MODULE: COMMERCIAL FLUID & AERODYNAMIC EDGE CASES
//! TARGET: Generic Commercial/Industrial Autonomous Systems
//! CLASS: eVTOL, Heavy-Lift Drones, Hydrofoils
//! SUBSYSTEM: Vortex Ring State (VRS) & Wake Starvation

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
        .open(format!("{}/lib_fluid.jsonl", export_dir))
        .unwrap();
    let mut writer = BufWriter::new(file);

    println!("=========================================================");
    println!("G^G SOVEREIGN PHYSICS ENGINE: COMMERCIAL WHITE-LABEL");
    println!("LIBRARY: FLUID & AERODYNAMICS (VRS)");
    println!("EXECUTING {} TRAJECTORIES...", NUM_TRAJECTORIES);
    println!("=========================================================\n");

    let failed_count = Arc::new(Mutex::new(0usize));

    let results: Vec<String> = (0..NUM_TRAJECTORIES).into_par_iter().map(|i| {
        let mut rng = rand::thread_rng();
        
        let rotor_induced_velocity_ms = rng.gen_range(8.0..25.0);
        let mass_kg = rng.gen_range(500.0..3000.0);
        
        let mut descent_velocity_ms = 0.0;
        let mut altitude_m = rng.gen_range(100.0..500.0);
        let mut vrs_collapse = false;
        
        let mut max_vorticity_ratio = 0.0;

        for _ in 0..(40.0 * HZ) as usize { 
            let commanded_descent_rate = rng.gen_range(4.0..20.0);
            
            descent_velocity_ms += (commanded_descent_rate - descent_velocity_ms) * 0.5 * DT; 
            
            let vrs_ratio = descent_velocity_ms / rotor_induced_velocity_ms;
            
            if vrs_ratio > max_vorticity_ratio {
                max_vorticity_ratio = vrs_ratio;
            }
            
            if vrs_ratio > 0.75 && vrs_ratio < 1.35 {
                let thrust_loss_factor = 1.0 - ((vrs_ratio - 1.0).abs() * 3.5).max(0.0).min(0.85); 
                
                let thrust_available = (mass_kg * 9.81 * 1.5) * thrust_loss_factor; 
                let acceleration = (thrust_available / mass_kg) - 9.81;
                
                descent_velocity_ms -= acceleration * DT; 
            } else {
                descent_velocity_ms = commanded_descent_rate; 
            }

            altitude_m -= descent_velocity_ms * DT;

            if altitude_m < 5.0 && descent_velocity_ms > 12.0 {
                vrs_collapse = true;
                break;
            }
        }

        if vrs_collapse {
            let mut fc = failed_count.lock().unwrap();
            *fc += 1;
        }

        let mut hasher = Sha256::new();
        let sig_payload = format!("fluid_{}_{}_{}", i, rotor_induced_velocity_ms, descent_velocity_ms);
        hasher.update(sig_payload.as_bytes());
        let hash = hex::encode(hasher.finalize());

        let out = json!({
            "trajectory_id": i,
            "rotor_induced_velocity_ms": f64::trunc(rotor_induced_velocity_ms * 100.0) / 100.0,
            "terminal_descent_velocity_ms": f64::trunc(descent_velocity_ms * 100.0) / 100.0,
            "airframe_mass_kg": f64::trunc(mass_kg * 10.0) / 10.0,
            "peak_vorticity_ratio": f64::trunc(max_vorticity_ratio * 1000.0) / 1000.0,
            "survived": !vrs_collapse,
            "failure_mode": if vrs_collapse { "VRS_FULL_COLLAPSE" } else { "NOMINAL" },
            "cryptographic_seal": hash
        });
        
        serde_json::to_string(&out).unwrap()
    }).collect();

    for res in results {
        writeln!(writer, "{}", res).unwrap();
    }

    let fc = *failed_count.lock().unwrap();
    println!("FLUID LIBRARY GENERATION COMPLETE.");
    println!("TOTAL TRAJECTORIES: {:?}", NUM_TRAJECTORIES);
    println!("FAILURE YIELD: {} ({:.2}%)", fc, (fc as f64 / NUM_TRAJECTORIES as f64) * 100.0);
    println!("EXECUTION TIME: {:?}", start_time.elapsed());
}
