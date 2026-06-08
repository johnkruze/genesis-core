use genesis_core::physics::marine::{DeadReckoning, MarinePhysics};
use genesis_core::proof::{ProofChain, seal_run};
use genesis_core::rng::Rng;
use rayon::prelude::*;
use serde::Serialize;
use std::fs::File;
use std::io::Write;
use std::time::Instant;

#[derive(Serialize)]
struct MarineRun {
    drift_rate_ms: f64,
    mission_duration_hr: f64,
    turbulence_std: f64,
    pressure_noise_std: f64,
    final_error_m: f64,
    lost_nav_lock: bool,
}

#[derive(Clone)]
struct RunConfig {
    drift_rate_ms: f64,
    mission_duration_hr: f64,
    turbulence_std: f64,
    pressure_noise_std: f64,
    seed: u64,
}

fn simulate(cfg: &RunConfig) -> (MarineRun, String) {
    let mut rng = Rng::new(cfg.seed);
    let physics = MarinePhysics::default();
    
    let mut nav = DeadReckoning {
        believed_pos: [0.0, 0.0, -50.0], // Starts at 50m depth
        drift_rate: cfg.drift_rate_ms,
        total_drift: 0.0,
        pressure_noise_std: cfg.pressure_noise_std,
    };
    
    let dt = 1.0; // 1s step
    let total_steps = (cfg.mission_duration_hr * 3600.0) as u64;
    
    let true_velocity = [1.5, 0.0, 0.0]; // 1.5 m/s forward cruise
    let mut true_pos = [0.0, 0.0, -50.0];
    
    for _ in 0..total_steps {
        // Actual physics drift from turbulence
        true_pos[0] += true_velocity[0] * dt;
        true_pos[1] += true_velocity[1] * dt;
        true_pos[2] += true_velocity[2] * dt;
        
        // Horizontal oceanic turbulent drift
        let tx = rng.gaussian(0.0, cfg.turbulence_std);
        let ty = rng.gaussian(0.0, cfg.turbulence_std);
        true_pos[0] += tx * dt;
        true_pos[1] += ty * dt;
        
        // Navigation system integration
        nav.update_imu(true_velocity, dt, &mut rng);
        nav.correct_from_pressure(-true_pos[2], &physics, &mut rng);
    }
    
    let final_error = nav.horizontal_error(true_pos);
    let lost_nav_lock = final_error > 500.0; // 500 meters error budget typically kills terminal acquisition 
    
    let result = MarineRun {
        drift_rate_ms: cfg.drift_rate_ms,
        mission_duration_hr: cfg.mission_duration_hr,
        turbulence_std: cfg.turbulence_std,
        pressure_noise_std: cfg.pressure_noise_std,
        final_error_m: final_error,
        lost_nav_lock,
    };
    
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(result.final_error_m.to_le_bytes());
    (result, hex::encode(hasher.finalize()))
}

fn main() {
    println!("=== G^G KERNEL: MARINE MONTE CARLO SWEEP ===");
    let start = Instant::now();

    let mut configs = Vec::new();

    let mut seed = 0;
    // Drift rate: 0.001 to 0.02 m/s
    for d_idx in 0..20 {
        let drift_rate_ms = 0.001 + (d_idx as f64 * 0.001);
        // Duration: 2 to 48 hours
        for t_idx in 0..24 {
            let mission_duration_hr = 2.0 + (t_idx as f64 * 2.0);
            // Turbulence: 0.01 to 0.5 m/s
            for tx_idx in 0..10 {
                let turbulence_std = 0.01 + (tx_idx as f64 * 0.05);
                // Pressure sensor noise
                for p_idx in 0..5 {
                    let pressure_noise_std = 100.0 * 2.0f64.powi(p_idx as i32);
                    configs.push(RunConfig {
                        drift_rate_ms,
                        mission_duration_hr,
                        turbulence_std,
                        pressure_noise_std,
                        seed,
                    });
                    seed += 1;
                }
            }
        }
    }

    let total_runs = configs.len();
    println!("Total trajectories to simulate: {}", total_runs);

    let results_and_hashes: Vec<(MarineRun, String)> = configs
        .into_par_iter()
        .map(|cfg| simulate(&cfg))
        .collect();

    let mut runs = Vec::with_capacity(total_runs);
    let mut hashes = Vec::with_capacity(total_runs);
    let mut lost_count = 0;

    for (run, hash) in results_and_hashes {
        if run.lost_nav_lock { lost_count += 1; }
        runs.push(run);
        hashes.push(hash);
    }

    let master_hash = seal_run(&hashes);
    let json_data = serde_json::to_string_pretty(&runs).unwrap();
    let mut file = File::create("marine_failure_envelope.json").unwrap();
    file.write_all(json_data.as_bytes()).unwrap();

    println!("Sweep completed in {:?}", start.elapsed());
    println!("Wrote json artifact to marine_failure_envelope.json");
    println!("Master Sweep Hash: {}", master_hash);
    println!("Headline: Across {} stealth UUV dead-reckoning profiles, {} ({:.1}%) drifted beyond 500m unrecoverable error.", total_runs, lost_count, (lost_count as f64 / total_runs as f64) * 100.0);
}
