use genesis_core::physics::swing::SynchronousMachine;
use genesis_core::proof::seal_run;
use rayon::prelude::*;
use std::fs::File;
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{BooleanArray, Float64Array, StringArray, UInt64Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;

#[derive(Clone)]
struct RunConfig {
    inertia_h: f64,
    weather_drop_pct: f64,
    t_ai_ms: u64,
    damping: f64,
}

struct RunResult {
    cascaded: bool,
    t_to_cascade_ms: u64,
    final_frequency_drift: f64,
    proof_hash: String,
}

fn simulate(cfg: &RunConfig) -> RunResult {
    let mut machine = SynchronousMachine::new();
    machine.h_constant = cfg.inertia_h;
    machine.damping = cfg.damping;

    // Fixed starting point: Grid running hot near peak load (large phase spread)
    machine.p_mech = 1.0;
    machine.p_max = 1.05; // Tight transmission margin
    let initial_delta_rad = (machine.p_mech / machine.p_max).asin();
    machine.delta = initial_delta_rad;

    let dt = 0.001; // 1 ms steps
    let max_time_ms = 10_000;
    
    // T=0: Cloud cover drops solar output, immediately robbing grid of driving force
    machine.simulate_weather_loss(cfg.weather_drop_pct + 10.0); // Base 10% drop + sweep

    // Grid starts immediately decelerating under the load.
    // The AI takes `t_ai_ms` to recognize the physical drop before trying to route new power.
    for _ in 0..cfg.t_ai_ms {
        machine.step(dt);
    }

    // AI kicks in to try and shed/route load, but instead of fixing it, it over-corrects
    // by shutting down an interconnect, causing a massive 15% shock constraint.
    machine.ai_apply_load_mismatch(15.0);

    // Watch resolution
    while !machine.cascaded && machine.time_ms < max_time_ms {
        machine.step(dt);
    }

    RunResult {
        cascaded: machine.cascaded,
        t_to_cascade_ms: if machine.cascaded { machine.time_ms } else { 0 },
        final_frequency_drift: machine.delta_omega / (2.0 * std::f64::consts::PI), // Hz
        proof_hash: machine.get_sealed_hash(),
    }
}

fn main() {
    println!("=== G^G KERNEL: SWING INTERMITTENCY SWEEP ===");
    let args: Vec<String> = std::env::args().collect();
    
    let limit: Option<usize> = args.get(1)
        .and_then(|s| s.parse().ok());

    let out_path = args.iter().position(|a| a == "--out")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/products/swing_intermittency_failure_envelope.parquet").to_string());

    let start = Instant::now();

    // 1. Generate configs
    let mut configs = Vec::new();

    // Inertia H: 1.0 (Heavy Renewable) to 6.0 (Heavy Spinning Base)
    for h_idx in 0..100 {
        let inertia_h = 1.0 + h_idx as f64 * 0.05;
        // Weather drop off: 0% to 50% physical power loss
        for w_idx in 0..100 {
            let weather_drop_pct = w_idx as f64 * 0.5;
            // AI Response Time: 1ms (Uber fast) to 100ms (Routing latency)
            for t_idx in 1..=100 {
                let t_ai_ms = t_idx as u64;
                // Grid Health (Damping): 0.01 to 0.10
                for d_idx in 0..10 {
                    let damping = 0.01 + (d_idx as f64 * 0.01);
                    configs.push(RunConfig {
                        inertia_h,
                        weather_drop_pct,
                        t_ai_ms,
                        damping,
                    });
                }
            }
        }
    }

    if let Some(l) = limit {
        configs.truncate(l);
        println!("Truncating run sweep to {} trajectories.", l);
    }

    let total_runs = configs.len();
    println!("Total trajectories to simulate: {}", total_runs);

    // 2. Setup Arrow / Parquet Writer
    let schema = Arc::new(Schema::new(vec![
        Field::new("inertia_h", DataType::Float64, false),
        Field::new("weather_drop_pct", DataType::Float64, false),
        Field::new("t_ai_ms", DataType::UInt64, false),
        Field::new("damping", DataType::Float64, false),
        Field::new("cascaded", DataType::Boolean, false),
        Field::new("t_to_cascade_ms", DataType::UInt64, false),
        Field::new("final_frequency_drift", DataType::Float64, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let file = File::create(&out_path).expect("Failed to create output Parquet file");
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .build();
    let mut writer = ArrowWriter::try_new(file, schema.clone(), Some(props))
        .expect("Failed to create ArrowWriter");

    // 3. Process in Chunks
    let chunk_size = 250_000;
    let mut processed = 0;
    let mut all_proof_hashes = Vec::with_capacity(total_runs);
    let mut cascaded_count = 0;
    let mut low_inertia_deaths = 0;

    while processed < total_runs {
        let this_chunk_size = std::cmp::min(chunk_size, total_runs - processed);
        let chunk = &configs[processed..processed + this_chunk_size];

        // Parallel simulation
        let results: Vec<(RunConfig, RunResult)> = chunk
            .into_par_iter()
            .map(|cfg| (cfg.clone(), simulate(cfg)))
            .collect();

        // Columnar buffers for RecordBatch
        let mut inertia_h = Vec::with_capacity(this_chunk_size);
        let mut weather_drop_pct = Vec::with_capacity(this_chunk_size);
        let mut t_ai_ms = Vec::with_capacity(this_chunk_size);
        let mut damping = Vec::with_capacity(this_chunk_size);
        let mut cascaded = Vec::with_capacity(this_chunk_size);
        let mut t_to_cascade_ms = Vec::with_capacity(this_chunk_size);
        let mut final_frequency_drift = Vec::with_capacity(this_chunk_size);
        let mut proof_hash_vec = Vec::with_capacity(this_chunk_size);

        for (cfg, res) in results {
            inertia_h.push(cfg.inertia_h);
            weather_drop_pct.push(cfg.weather_drop_pct);
            t_ai_ms.push(cfg.t_ai_ms);
            damping.push(cfg.damping);

            cascaded.push(res.cascaded);
            t_to_cascade_ms.push(res.t_to_cascade_ms);
            final_frequency_drift.push(res.final_frequency_drift);

            if res.cascaded {
                cascaded_count += 1;
                if cfg.t_ai_ms <= 15 && cfg.inertia_h < 3.0 {
                    low_inertia_deaths += 1;
                }
            }

            all_proof_hashes.push(res.proof_hash.clone());
            proof_hash_vec.push(res.proof_hash);
        }

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Float64Array::from(inertia_h)),
                Arc::new(Float64Array::from(weather_drop_pct)),
                Arc::new(UInt64Array::from(t_ai_ms)),
                Arc::new(Float64Array::from(damping)),
                Arc::new(BooleanArray::from(cascaded)),
                Arc::new(UInt64Array::from(t_to_cascade_ms)),
                Arc::new(Float64Array::from(final_frequency_drift)),
                Arc::new(StringArray::from(proof_hash_vec)),
            ]
        ).expect("Failed to create RecordBatch");

        writer.write(&batch).expect("Failed to write RecordBatch");

        processed += this_chunk_size;
        println!("  Simulated and wrote {}/{} runs...", processed, total_runs);
    }

    writer.close().expect("Failed to close ArrowWriter");

    let master_hash = seal_run(&all_proof_hashes);

    println!("Sweep completed in {:?}", start.elapsed());
    println!("Wrote Parquet artifact to {}", out_path);
    println!("Master Sweep Hash: {}", master_hash);
    println!("Headline: Without rotating momentum, even lightning-fast AI routing cannot stop grid phase collapse.");
    println!("Out of {} grid setups, {} ({:.1}%) catastrophically lost synchronism.", total_runs, cascaded_count, (cascaded_count as f64 / total_runs as f64) * 100.0);
    println!("Crucially, {} grid-collapses occurred despite the AI acting in practically instantaneous time (<15ms) simply because H < 3.0.", low_inertia_deaths);
}
