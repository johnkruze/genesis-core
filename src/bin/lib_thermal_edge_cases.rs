//! 1000Hz GENESIS CORE MODULE: COMMERCIAL THERMAL EDGE CASES
//! TARGET: Generic Commercial/Industrial Autonomous Systems
//! CLASS: Electric Actuators, High-Density Packs
//! SUBSYSTEM: Thermal Runaway & I^2R Bus Collapse

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
        .open(format!("{}/lib_thermal.jsonl", export_dir))
        .unwrap();
    let mut writer = BufWriter::new(file);

    println!("=========================================================");
    println!("G^G SOVEREIGN PHYSICS ENGINE: COMMERCIAL WHITE-LABEL");
    println!("LIBRARY: THERMAL & BATTERY EDGE CASES");
    println!("EXECUTING {} TRAJECTORIES...", NUM_TRAJECTORIES);
    println!("=========================================================\n");

    let failed_count = Arc::new(Mutex::new(0usize));

    let results: Vec<String> = (0..NUM_TRAJECTORIES).into_par_iter().map(|i| {
        let mut rng = rand::thread_rng();
        
        let initial_ambient_c = rng.gen_range(20.0..60.0);
        let mass_kg = rng.gen_range(0.5..4.0); // Individual joint actuator mass
        let specific_heat_capacity = rng.gen_range(300.0..600.0);
        
        let mut current_temp_c = initial_ambient_c;
        let mut thermal_runaway = false;
        
        // Electric parameters
        let initial_voltage = 400.0;
        let mut battery_bus_voltage = initial_voltage;
        let base_resistance_ohms = rng.gen_range(0.05..0.2);
        
        let mut max_temp = current_temp_c;
        let mut max_dissipation_rate_w = 0.0;

        let mut ai_load_demand = 0.0;

        for tick in 0..(30.0 * HZ) as usize {
            if tick % 1000 == 0 {
                ai_load_demand = rng.gen_range(0.8..3.5); // Sustained stuck-joint peak loads
            } 
            
            // Peak current draw
            let current_draw_amps = ai_load_demand * 150.0;
            
            // I^2R resistance heating spikes geometrically with current draw
            let dynamic_resistance = base_resistance_ohms * (1.0 + (current_temp_c * 0.004));
            let resistive_heating_w = current_draw_amps * current_draw_amps * dynamic_resistance;
            
            // Total heat generated in Jules per tick
            let heat_generated_j = resistive_heating_w * DT;
            let temp_delta = heat_generated_j / (mass_kg * specific_heat_capacity);
            
            // Dissipation is overwhelmed by high ambient environments
            let dissipation_rate_w = ((current_temp_c - initial_ambient_c) * 15.0).max(0.0);
            let cooling_delta = (dissipation_rate_w * DT) / (mass_kg * specific_heat_capacity);
            
            current_temp_c += temp_delta - cooling_delta;
            
            if dissipation_rate_w > max_dissipation_rate_w {
                max_dissipation_rate_w = dissipation_rate_w;
            }

            if current_temp_c > max_temp {
                max_temp = current_temp_c;
            }

            // High load sags the bus voltage
            let voltage_drop = current_draw_amps * dynamic_resistance;
            battery_bus_voltage = initial_voltage - voltage_drop;

            // Runaway bounds (lithium dendrite punch-through typically > 130C)
            // Voltage bounds (sagging below 280V triggers ECU fault shutdown)
            if current_temp_c > 130.0 || battery_bus_voltage < 280.0 {
                thermal_runaway = true;
                break;
            }
        }

        if thermal_runaway {
            let mut fc = failed_count.lock().unwrap();
            *fc += 1;
        }

        let mut hasher = Sha256::new();
        let sig_payload = format!("thermal_{}_{}_{}", i, max_temp, battery_bus_voltage);
        hasher.update(sig_payload.as_bytes());
        let hash = hex::encode(hasher.finalize());

        let out = json!({
            "trajectory_id": i,
            "actuator_mass_kg": f64::trunc(mass_kg * 100.0) / 100.0,
            "ambient_start_c": f64::trunc(initial_ambient_c * 100.0) / 100.0,
            "max_core_temp_c": f64::trunc(max_temp * 100.0) / 100.0,
            "internal_resistance_ohms": f64::trunc(base_resistance_ohms * 1000.0) / 1000.0,
            "peak_heat_dissipation_w": f64::trunc(max_dissipation_rate_w * 10.0) / 10.0,
            "battery_bus_voltage_sag": f64::trunc(battery_bus_voltage * 100.0) / 100.0,
            "survived": !thermal_runaway,
            "failure_mode": if thermal_runaway { if max_temp > 130.0 { "THERMAL_SATURATION" } else { "BUS_VOLTAGE_COLLAPSE" } } else { "NOMINAL" },
            "cryptographic_seal": hash
        });
        
        serde_json::to_string(&out).unwrap()
    }).collect();

    for res in results {
        writeln!(writer, "{}", res).unwrap();
    }

    let fc = *failed_count.lock().unwrap();
    println!("THERMAL LIBRARY GENERATION COMPLETE.");
    println!("TOTAL TRAJECTORIES: {:?}", NUM_TRAJECTORIES);
    println!("FAILURE YIELD: {} ({:.2}%)", fc, (fc as f64 / NUM_TRAJECTORIES as f64) * 100.0);
    println!("EXECUTION TIME: {:?}", start_time.elapsed());
}
