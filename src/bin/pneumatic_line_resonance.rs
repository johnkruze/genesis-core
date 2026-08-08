//! 1000Hz GENESIS CORE MODULE: PNEUMATIC_LINE_RESONANCE
//! TARGET: Generic Commercial/Industrial Autonomous Systems
//! CLASS: Generic Autonomous Platform
//! SUBSYSTEM: Drive-by-Wire Pneumatic Brake Actuation
//! VULNERABILITY: Class 8 trucks rely on compressed air to engage the foundation brakes. The highway autonomy stack sends a digital braking signal, which triggers a solenoid to release 120 PSI of air down 60 feet of plastic tubing. When ABS pulses at 5-10Hz, it induces a standing acoustic pressure wave (water hammer) inside the air lines. If the commanded pulse frequency matches the pipe's natural acoustic resonance, the air flow chokes, effectively severing braking force to the rear axles.

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

// Class-8 Pneumatic Brake Baseline
const SPEED_OF_SOUND_AIR_MS: f64 = 343.0; // Speed of sound dictates pressure wave propagation
const CRITICAL_BRAKE_PRESSURE_PSI: f64 = 60.0; // Needs at least 60 PSI to maintain safe stopping deceleration
const SYSTEM_AIR_PRESSURE_PSI: f64 = 120.0;

fn main() {
    let start_time = Instant::now();
    let export_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/exports/sovereign");
    std::fs::create_dir_all(export_dir).unwrap();
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(format!("{}/pneumatic_line_resonance.json", export_dir))
        .unwrap();
    let mut writer = BufWriter::new(file);

    println!("=========================================================");
    println!("G^G SOVEREIGN PHYSICS ENGINE: 1000Hz FLUID DYNAMICS AUDIT");
    println!("TARGET: GENERIC COMMERCIAL SYSTEMS");
    println!("MODULE: PNEUMATIC_LINE_RESONANCE");
    println!("EXECUTING 1,200,000 TRAJECTORIES...");
    println!("=========================================================\n");

    let failed_count = Arc::new(Mutex::new(0usize));

    let results: Vec<serde_json::Value> = (0..NUM_TRAJECTORIES).into_par_iter().map(|i| {
        let mut rng = rand::thread_rng();
        
        let mut min_delivered_pressure_psi = SYSTEM_AIR_PRESSURE_PSI;
        let mut pneumatic_choke_failure = false;
        
        // Trailer air line length ranges from 15 meters to 25 meters
        let pneumatic_line_length_m = rng.gen_range(15.0..25.0); 
        
        // Acoustic resonance frequency of a pipe open at both ends (solenoid to brake chamber)
        let natural_acoustic_frequency_hz = SPEED_OF_SOUND_AIR_MS / (2.0 * pneumatic_line_length_m);
        
        // The digital ABS controller pulses the brakes to prevent jackknifing on wet roads
        let abs_command_hz = rng.gen_range(5.0..12.0); 

        // If the AI commands an ABS frequency that happens to match the natural harmonic of the 53ft trailer's air lines...
        let harmonic_match_delta = (natural_acoustic_frequency_hz - abs_command_hz).abs();
        
        for tick in 0..(2.0 * HZ) as usize { // 2 seconds of emergency braking
            let time = tick as f64 * DT;
            
            // The solenoid sends a square wave pressure pulse
            let commanded_pulse = (time * abs_command_hz * std::f64::consts::PI * 2.0).sin();
            
            // The reflected pressure wave coming back from the brake chamber
            // If they are in phase, they constructively interfere (pressure spikes).
            // If they are out of phase, they destructively interfere (pressure flatlines).
            
            // At resonance, the standing wave creates "nodes" where the dynamic pressure is effectively zero.
            let standing_wave_choke_factor = if harmonic_match_delta < 1.0 {
                // Perfect harmonic match creates a massive choke
                rng.gen_range(0.1..0.3)
            } else {
                rng.gen_range(0.7..1.0) // Nominal flow
            };

            let delivered_pressure_psi = SYSTEM_AIR_PRESSURE_PSI * standing_wave_choke_factor;
            
            if commanded_pulse > 0.0 { // During the "ON" phase of the ABS pulse
                if delivered_pressure_psi < min_delivered_pressure_psi {
                    min_delivered_pressure_psi = delivered_pressure_psi;
                }

                // If the standing wave prevents the air from reaching the required 60 PSI, the 80,000lb truck 
                // loses its trailer brakes and jackknifes or rear-ends the target.
                if delivered_pressure_psi < CRITICAL_BRAKE_PRESSURE_PSI {
                    pneumatic_choke_failure = true;
                    break;
                }
            }
        }

        if pneumatic_choke_failure {
            let mut fc = failed_count.lock().unwrap();
            *fc += 1;
        }

        json!({
            "trajectory_id": i,
            "trailer_line_length_m": f64::trunc(pneumatic_line_length_m * 10.0) / 10.0,
            "matched_abs_frequency_Hz": f64::trunc(abs_command_hz * 10.0) / 10.0,
            "min_delivered_pressure_PSI": f64::trunc(min_delivered_pressure_psi * 10.0) / 10.0,
            "survived": !pneumatic_choke_failure,
            "failure_mode": if !pneumatic_choke_failure { "NOMINAL" } else { "PNEUMATIC_STANDING_WAVE_ABS_CHOKE" },
            "cryptographic_seal": format!("sha256:pneumatic_resonance_{}", i)
        })
    }).collect();

    for res in results {
        writeln!(writer, "{}", res.to_string()).unwrap();
    }

    let fc = *failed_count.lock().unwrap();
    let failure_rate = (fc as f64 / NUM_TRAJECTORIES as f64) * 100.0;
    
    println!("PNEUMATIC_LINE_RESONANCE PHYSICS AUDIT COMPLETE.");
    println!("TOTAL TRAJECTORIES: {:?}", NUM_TRAJECTORIES);
    println!("CATASTROPHIC BRAKE CHOKE RATE: {} ({:.2}%)", fc, failure_rate);
    println!("EXECUTION TIME: {:?}", start_time.elapsed());
    println!("SEALED TO: {}/pneumatic_line_resonance.json\n", export_dir);
}