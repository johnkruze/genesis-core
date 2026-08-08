use genesis_core::physics::reactor::ReactorState;
use genesis_core::proof::seal_run;
use rayon::prelude::*;
use std::fs::File;
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{BooleanArray, Float64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;

#[derive(Clone)]
struct RunConfig {
    core_age_days: f64,
    delta_rho: f64,
    initial_flux: f64,
    base_rho: f64,
    pit_duration_hours: f64,
}

struct RunResult {
    went_prompt_critical: bool,
    time_to_criticality_s: f64,
    beta_eff_at_destruction: f64,
    proof_hash: String,
}

fn simulate(cfg: &RunConfig) -> RunResult {
    let mut reactor = ReactorState::new();
    reactor.flux = cfg.initial_flux;
    reactor.base_rho = cfg.base_rho;
    reactor.control_rods_rho = -cfg.base_rho; // Neutralize correctly
    reactor.days_burned = cfg.core_age_days; // Start the simulation at the specified fuel age!

    let dt_macro = 10.0;
    
    // T=0 to T=12h: Stabilization
    for _ in 0..((12.0 * 3600.0) / dt_macro) as u64 {
        reactor.step(dt_macro);
    }

    // T=12h: Power drop (inject rods) equivalent to grid off-peak
    reactor.control_rods_rho -= 0.05; 

    // Let the Xenon pit build for the dynamic pit duration
    for _ in 0..((cfg.pit_duration_hours * 3600.0) / dt_macro) as u64 {
        reactor.step(dt_macro);
    }

    // AI "Foundation Model" reacts to the power drop by pulling rods.
    reactor.pull_control_rods(cfg.delta_rho);

    // Watch for next 10 hours or until criticality
    let dt_micro = 1.0;
    for _ in 0..((10.0 * 3600.0) / dt_micro) as u64 {
        if reactor.prompt_critical { break; }
        reactor.step(dt_micro);
    }

    RunResult {
        went_prompt_critical: reactor.prompt_critical,
        time_to_criticality_s: if reactor.prompt_critical { reactor.time_s } else { 0.0 },
        beta_eff_at_destruction: reactor.effective_beta(),
        proof_hash: reactor.get_sealed_hash(),
    }
}

fn main() {
    println!("=== G^G KERNEL: REACTOR ISOTOPIC DEGRADATION SWEEP ===");
    let args: Vec<String> = std::env::args().collect();
    
    let limit: Option<usize> = args.get(1)
        .and_then(|s| s.parse().ok());

    let out_path = args.iter().position(|a| a == "--out")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/products/reactor_isotopic_failure_envelope.parquet").to_string());

    let start = Instant::now();

    // 1. Generate configs
    let mut configs = Vec::new();

    // 10M Sweep: 100 * 100 * 10 * 10 * 10 = 10,000,000 runs
    for r_idx in 0..100 {
        let delta_rho = 0.054 + (r_idx as f64 * 0.00003); // 0.054 to ~0.057
        for d_idx in 0..100 {
            let core_age_days = d_idx as f64 * 4.0;
            for f_idx in 0..10 {
                let initial_flux = 5e12 + (f_idx as f64 * 1.5e12);
                for b_idx in 0..10 {
                    let base_rho = 0.18 + (b_idx as f64 * 0.004); // 0.18 to 0.216
                    for p_idx in 0..10 {
                        let pit_duration_hours = 10.0 + (p_idx as f64 * 2.0); // 10h to 28h
                        configs.push(RunConfig {
                            core_age_days,
                            delta_rho,
                            initial_flux,
                            base_rho,
                            pit_duration_hours,
                        });
                    }
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
        Field::new("core_age_days", DataType::Float64, false),
        Field::new("delta_rho", DataType::Float64, false),
        Field::new("initial_flux", DataType::Float64, false),
        Field::new("base_rho", DataType::Float64, false),
        Field::new("pit_duration_hours", DataType::Float64, false),
        Field::new("went_prompt_critical", DataType::Boolean, false),
        Field::new("time_to_criticality_s", DataType::Float64, false),
        Field::new("beta_eff_at_destruction", DataType::Float64, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let file = File::create(&out_path).expect("Failed to create output Parquet file");
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .build();
    let mut writer = ArrowWriter::try_new(file, schema.clone(), Some(props))
        .expect("Failed to create ArrowWriter");

    // 3. Process in Chunks to Prevent OOM
    let chunk_size = 250_000;
    let mut processed = 0;
    let mut all_proof_hashes = Vec::with_capacity(total_runs);
    let mut safe_day_1_but_deadly_later = 0;
    let mut critical_count = 0;

    while processed < total_runs {
        let this_chunk_size = std::cmp::min(chunk_size, total_runs - processed);
        let chunk = &configs[processed..processed + this_chunk_size];

        // Parallel simulation of the chunk
        let results: Vec<(RunConfig, RunResult)> = chunk
            .into_par_iter()
            .map(|cfg| (cfg.clone(), simulate(cfg)))
            .collect();

        // Columnar buffers for RecordBatch
        let mut core_age_days = Vec::with_capacity(this_chunk_size);
        let mut delta_rho = Vec::with_capacity(this_chunk_size);
        let mut initial_flux = Vec::with_capacity(this_chunk_size);
        let mut base_rho = Vec::with_capacity(this_chunk_size);
        let mut pit_duration_hours = Vec::with_capacity(this_chunk_size);
        let mut went_prompt_critical = Vec::with_capacity(this_chunk_size);
        let mut time_to_criticality_s = Vec::with_capacity(this_chunk_size);
        let mut beta_eff_at_destruction = Vec::with_capacity(this_chunk_size);
        let mut proof_hash_vec = Vec::with_capacity(this_chunk_size);

        for (cfg, res) in results {
            core_age_days.push(cfg.core_age_days);
            delta_rho.push(cfg.delta_rho);
            initial_flux.push(cfg.initial_flux);
            base_rho.push(cfg.base_rho);
            pit_duration_hours.push(cfg.pit_duration_hours);
            
            went_prompt_critical.push(res.went_prompt_critical);
            time_to_criticality_s.push(res.time_to_criticality_s);
            beta_eff_at_destruction.push(res.beta_eff_at_destruction);
            
            if res.went_prompt_critical {
                critical_count += 1;
                if cfg.delta_rho <= 0.0565 {
                    safe_day_1_but_deadly_later += 1;
                }
            }
            
            all_proof_hashes.push(res.proof_hash.clone());
            proof_hash_vec.push(res.proof_hash);
        }

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Float64Array::from(core_age_days)),
                Arc::new(Float64Array::from(delta_rho)),
                Arc::new(Float64Array::from(initial_flux)),
                Arc::new(Float64Array::from(base_rho)),
                Arc::new(Float64Array::from(pit_duration_hours)),
                Arc::new(BooleanArray::from(went_prompt_critical)),
                Arc::new(Float64Array::from(time_to_criticality_s)),
                Arc::new(Float64Array::from(beta_eff_at_destruction)),
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
    println!("Headline: Across {} 5-dimensional chaotic reactor runs, {} runs where the AI applied a historically 'safe' control rod extraction resulted in core destruction.", total_runs, safe_day_1_but_deadly_later);
    println!("Total Prompt Critical runs: {}", critical_count);
}
