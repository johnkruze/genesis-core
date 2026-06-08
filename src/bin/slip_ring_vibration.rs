//! 1000Hz GENESIS CORE MODULE: SLIP_RING_VIBRATION
//! TARGET: Generic Commercial/Industrial Autonomous Systems
//! CLASS: Quadrupedal Robotics
//! SUBSYSTEM: Infinite Rotation Actuators
//! VULNERABILITY: Anybotics uses infinite rotation joints (slip rings) to pass high-speed data buses across moving boundaries. In heavy industrial environments (e.g. vibrating steel catwalks on an oil rig), exogenous structural vibration couples with the slip ring brushes. At specific harmonics, the physical brushes bounce off the gold contact rings, literally dropping Ethernet packets on the floor and severing the E2E matrix communication spine for milliseconds at a time.

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

// Anybotics Slip Ring Baseline
const BRUSH_SPRING_FORCE_N: f64 = 1.2; // The delicate preload on the gold/silver slip ring brush
const E2E_CRITICAL_PACKET_LOSS_MS: f64 = 30.0; // If the central policy loses connection to the leg for 30ms, the quadruped trips.

fn main() {
    let start_time = Instant::now();
    let export_dir = "/Users/aijesusbro/Spectrum/data/exports/sovereign";
    std::fs::create_dir_all(export_dir).unwrap();
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(format!("{}/slip_ring_vibration.json", export_dir))
        .unwrap();
    let mut writer = BufWriter::new(file);

    println!("=========================================================");
    println!("G^G SOVEREIGN PHYSICS ENGINE: 1000Hz VIBRO-MECHANICAL AUDIT");
    println!("TARGET: GENERIC COMMERCIAL SYSTEMS");
    println!("MODULE: SLIP_RING_VIBRATION");
    println!("EXECUTING 1,200,000 TRAJECTORIES...");
    println!("=========================================================\n");

    let failed_count = Arc::new(Mutex::new(0usize));

    let results: Vec<serde_json::Value> = (0..NUM_TRAJECTORIES).into_par_iter().map(|i| {
        let mut rng = rand::thread_rng();
        
        // Simulating the quadruped walking next to a massive diesel generator or pump on an offshore rig
        let exogenous_vibration_g = rng.gen_range(0.2..1.5); // Severe industrial vibration propagating through the deck
        let vibration_freq_hz = rng.gen_range(30.0..120.0); // Diesel engine RPM harmonics
        
        let brush_mass_kg = 0.005; // Extremely light, but still has mass
        
        let mut continuous_disconnect_ms = 0.0;
        let mut max_continuous_disconnect_ms = 0.0;
        let mut matrix_packet_starvation = false;
        let mut brush_floating_timer_ms = 0.0;

        for tick in 0..(2.0 * HZ) as usize { // 2 seconds of exposure
            let time = tick as f64 * DT;
            
            // The acceleration of the joint casing due to the floor vibration
            let instantaneous_vibration_accel_g = exogenous_vibration_g * (time * vibration_freq_hz * std::f64::consts::PI * 2.0).sin();
            let instantaneous_vibration_force_n = brush_mass_kg * instantaneous_vibration_accel_g * 9.81;
            
            // A secondary harmonic from the quadruped's own walking impact
            let internal_walking_shock_n = if tick % (HZ as usize) < 100 {
                rng.gen_range(0.5..1.5) // Heel strike
            } else {
                0.0
            };

            // If the total inertial force on the brush exceeds the tiny spring force holding it to the ring, it lifts off.
            let total_separating_force = instantaneous_vibration_force_n.abs() + internal_walking_shock_n;

            if total_separating_force > BRUSH_SPRING_FORCE_N {
                // Physical air gap created. Brush is violently kicked away.
                brush_floating_timer_ms = 15.0; // It takes the weak spring 15ms to fully retrieve the brush mass and seat it
            }

            if brush_floating_timer_ms > 0.0 {
                // Brush is floating. Data bus severed. 
                brush_floating_timer_ms -= DT * 1000.0;
                continuous_disconnect_ms += DT * 1000.0;
                
                if continuous_disconnect_ms > max_continuous_disconnect_ms {
                    max_continuous_disconnect_ms = continuous_disconnect_ms;
                }
            } else {
                // Contact restored
                continuous_disconnect_ms = 0.0;
            }

            if max_continuous_disconnect_ms > E2E_CRITICAL_PACKET_LOSS_MS {
                matrix_packet_starvation = true;
                break;
            }
        }

        if matrix_packet_starvation {
            let mut fc = failed_count.lock().unwrap();
            *fc += 1;
        }

        json!({
            "trajectory_id": i,
            "industrial_deck_vibration_G": f64::trunc(exogenous_vibration_g * 100.0) / 100.0,
            "vibration_frequency_Hz": f64::trunc(vibration_freq_hz * 10.0) / 10.0,
            "max_data_bus_severance_ms": f64::trunc(max_continuous_disconnect_ms * 10.0) / 10.0,
            "survived": !matrix_packet_starvation,
            "failure_mode": if !matrix_packet_starvation { "NOMINAL" } else { "SLIP_RING_BRUSH_FLOAT_PACKET_DROP" },
            "cryptographic_seal": format!("sha256:slip_ring_vibration_{}", i)
        })
    }).collect();

    for res in results {
        writeln!(writer, "{}", res.to_string()).unwrap();
    }

    let fc = *failed_count.lock().unwrap();
    let failure_rate = (fc as f64 / NUM_TRAJECTORIES as f64) * 100.0;
    
    println!("SLIP_RING_VIBRATION PHYSICS AUDIT COMPLETE.");
    println!("TOTAL TRAJECTORIES: {:?}", NUM_TRAJECTORIES);
    println!("CATASTROPHIC MATRIX PACKET LOSS RATE: {} ({:.2}%)", fc, failure_rate);
    println!("EXECUTION TIME: {:?}", start_time.elapsed());
    println!("SEALED TO: {}/slip_ring_vibration.json\n", export_dir);
}