use genesis_core::physics::atheric::AthericSystem;
use genesis_core::proof::seal_run;
use genesis_core::rng::Rng;
use rayon::prelude::*;
use serde::Serialize;
use std::fs::File;
use std::io::Write;
use std::time::Instant;

#[derive(Serialize)]
struct AthericRun {
    clock_drift_ns: f64,
    jamming_intensity: f64,
    distance_km: f64,
    n_channels: usize,
    capacity_mbps: f64,
    swarm_collapsed: bool,
}

#[derive(Clone)]
struct RunConfig {
    clock_drift_ns: f64,
    jamming_intensity: f64,
    distance_km: f64,
    n_channels: usize,
    seed: u64,
}

fn simulate(cfg: &RunConfig) -> (AthericRun, String) {
    let mut rng = Rng::new(cfg.seed);
    
    // Base RF settings for an autonomous swarm node
    let tx_power = 20.0; // 20 Watts
    let noise_floor_dbm = -100.0;
    
    let mut sys = AthericSystem::new(cfg.n_channels, tx_power, noise_floor_dbm, cfg.distance_km);
    sys.bandwidth = 1_000_000.0; // 1 MHz per channel
    
    // Apply EW broadband jamming (noise floor multiplier)
    sys.apply_broadband(cfg.jamming_intensity);
    
    // Constraint: 1000ns (1us) clock drift shatters the timing window of the hopping sequence
    if cfg.clock_drift_ns > 1000.0 {
        sys.apply_clock_drift();
    }
    
    let mut capacity = sys.total_capacity();
    
    if sys.desync {
        // Multi-channel coherence collapses. Receiver must guess the channel, effectively
        // reducing total throughput down to 1/N of the un-jammed spectrum.
        capacity *= 1.0 / (cfg.n_channels.max(1) as f64);
    }
    
    let capacity_mbps = capacity / 1_000_000.0;
    
    // Minimum 50 mbps required for autonomous swarm sensor fusion
    let swarm_collapsed = capacity_mbps < 50.0;

    let result = AthericRun {
        clock_drift_ns: cfg.clock_drift_ns,
        jamming_intensity: cfg.jamming_intensity,
        distance_km: cfg.distance_km,
        n_channels: cfg.n_channels,
        capacity_mbps,
        swarm_collapsed,
    };
    
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(result.capacity_mbps.to_le_bytes());
    (result, hex::encode(hasher.finalize()))
}

fn main() {
    println!("=== G^G KERNEL: ATHERIC MONTE CARLO SWEEP ===");
    let start = Instant::now();

    let mut configs = Vec::new();

    let mut seed = 0;
    // Clock drift: 0 to 5000 ns (0 to 5 us)
    for c_idx in 0..50 {
        let clock_drift_ns = c_idx as f64 * 100.0;
        // Jamming: completely clear to severe broadband
        for j_idx in 0..25 {
            let jamming_intensity = j_idx as f64 * 0.1;
            // Distance: 1km to 50km
            for d_idx in 1..=20 {
                let distance_km = d_idx as f64 * 2.5;
                // N-channels: 1, 8, 16, 64, 128
                for &n_channels in &[1, 8, 16, 64, 128] {
                    configs.push(RunConfig {
                        clock_drift_ns,
                        jamming_intensity,
                        distance_km,
                        n_channels,
                        seed,
                    });
                    seed += 1;
                }
            }
        }
    }

    let total_runs = configs.len();
    println!("Total trajectories to simulate: {}", total_runs);

    let results_and_hashes: Vec<(AthericRun, String)> = configs
        .into_par_iter()
        .map(|cfg| simulate(&cfg))
        .collect();

    let mut runs = Vec::with_capacity(total_runs);
    let mut hashes = Vec::with_capacity(total_runs);
    let mut collapsed_count = 0;

    for (run, hash) in results_and_hashes {
        if run.swarm_collapsed { collapsed_count += 1; }
        runs.push(run);
        hashes.push(hash);
    }

    let master_hash = seal_run(&hashes);
    let json_data = serde_json::to_string_pretty(&runs).unwrap();
    let mut file = File::create("atheric_failure_envelope.json").unwrap();
    file.write_all(json_data.as_bytes()).unwrap();

    println!("Sweep completed in {:?}", start.elapsed());
    println!("Wrote json artifact to atheric_failure_envelope.json");
    println!("Master Sweep Hash: {}", master_hash);
    println!("Headline: Mere microseconds of clock drift completely shatters multi-channel hopping resilience.");
    println!("Across {} EW network conditions, {} ({:.1}%) forced total swarm link collapse.", total_runs, collapsed_count, (collapsed_count as f64 / total_runs as f64) * 100.0);
}
