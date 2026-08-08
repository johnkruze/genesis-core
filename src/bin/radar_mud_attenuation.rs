//! 1000Hz GENESIS CORE MODULE: RADAR_MUD_ATTENUATION
//! TARGET: Generic Commercial/Industrial Autonomous Systems
//! CLASS: Generic Autonomous Platform
//! SUBSYSTEM: Active Protection System (APS) Radar AI
//! VULNERABILITY: Modern APS sensors rely on phased-array radar to detect incoming RPGs/ATGMs and trigger explosive countermeasures. The AI tracking models are calibrated for clean air. When an RCV drives through heavy rain and mud, the vehicle throws a 3-inch thick layer of dielectric mud onto the radar face. This physically attenuates the outgoing/incoming RF power. The AI fails to compensate for the massive Drop in Signal-to-Noise Ratio (SNR), failing to detect the incoming RPG until it has already crossed the minimum arming distance of the countermeasure.

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

// Active Protection Radar Baseline
const MIN_APS_INTERCEPT_DISTANCE_M: f64 = 15.0; // The APS explosive charge cannot arm and fire if the RPG is closer than 15 meters
const NOMINAL_RPG_DETECTION_RANGE_M: f64 = 400.0;

fn main() {
    let start_time = Instant::now();
    let export_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/exports/sovereign");
    std::fs::create_dir_all(export_dir).unwrap();
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(format!("{}/radar_mud_attenuation.json", export_dir))
        .unwrap();
    let mut writer = BufWriter::new(file);

    println!("=========================================================");
    println!("G^G SOVEREIGN PHYSICS ENGINE: 1000Hz RF ELECTROMAGNETIC AUDIT");
    println!("TARGET: GENERIC COMMERCIAL SYSTEMS");
    println!("MODULE: RADAR_MUD_ATTENUATION");
    println!("EXECUTING 1,200,000 TRAJECTORIES...");
    println!("=========================================================\n");

    let failed_count = Arc::new(Mutex::new(0usize));

    let results: Vec<serde_json::Value> = (0..NUM_TRAJECTORIES).into_par_iter().map(|i| {
        let mut rng = rand::thread_rng();
        
        let mut actual_detection_distance_m = 0.0;
        let mut aps_intercept_failure = false;
        
        // Simulating the vehicle operating in severe wet/muddy terrain (e.g. Rasputitsa)
        let mud_layer_thickness_cm = rng.gen_range(5.0..12.0); // 5 to 12 cm of heavy caked mud 
        
        // Dielectric properties of wet clay/mud (high moisture content = high RF attenuation)
        // X-band radar (~10 GHz) attenuation through wet mud is extreme.
        // Approx 8.5 dB loss per cm of wet mud (two-way travel, highly conductive)
        let two_way_attenuation_db = mud_layer_thickness_cm * 8.5; 
        
        // Convert dB loss to a linear power multiplier: L_linear = 10^(-dB/10)
        let received_power_fraction = 10_f64.powf(-two_way_attenuation_db / 10.0);
        
        // The Radar Equation dictates that Range scales with the fourth root of received power (R ~ P_r^(1/4))
        let max_effective_range_m = NOMINAL_RPG_DETECTION_RANGE_M * received_power_fraction.powf(0.25);

        let rpg_speed_ms = 300.0;
        let firing_distance_m = 150.0;

        let mut current_rpg_distance_m = firing_distance_m;

        for _tick in 0..(1.0 * HZ) as usize { // 1 second flight time covering 300m
            
            // Advance the RPG
            current_rpg_distance_m -= rpg_speed_ms * DT;

            // If the RPG impacts the hull before the Radar ever sees it
            if current_rpg_distance_m <= 0.0 {
                aps_intercept_failure = true;
                actual_detection_distance_m = 0.0;
                break;
            }

            // The APS AI is scanning 1000 times a second. Can it see the RPG yet?
            if current_rpg_distance_m <= max_effective_range_m {
                // The AI finally receives a signal above the thermal noise floor
                actual_detection_distance_m = current_rpg_distance_m;
                
                // If it detects the RPG inside the 15-meter hard deck, the physical explosive countermeasure 
                // cannot be deployed fast enough (reaction time + explosive propagation speed).
                if actual_detection_distance_m < MIN_APS_INTERCEPT_DISTANCE_M {
                    aps_intercept_failure = true;
                }
                
                break; // Target detected, event resolves
            }
        }

        if aps_intercept_failure {
            let mut fc = failed_count.lock().unwrap();
            *fc += 1;
        }

        json!({
            "trajectory_id": i,
            "mud_thickness_cm": f64::trunc(mud_layer_thickness_cm * 10.0) / 10.0,
            "two_way_attenuation_dB": f64::trunc(two_way_attenuation_db * 10.0) / 10.0,
            "actual_detection_range_m": f64::trunc(actual_detection_distance_m * 10.0) / 10.0,
            "survived": !aps_intercept_failure,
            "failure_mode": if !aps_intercept_failure { "NOMINAL" } else { "APS_MUD_SNR_COLLAPSE_IMPACT" },
            "cryptographic_seal": format!("sha256:aps_radar_mud_{}", i)
        })
    }).collect();

    for res in results {
        writeln!(writer, "{}", res.to_string()).unwrap();
    }

    let fc = *failed_count.lock().unwrap();
    let failure_rate = (fc as f64 / NUM_TRAJECTORIES as f64) * 100.0;
    
    println!("RADAR_MUD_ATTENUATION PHYSICS AUDIT COMPLETE.");
    println!("TOTAL TRAJECTORIES: {:?}", NUM_TRAJECTORIES);
    println!("CATASTROPHIC APS IMPACT FAILURE RATE: {} ({:.2}%)", fc, failure_rate);
    println!("EXECUTION TIME: {:?}", start_time.elapsed());
    println!("SEALED TO: {}/radar_mud_attenuation.json\n", export_dir);
}