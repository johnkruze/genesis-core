//! 1000Hz GENESIS CORE MODULE: COMMERCIAL SENSOR PHYSICS EDGE CASES
//! TARGET: Generic Commercial/Industrial Autonomous Systems
//! CLASS: Autonomous Vehicles, Drones, UGVs
//! SUBSYSTEM: Extended Kalman Filter (EKF) Divergence & Optical Occlusion

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
        .open(format!("{}/lib_sensor.jsonl", export_dir))
        .unwrap();
    let mut writer = BufWriter::new(file);

    println!("=========================================================");
    println!("G^G SOVEREIGN PHYSICS ENGINE: COMMERCIAL WHITE-LABEL");
    println!("LIBRARY: SENSOR PHYSICS (EKF DIVERGENCE)");
    println!("EXECUTING {} TRAJECTORIES...", NUM_TRAJECTORIES);
    println!("=========================================================\n");

    let failed_count = Arc::new(Mutex::new(0usize));

    let results: Vec<String> = (0..NUM_TRAJECTORIES).into_par_iter().map(|i| {
        let mut rng = rand::thread_rng();
        
        let imu_update_rate_hz = 500.0; 
        let optical_framerate_hz = 30.0; 
        
        // Massive stroboscopic vibrational drift
        let structural_vibration_hz = rng.gen_range(480.0..520.0); 
        let vibration_amplitude_g = rng.gen_range(2.0..15.0);
        
        // Emulate entering a dust cloud or severe sun glare
        let occlusion_start_s = rng.gen_range(10.0..40.0);
        let occlusion_duration_s = rng.gen_range(1.0..10.0);
        
        let mut true_position_x = 0.0;
        let mut ekf_estimated_x = 0.0;
        let mut navigation_divergence = false;

        let mut ekf_covariance = 0.01;
        let mut max_covariance = ekf_covariance;

        for tick in 0..(60.0 * HZ) as usize { 
            
            let time_s = tick as f64 * DT;
            
            // True velocity
            true_position_x += 5.0 * DT;
            
            // Aliasing: beat frequency between vibration and IMU sampling
            // If vibration is highly harmonic, the IMU interprets it as a DC bias
            let alias_phase = time_s * structural_vibration_hz * 2.0 * std::f64::consts::PI;
            let aliased_noise = alias_phase.sin() * vibration_amplitude_g;
            
            if tick % (HZ / imu_update_rate_hz) as usize == 0 {
                // Bias injected directly into the velocity estimate
                ekf_estimated_x += (5.0 + aliased_noise) * (1.0 / imu_update_rate_hz);
                ekf_covariance += 0.05 * vibration_amplitude_g; // Uncertainty grows aggressively under vibration
            }

            // Optical flow correction
            if tick % (HZ / optical_framerate_hz) as usize == 0 {
                let optical_confidence = if vibration_amplitude_g > 5.0 { 0.2 } else { 0.95 };
                
                // If occluded (dust/glare), vision goes blind, meaning kalman gain drops to zero.
                // The EKF must coast entirely on the aliased IMU.
                if time_s > occlusion_start_s && time_s < (occlusion_start_s + occlusion_duration_s) {
                    // Fully blind. No measurement update step occurs.
                    ekf_covariance += 0.5; // Uncertainty skyrockets
                } else {
                    // Introduce slight measurement noise, it's not perfect ground truth
                    let optical_measurement = true_position_x + rng.gen_range(-0.1..0.1);
                    let innovation = optical_measurement - ekf_estimated_x;
                    let kalman_gain = ekf_covariance / (ekf_covariance + (1.0 - optical_confidence));
                    
                    ekf_estimated_x += kalman_gain * innovation;
                    ekf_covariance = (1.0 - kalman_gain) * ekf_covariance;
                }
            }
            
            if ekf_covariance > max_covariance {
                max_covariance = ekf_covariance;
            }

            let drift_error = (true_position_x - ekf_estimated_x).abs();

            // 3 meters of blind translation error means the AMR/Drone strikes a wall
            if drift_error > 3.0 && time_s > 5.0 { 
                navigation_divergence = true;
                break;
            }
        }

        if navigation_divergence {
            let mut fc = failed_count.lock().unwrap();
            *fc += 1;
        }

        let mut hasher = Sha256::new();
        let sig_payload = format!("sensor_{}_{}_{}", i, structural_vibration_hz, ekf_covariance);
        hasher.update(sig_payload.as_bytes());
        let hash = hex::encode(hasher.finalize());

        let out = json!({
            "trajectory_id": i,
            "structural_vibration_hz": f64::trunc(structural_vibration_hz * 10.0) / 10.0,
            "vibration_amplitude_g": f64::trunc(vibration_amplitude_g * 10.0) / 10.0,
            "optical_occlusion_duration_s": f64::trunc(occlusion_duration_s * 10.0) / 10.0,
            "final_ekf_drift_error_m": f64::trunc((true_position_x - ekf_estimated_x).abs() * 100.0) / 100.0,
            "ekf_covariance_trace_matrix": f64::trunc(max_covariance * 1000.0) / 1000.0,
            "survived": !navigation_divergence,
            "failure_mode": if navigation_divergence { "EKF_ALIASING_DIVERGENCE" } else { "NOMINAL" },
            "cryptographic_seal": hash
        });
        
        serde_json::to_string(&out).unwrap()
    }).collect();

    for res in results {
        writeln!(writer, "{}", res).unwrap();
    }

    let fc = *failed_count.lock().unwrap();
    println!("SENSOR LIBRARY GENERATION COMPLETE.");
    println!("TOTAL TRAJECTORIES: {:?}", NUM_TRAJECTORIES);
    println!("FAILURE YIELD: {} ({:.2}%)", fc, (fc as f64 / NUM_TRAJECTORIES as f64) * 100.0);
    println!("EXECUTION TIME: {:?}", start_time.elapsed());
}
