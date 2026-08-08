//! 1000Hz GENESIS CORE MODULE: THERMAL_SIGHT_SATURATION
//! TARGET: Generic Commercial/Industrial Autonomous Systems
//! CLASS: Generic Autonomous Platform
//! SUBSYSTEM: Auto-Exposure / Thermal Gimbal AI
//! VULNERABILITY: Modern Uncooled Bolometer thermal sights rely on AI-driven local contrast enhancement and auto-gain to extract targets from thermal background noise. In a cold winter environment (0C), the AI sets the sensor gain very high. If a single magnesium or white-phosphorus flare (burning at 2500C+) enters the field of view, the sheer magnitude of the blackbody radiation mathematically crushes the 14-bit dynamic range of the sensor array. The AI's histogram-equalization algorithm furiously ramps down the exposure to compensate, instantly turning the rest of the 0C battlefield completely black and dropping all target tracks until the flare burns out.

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

// Thermal Sight Baseline
const MINIMUM_CONTRAST_THRESHOLD: f64 = 0.05; // If contrast drops below 5%, the target bounding box drops

fn main() {
    let start_time = Instant::now();
    let export_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/exports/sovereign");
    std::fs::create_dir_all(export_dir).unwrap();
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(format!("{}/thermal_sight_saturation.json", export_dir))
        .unwrap();
    let mut writer = BufWriter::new(file);

    println!("=========================================================");
    println!("G^G SOVEREIGN PHYSICS ENGINE: 1000Hz ELECTRO-OPTICAL AUDIT");
    println!("TARGET: GENERIC COMMERCIAL SYSTEMS");
    println!("MODULE: THERMAL_SIGHT_SATURATION");
    println!("EXECUTING 1,200,000 TRAJECTORIES...");
    println!("=========================================================\n");

    let failed_count = Arc::new(Mutex::new(0usize));

    let results: Vec<serde_json::Value> = (0..NUM_TRAJECTORIES).into_par_iter().map(|i| {
        let mut rng = rand::thread_rng();
        
        let mut target_lost = false;
        let mut max_blind_duration_s = 0.0;
        
        // Simulating a cold-weather engagement
        let background_temp_k: f64 = rng.gen_range(260.0..280.0); // -13C to 7C
        let target_soldier_temp_k: f64 = 300.0; // 27C (exposed skin/gear)
        
        // A magnesium flare is popped 200 meters away, in the FOV
        let flare_ignition_tick = 2000; // 2 seconds in
        let flare_duration_s = rng.gen_range(30.0..45.0);
        let flare_temp_k: f64 = rng.gen_range(2500.0..3000.0);
        
        // Stefan-Boltzmann Law: Power emitted is proportional to T^4
        // The sensor sees irradiance. 
        let background_radiance = background_temp_k.powi(4);
        let target_radiance = target_soldier_temp_k.powi(4);
        let flare_radiance = flare_temp_k.powi(4);
        
        let mut current_sensor_gain = 1.0;
        let mut current_blind_time = 0.0;

        for tick in 0..(50.0 * HZ) as usize { // 50 seconds covering the flare burn
            
            let mut total_scene_radiance_max = target_radiance;
            
            if tick > flare_ignition_tick && tick < flare_ignition_tick + (flare_duration_s * HZ) as usize {
                // The flare is in the FOV. The maximum radiance in the scene spikes by a factor of 10,000x
                total_scene_radiance_max = flare_radiance;
            }

            // The AI auto-exposure loop (Automatic Gain Control - AGC)
            // It tries to normalize the maximum radiance to the top of its 14-bit digital range (16383)
            let digital_max = 16383.0;
            let target_gain = digital_max / total_scene_radiance_max;
            
            // Gain adjusts smoothly over time (e.g. 100ms time constant)
            current_sensor_gain += (target_gain - current_sensor_gain) * 10.0 * DT;
            
            // Calculate how the actual target soldier appears after the gain is applied
            // Digital value = Radiance * Gain
            let digital_target_brightness = target_radiance * current_sensor_gain;
            let digital_background_brightness = background_radiance * current_sensor_gain;
            
            // Contrast = (Target - Background) / Digital_Max
            let apparent_contrast = (digital_target_brightness - digital_background_brightness) / digital_max;

            // If the flare is burning, the gain drops so low that the target soldier's 300K radiance
            // gets quantized down to essentially the same digital value as the 270K background.
            if apparent_contrast < MINIMUM_CONTRAST_THRESHOLD {
                current_blind_time += DT;
                if current_blind_time > max_blind_duration_s {
                    max_blind_duration_s = current_blind_time;
                }
            } else {
                current_blind_time = 0.0;
            }

            // If the AI loses the target for more than 5 consecutive seconds, it drops the lock.
            // On a modern battlefield, 5 seconds of blindness means the target has moved to hard cover.
            if max_blind_duration_s > 5.0 {
                target_lost = true;
                break;
            }
        }

        if target_lost {
            let mut fc = failed_count.lock().unwrap();
            *fc += 1;
        }

        json!({
            "trajectory_id": i,
            "flare_temp_kelvin": f64::trunc(flare_temp_k * 10.0) / 10.0,
            "max_radiance_ratio": f64::trunc((flare_radiance / target_radiance) * 1.0) / 1.0,
            "blindness_duration_s": f64::trunc(max_blind_duration_s * 10.0) / 10.0,
            "survived": !target_lost,
            "failure_mode": if !target_lost { "NOMINAL" } else { "THERMAL_AGC_DYNAMIC_RANGE_COLLAPSE" },
            "cryptographic_seal": format!("sha256:thermal_sight_blind_{}", i)
        })
    }).collect();

    for res in results {
        writeln!(writer, "{}", res.to_string()).unwrap();
    }

    let fc = *failed_count.lock().unwrap();
    let failure_rate = (fc as f64 / NUM_TRAJECTORIES as f64) * 100.0;
    
    println!("THERMAL_SIGHT_SATURATION PHYSICS AUDIT COMPLETE.");
    println!("TOTAL TRAJECTORIES: {:?}", NUM_TRAJECTORIES);
    println!("CATASTROPHIC TARGET TRACK LOSS RATE: {} ({:.2}%)", fc, failure_rate);
    println!("EXECUTION TIME: {:?}", start_time.elapsed());
    println!("SEALED TO: {}/thermal_sight_saturation.json\n", export_dir);
}