//! 1000Hz GENESIS CORE MODULE: COMMERCIAL STRUCTURAL MECHANICS EDGE CASES
//! TARGET: Generic Commercial/Industrial Autonomous Systems
//! CLASS: UGVs, Bipeds, Heavy Tracked Systems
//! SUBSYSTEM: Harmonic Fatigue & Torsional Yielding

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
        .open(format!("{}/lib_structural.jsonl", export_dir))
        .unwrap();
    let mut writer = BufWriter::new(file);

    println!("=========================================================");
    println!("G^G SOVEREIGN PHYSICS ENGINE: COMMERCIAL WHITE-LABEL");
    println!("LIBRARY: STRUCTURAL EXHAUSTION & FATIGUE");
    println!("EXECUTING {} TRAJECTORIES...", NUM_TRAJECTORIES);
    println!("=========================================================\n");

    let failed_count = Arc::new(Mutex::new(0usize));

    let results: Vec<String> = (0..NUM_TRAJECTORIES).into_par_iter().map(|i| {
        let mut rng = rand::thread_rng();
        
        let mass_payload_kg = rng.gen_range(50.0..5000.0);
        let structural_yield_threshold_nm = mass_payload_kg * rng.gen_range(5.0..20.0);
        
        let terrain_roughness_hz = rng.gen_range(1.0..10.0);
        let mut current_fatigue_index = 0.0;
        let fatigue_limit = 1_000_000.0;

        let mut structural_fracture = false;
        let mut max_torque_nm = 0.0;

        for tick in 0..(120.0 * HZ) as usize { 
            let time_s = tick as f64 * DT;
            
            let ai_acceleration_ms2: f64 = rng.gen_range(-5.0..5.0);
            
            let dynamic_lever_m = 0.5 + (time_s * terrain_roughness_hz * 2.0 * std::f64::consts::PI).sin() * 0.2;
            let instantaneous_torque = (mass_payload_kg * 9.81 + mass_payload_kg * ai_acceleration_ms2.abs()) * dynamic_lever_m;
            
            if instantaneous_torque > max_torque_nm {
                max_torque_nm = instantaneous_torque;
            }

            if instantaneous_torque > structural_yield_threshold_nm * 0.7 {
                current_fatigue_index += instantaneous_torque * DT * 100.0; 
            }

            if instantaneous_torque > structural_yield_threshold_nm || current_fatigue_index > fatigue_limit {
                structural_fracture = true;
                break;
            }
        }

        if structural_fracture {
            let mut fc = failed_count.lock().unwrap();
            *fc += 1;
        }

        let mut hasher = Sha256::new();
        let sig_payload = format!("struct_{}_{}_{}", i, max_torque_nm, current_fatigue_index);
        hasher.update(sig_payload.as_bytes());
        let hash = hex::encode(hasher.finalize());

        let out = json!({
            "trajectory_id": i,
            "payload_mass_kg": f64::trunc(mass_payload_kg * 10.0) / 10.0,
            "terrain_frequency_hz": f64::trunc(terrain_roughness_hz * 10.0) / 10.0,
            "max_experienced_torque_nm": f64::trunc(max_torque_nm * 10.0) / 10.0,
            "torsional_strain_mpa": f64::trunc((max_torque_nm / 0.03) / 1_000_000.0 * 100.0) / 100.0, // Rough Pa calculation given 3cm radius shaft
            "fatigue_accrued": f64::trunc(current_fatigue_index * 1.0) / 1.0,
            "survived": !structural_fracture,
            "failure_mode": if structural_fracture { if current_fatigue_index > fatigue_limit { "HARMONIC_FATIGUE_FAILURE" } else { "INSTANT_YIELD_FRACTURE" } } else { "NOMINAL" },
            "cryptographic_seal": hash
        });
        
        serde_json::to_string(&out).unwrap()
    }).collect();

    for res in results {
        writeln!(writer, "{}", res).unwrap();
    }

    let fc = *failed_count.lock().unwrap();
    println!("STRUCTURAL LIBRARY GENERATION COMPLETE.");
    println!("TOTAL TRAJECTORIES: {:?}", NUM_TRAJECTORIES);
    println!("FAILURE YIELD: {} ({:.2}%)", fc, (fc as f64 / NUM_TRAJECTORIES as f64) * 100.0);
    println!("EXECUTION TIME: {:?}", start_time.elapsed());
}
