//! 1000Hz GENESIS CORE MODULE: COMPOUNDING_MONTE_CARLO
//! TARGET: Biological Systems & Pharmaceutical R&D
//! CLASS: Human Body Digital Twin / Sterile Biotech Process
//! SUBSYSTEM: Gastric Dissolution, Vascular Rheology, and Bioreactor Shear Audits

use genesis_core::physics::compounding::CompoundingState;
use rayon::prelude::*;
use serde_json::json;
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::time::Instant;
use rand::Rng;

const NUM_TRAJECTORIES_PER_SCENARIO: usize = 1000;
const HZ: f64 = 1000.0;
const DT: f64 = 1.0 / HZ;

fn main() {
    let start_time = Instant::now();
    let export_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/exports/sovereign");
    std::fs::create_dir_all(export_dir).unwrap();
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(format!("{}/compounding_biotech_audit.json", export_dir))
        .unwrap();
    let mut writer = BufWriter::new(file);

    println!("=========================================================");
    println!("G^G SOVEREIGN PHYSICS ENGINE: 1000Hz BIOLOGICAL FLUIDS AUDIT");
    println!("TARGET: PHARMACEUTICAL R&D & HUMAN DIGITAL TWIN");
    println!("MODULE: COMPOUNDING_MONTE_CARLO");
    println!("EXECUTING 3,000 TRAJECTORIES...");
    println!("=========================================================\n");

    // Scenario 1: Gastric Dissolution (GI Tract)
    println!("Running Scenario 1: Gastric Dissolution (GI Tract)...");
    let digestion_results: Vec<serde_json::Value> = (0..NUM_TRAJECTORIES_PER_SCENARIO).into_par_iter().map(|i| {
        let mut rng = rand::thread_rng();
        let mut state = CompoundingState::new_stomach_state();
        
        // Randomize stomach parameters: pH range 1.0..3.5, and peristaltic shear rate frequency (agitation)
        let initial_ph = rng.gen_range(1.0..3.5);
        let peristaltic_freq_hz = rng.gen_range(0.05..0.25);
        state.ph = initial_ph;
        
        // Solubility shifts based on stomach acidity (more acidic = higher solubility Cs)
        state.solubility_limit_cs = 40.0 + (3.5 - initial_ph) * 10.0;

        let sim_duration_s = 20.0; // 20 second window
        let steps = (sim_duration_s * HZ) as usize;

        for tick in 0..steps {
            let t = tick as f64 * DT;
            // Modeled peristaltic compression shear rate
            let shear_rate = (2.0 * std::f64::consts::PI * peristaltic_freq_hz * t).sin().abs() * 15.0;
            state.step(shear_rate, 0.0, DT);

            // Exit early if fully dissolved
            if state.solid_mass_kg <= 1e-6 {
                break;
            }
        }

        let dissolved_pct = (1.0 - (state.solid_mass_kg / 0.001)) * 100.0;
        json!({
            "trajectory_id": i,
            "scenario": "gastric_dissolution",
            "initial_ph": initial_ph,
            "peristaltic_frequency_hz": peristaltic_freq_hz,
            "final_solid_mass_kg": state.solid_mass_kg,
            "dissolved_percentage": dissolved_pct,
            "completely_dissolved": state.solid_mass_kg <= 1e-6,
            "active_potency": state.active_potency,
            "cryptographic_seal": state.proof.clone().seal()
        })
    }).collect();

    // Scenario 2: Bioreactor Biologic Stirring (Fragile Protein Shear)
    println!("Running Scenario 2: Bioreactor Biologic Stirring (Fragile Protein Shear)...");
    let bioreactor_results: Vec<serde_json::Value> = (0..NUM_TRAJECTORIES_PER_SCENARIO).into_par_iter().map(|i| {
        let mut rng = rand::thread_rng();
        let mut state = CompoundingState::new_bioreactor_state();
        
        // Impeller speed (RPM / shear rate)
        let impeller_rpm = rng.gen_range(50.0..350.0);
        // Map RPM to shear rate (e.g. 50rpm = 20s^-1, 350rpm = 140s^-1)
        let base_shear_rate = impeller_rpm * 0.4;
        
        // Add mechanical turbulence noise (vessel agitation spikes)
        let turbulence_noise = rng.gen_range(1.0..2.5);

        let sim_duration_s = 30.0; // 30 second continuous stir run
        let steps = (sim_duration_s * HZ) as usize;

        for tick in 0..steps {
            let t = tick as f64 * DT;
            // Fluctuate shear rate based on turbulence
            let current_shear = base_shear_rate * (1.0 + 0.1 * (t * 5.0).cos() * turbulence_noise);
            state.step(current_shear, 0.0, DT);

            // Exit early if completely denatured
            if state.active_potency <= 1e-4 {
                break;
            }
        }

        json!({
            "trajectory_id": i + NUM_TRAJECTORIES_PER_SCENARIO,
            "scenario": "bioreactor_stirring",
            "impeller_rpm": impeller_rpm,
            "turbulence_factor": turbulence_noise,
            "accumulated_shear_pa": state.accumulated_shear_stress,
            "active_potency": state.active_potency,
            "denatured": state.active_potency <= 0.80, // >20% loss in efficacy
            "cryptographic_seal": state.proof.clone().seal()
        })
    }).collect();

    // Scenario 3: Vascular Blood Transport (Vessel Rheology)
    println!("Running Scenario 3: Vascular Blood Transport (Vessel Rheology)...");
    let vascular_results: Vec<serde_json::Value> = (0..NUM_TRAJECTORIES_PER_SCENARIO).into_par_iter().map(|i| {
        let mut rng = rand::thread_rng();
        let mut state = CompoundingState::new_blood_state();
        
        // Capillary vessel diameter (microns)
        let capillary_diameter_um = rng.gen_range(8.0..45.0);
        // Smaller vessels generate much higher local shear rates at same pressure
        let base_shear = 400.0 * (15.0 / capillary_diameter_um);
        
        let sim_duration_s = 10.0; // 10 second transport window
        let steps = (sim_duration_s * HZ) as usize;

        for _ in 0..steps {
            // Heartbeat pulsatile flow shear modulation
            let heart_rate_hz = 1.2; // 72 bpm
            let pulsatile_shear = base_shear * (1.0 + 0.3 * (2.0 * std::f64::consts::PI * heart_rate_hz * state.time_s).sin());
            
            state.step(pulsatile_shear, 0.0, DT);
        }

        json!({
            "trajectory_id": i + (2 * NUM_TRAJECTORIES_PER_SCENARIO),
            "scenario": "vascular_transport",
            "capillary_diameter_um": capillary_diameter_um,
            "average_viscosity_pas": state.viscosity, // Demonstrates shear thinning
            "accumulated_shear_pa": state.accumulated_shear_stress,
            "api_concentration": state.api_concentration,
            "active_potency": state.active_potency,
            "cryptographic_seal": state.proof.clone().seal()
        })
    }).collect();

    // Collect all results and write to file
    println!("Writing all sealed trajectories to disk...");
    let mut total_digestion_dissolved = 0;
    let mut total_bioreactor_denatured = 0;

    for res in &digestion_results {
        writeln!(writer, "{}", res.to_string()).unwrap();
        if res["completely_dissolved"].as_bool().unwrap() {
            total_digestion_dissolved += 1;
        }
    }

    for res in &bioreactor_results {
        writeln!(writer, "{}", res.to_string()).unwrap();
        if res["denatured"].as_bool().unwrap() {
            total_bioreactor_denatured += 1;
        }
    }

    for res in &vascular_results {
        writeln!(writer, "{}", res.to_string()).unwrap();
    }

    println!("\nCOMPOUNDING & BIOTECH PHYSICS AUDIT COMPLETE.");
    println!("TOTAL TRAJECTORIES EVALUATED: 3,000");
    println!("SCENARIO 1 (GI TRACT) COMPLETE DISSOLUTION RATE: {}/{} ({:.2}%)", 
             total_digestion_dissolved, 
             NUM_TRAJECTORIES_PER_SCENARIO, 
             (total_digestion_dissolved as f64 / NUM_TRAJECTORIES_PER_SCENARIO as f64) * 100.0);
    println!("SCENARIO 2 (BIOREACTOR) BIOLOGIC DENATURING RATE: {}/{} ({:.2}%)", 
             total_bioreactor_denatured, 
             NUM_TRAJECTORIES_PER_SCENARIO, 
             (total_bioreactor_denatured as f64 / NUM_TRAJECTORIES_PER_SCENARIO as f64) * 100.0);
    println!("EXECUTION TIME: {:?}", start_time.elapsed());
    println!("SEALED TO: {}/compounding_biotech_audit.json\n", export_dir);
}
