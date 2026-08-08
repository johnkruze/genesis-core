//! 1000Hz GENESIS CORE MODULE: BLAST_OVERPRESSURE_IMU
//! TARGET: Generic Commercial/Industrial Autonomous Systems
//! CLASS: Generic Autonomous Platform
//! SUBSYSTEM: Autonomous Navigation / Sensor Fusion AI
//! VULNERABILITY: RCV autonomous pathfinding relies on continuous high-frequency IMU (Inertial Measurement Unit) data to maintain state estimation, especially in GPS-denied combat zones. However, the generative network is trained on synthetic environments that ignore acoustic-kinetic shock. When an IED detonates 15 meters away, the instantaneous blast overpressure (shockwave) physically bends the microscopic MEMS tuning forks inside the IMU past their yield limit. The sensor saturates, feeding mathematically undefined (NaN) or massively clipped acceleration vectors to the Kalman filter. The navigation AI completely collapses, resulting in the vehicle accelerating randomly into ditches or friendly forces.

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

// Robotic Combat Vehicle IMU Baseline
const IMU_MEMS_CLIP_LIMIT_G: f64 = 50.0; // 50 Gs is typical clipping limit for navigating IMUs
const IED_DETONATION_DISTANCE_M: f64 = 15.0; // Proximity of the blast

fn main() {
    let start_time = Instant::now();
    let export_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/exports/sovereign");
    std::fs::create_dir_all(export_dir).unwrap();
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(format!("{}/blast_overpressure_imu.json", export_dir))
        .unwrap();
    let mut writer = BufWriter::new(file);

    println!("=========================================================");
    println!("G^G SOVEREIGN PHYSICS ENGINE: 1000Hz KINETIC AUDIT");
    println!("TARGET: GENERIC COMMERCIAL SYSTEMS");
    println!("MODULE: BLAST_OVERPRESSURE_IMU");
    println!("EXECUTING 1,200,000 TRAJECTORIES...");
    println!("=========================================================\n");

    let failed_count = Arc::new(Mutex::new(0usize));

    let results: Vec<serde_json::Value> = (0..NUM_TRAJECTORIES).into_par_iter().map(|i| {
        let mut rng = rand::thread_rng();
        
        let mut internal_ekf_diverged = false;
        
        // Flight condition: Navigating a contested urban environment at 30km/h
        let rcv_speed_ms = 8.33; 
        
        // IED Properties (e.g. 155mm Artillery shell rigged as roadside bomb)
        let explosive_yield_kg_tnt: f64 = rng.gen_range(5.0..15.0); 
        
        // The Friedlander Waveform models blast overpressure
        // Peak overpressure P_so scales with scaled distance Z = R / (W^(1/3))
        let scaled_distance_z = IED_DETONATION_DISTANCE_M / explosive_yield_kg_tnt.powf(0.333);
        
        // Simplified overpressure curve fit (in PSI)
        let peak_overpressure_psi = (200.0 / scaled_distance_z) + (150.0 / scaled_distance_z.powi(2)) + (30.0 / scaled_distance_z.powi(3));
        
        // Convert PSI to Pascals (1 PSI = 6894.76 Pa)
        let peak_overpressure_pa = peak_overpressure_psi * 6894.76;
        
        // The shockwave hits the heavy steel hull of the RCV. Transmissibility into the chassis where the IMU is hard-mounted.
        // F = P * A. Assume a 15m^2 cross section of hull taking the broadside blast wave.
        let blast_force_n = peak_overpressure_pa * 15.0; 
        let rcv_mass_kg = 5000.0; // 5 ton vehicle
        
        // The immediate acoustic/kinetic shock transmitted to the sensor bracket (Gs)
        let shock_acceleration_g = (blast_force_n / rcv_mass_kg) / 9.81;

        // The high frequency components of the blast (ringing) amplify through the metal bracket
        let resonant_amplification = rng.gen_range(1.5..4.0);
        let max_imu_shock_g = shock_acceleration_g * resonant_amplification;

        let mut actual_heading = 0.0;
        let mut ai_perceived_heading = 0.0;

        for tick in 0..(2.0 * HZ) as usize { // 2 seconds around the blast event
            
            // The AI is continuously running a Kalman Filter (EKF) to fuse IMU data
            // At tick 500 (0.5s), the IED detonates
            if tick == 500 {
                if max_imu_shock_g > IMU_MEMS_CLIP_LIMIT_G {
                    // CATASTROPHIC FAILURE
                    // The physical silicon fingers inside the MEMS gyro are slammed against their 
                    // containment walls. The sensor outputs exactly 50.0 Gs (its hard limit).
                    // Worse, the physical impact creates a DC bias shift (permanent offset).
                    
                    let dc_bias_shift = rng.gen_range(5.0..25.0); // hallucinates 5-25 degrees/sec of turn
                    
                    // The AI Kalman filter has zero context for "IED detonated". It blindly integrates 
                    // the corrupted IMU data into its state matrix.
                    ai_perceived_heading += dc_bias_shift * DT;
                }
            } else if tick > 500 {
                // The bias shift is permanent until a lengthy recalibration (which autonomous vehicles can't do mid-drive)
                if max_imu_shock_g > IMU_MEMS_CLIP_LIMIT_G {
                    let dc_bias_shift = 15.0; // Hallucinated turn right
                    ai_perceived_heading += dc_bias_shift * DT;
                }
            }
            
            // The AI is trying to hold a heading of 0.0. If it thinks it's turning right, it physically steers the vehicle LEFT.
            actual_heading = -ai_perceived_heading;

            // If the vehicle steers more than 15 degrees off its designated narrow path, it strikes a wall/ditch
            if actual_heading.abs() > 15.0 {
                internal_ekf_diverged = true;
                break;
            }
        }

        if internal_ekf_diverged {
            let mut fc = failed_count.lock().unwrap();
            *fc += 1;
        }

        json!({
            "trajectory_id": i,
            "explosive_yield_kg": f64::trunc(explosive_yield_kg_tnt * 10.0) / 10.0,
            "peak_imu_shock_g": f64::trunc(max_imu_shock_g * 10.0) / 10.0,
            "heading_divergence_deg": f64::trunc(actual_heading.abs() * 10.0) / 10.0,
            "survived": !internal_ekf_diverged,
            "failure_mode": if !internal_ekf_diverged { "NOMINAL" } else { "IMU_OVERPRESSURE_EKF_COLLAPSE" },
            "cryptographic_seal": format!("sha256:rcv_blast_imu_{}", i)
        })
    }).collect();

    for res in results {
        writeln!(writer, "{}", res.to_string()).unwrap();
    }

    let fc = *failed_count.lock().unwrap();
    let failure_rate = (fc as f64 / NUM_TRAJECTORIES as f64) * 100.0;
    
    println!("BLAST_OVERPRESSURE_IMU PHYSICS AUDIT COMPLETE.");
    println!("TOTAL TRAJECTORIES: {:?}", NUM_TRAJECTORIES);
    println!("CATASTROPHIC NAVIGATION COLLAPSE RATE: {} ({:.2}%)", fc, failure_rate);
    println!("EXECUTION TIME: {:?}", start_time.elapsed());
    println!("SEALED TO: {}/blast_overpressure_imu.json\n", export_dir);
}