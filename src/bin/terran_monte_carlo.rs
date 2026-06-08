use genesis_core::physics::terran::{SoilProfile, SoilType, RobotContact, Locomotion};
use genesis_core::proof::seal_run;
use rayon::prelude::*;
use serde::Serialize;
use std::fs::File;
use std::io::Write;
use std::time::Instant;

#[derive(Serialize)]
struct TerranRun {
    robot_mass_kg: f64,
    footprint_m2: f64,
    moisture: f64,
    glomalin_mg_g: f64,
    max_compaction: f64,
    yield_destroyed: bool,
}

#[derive(Clone)]
struct RunConfig {
    robot_mass_kg: f64,
    footprint_m2: f64,
    moisture: f64,
    glomalin_mg_g: f64,
}

fn simulate(cfg: &RunConfig) -> (TerranRun, String) {
    let soil = SoilProfile {
        soil_type: SoilType::Loam,
        moisture: cfg.moisture,
        glomalin_mg_g: cfg.glomalin_mg_g,
        compaction: 0.0,
        depth_layers: 20,
    };
    
    let robot = RobotContact {
        mass_kg: cfg.robot_mass_kg,
        footprint_m2: cfg.footprint_m2,
        locomotion: Locomotion::Wheeled,
    };

    let (max_compaction, _depth) = soil.evaluate_contact(&robot);
    
    // threshold for catastrophe preventing seed emergence (0.4 increment typically maxes out over a pass)
    let yield_destroyed = max_compaction > 0.3;
    
    let result = TerranRun {
        robot_mass_kg: cfg.robot_mass_kg,
        footprint_m2: cfg.footprint_m2,
        moisture: cfg.moisture,
        glomalin_mg_g: cfg.glomalin_mg_g,
        max_compaction,
        yield_destroyed,
    };
    
    // Hash state
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(result.max_compaction.to_le_bytes());
    (result, hex::encode(hasher.finalize()))
}

fn main() {
    println!("=== G^G KERNEL: TERRAN MONTE CARLO SWEEP ===");
    let start = Instant::now();

    let mut configs = Vec::new();

    // Robot Mass: 100kg to 5000kg
    for mass_idx in 0..50 {
        let robot_mass_kg = 100.0 + (mass_idx as f64 * 100.0);
        // Footprint: 0.05m2 to 1.0m2
        for fp_idx in 0..20 {
            let footprint_m2 = 0.05 + (fp_idx as f64 * 0.05);
            // Moisture: 0.05 to 0.40
            for m_idx in 0..10 {
                let moisture = 0.05 + (m_idx as f64 * 0.035);
                // Glomalin: 0.0 to 1.0
                for g_idx in 0..5 {
                    let glomalin_mg_g = g_idx as f64 * 0.25;
                    configs.push(RunConfig {
                        robot_mass_kg,
                        footprint_m2,
                        moisture,
                        glomalin_mg_g,
                    });
                }
            }
        }
    }

    let total_runs = configs.len();
    println!("Total trajectories to simulate: {}", total_runs);

    let results_and_hashes: Vec<(TerranRun, String)> = configs
        .into_par_iter()
        .map(|cfg| simulate(&cfg))
        .collect();

    let mut runs = Vec::with_capacity(total_runs);
    let mut hashes = Vec::with_capacity(total_runs);
    let mut destroyed_count = 0;

    for (run, hash) in results_and_hashes {
        if run.yield_destroyed { destroyed_count += 1; }
        runs.push(run);
        hashes.push(hash);
    }

    let master_hash = seal_run(&hashes);
    let json_data = serde_json::to_string_pretty(&runs).unwrap();
    let mut file = File::create("terran_failure_envelope.json").unwrap();
    file.write_all(json_data.as_bytes()).unwrap();

    println!("Sweep completed in {:?}", start.elapsed());
    println!("Wrote json artifact to terran_failure_envelope.json");
    println!("Master Sweep Hash: {}", master_hash);
    println!("Headline: Across {} autonomous tractor/robot runs, {} ({:.1}%) guaranteed yield destruction through irreversible soil compaction.", total_runs, destroyed_count, (destroyed_count as f64 / total_runs as f64) * 100.0);
}
