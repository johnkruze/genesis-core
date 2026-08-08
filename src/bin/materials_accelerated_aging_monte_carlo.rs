use std::time::Instant;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output;
use genesis_core::rng::Rng;
use genesis_core::physics::tribology::{TribologySurfaceState, TribologyAgingParams};
use serde::Serialize;
use std::sync::Arc;
use arrow::array::{Float64Array, StringArray, BooleanArray};
use arrow::record_batch::RecordBatch;
use arrow::datatypes::{Schema, Field, DataType};
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::file::properties::WriterProperties;
use parquet::basic::Compression;

#[derive(Debug, Serialize)]
struct AgingRunResult {
    id: u32,
    short_id: String,
    contact_pressure_mpa: f64,
    sliding_velocity_m_s: f64,
    ambient_temperature_k: f64,
    flash_temperature_k: f64,
    cumulative_galling_wear_um: f64,
    phase_crystallization_pct: f64,
    friction_coefficient_mu: f64,
    is_galling_seizure_failed: bool,
    proof_hash: String,
}

fn run_single_aging(
    id: u32,
    rng: &mut Rng,
) -> AgingRunResult {
    let short_id = output::short_id(rng);
    
    // Sweep pressure (10 to 350 MPa), velocity (0.05 to 1.5 m/s), temp (300 to 700 K)
    let pressure = rng.range(10.0, 350.0);
    let velocity = rng.range(0.05, 1.5);
    let temp_k = rng.range(300.0, 700.0);

    let params = TribologyAgingParams::default();
    let mut state = TribologySurfaceState::new(pressure, velocity, temp_k);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(pressure);
    proof.feed_f64(velocity);
    proof.feed_f64(temp_k);

    let dt_hr = 1.0; // 1-hour timesteps
    let total_steps = 100; // 100 hours accelerated testing

    for step in 0..total_steps {
        state.step(&params, dt_hr);

        if step % 25 == 0 {
            proof.feed_f64(state.cumulative_galling_wear_um);
            proof.feed_f64(state.phase_crystallization_pct);
        }
    }

    proof.feed_f64(state.cumulative_galling_wear_um);
    proof.feed_str(if state.is_galling_seizure_failed { "SEIZURE_FAILED" } else { "STABLE_SURFACE_PASSED" });

    AgingRunResult {
        id,
        short_id,
        contact_pressure_mpa: pressure,
        sliding_velocity_m_s: velocity,
        ambient_temperature_k: temp_k,
        flash_temperature_k: state.flash_temperature_k,
        cumulative_galling_wear_um: state.cumulative_galling_wear_um,
        phase_crystallization_pct: state.phase_crystallization_pct,
        friction_coefficient_mu: state.friction_coefficient_mu,
        is_galling_seizure_failed: state.is_galling_seizure_failed,
        proof_hash: proof.seal(),
    }
}

fn write_parquet_dataset(path: &str, results: &[AgingRunResult], run_proof: &str) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::Utf8, false),
        Field::new("contact_pressure_mpa", DataType::Float64, false),
        Field::new("sliding_velocity_m_s", DataType::Float64, false),
        Field::new("ambient_temperature_k", DataType::Float64, false),
        Field::new("flash_temperature_k", DataType::Float64, false),
        Field::new("cumulative_galling_wear_um", DataType::Float64, false),
        Field::new("phase_crystallization_pct", DataType::Float64, false),
        Field::new("friction_coefficient_mu", DataType::Float64, false),
        Field::new("is_galling_seizure_failed", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let traj_ids: StringArray = results.iter().map(|r| Some(format!("aging_{}", r.short_id))).collect();
    let pressures: Float64Array = results.iter().map(|r| Some(r.contact_pressure_mpa)).collect();
    let vels: Float64Array = results.iter().map(|r| Some(r.sliding_velocity_m_s)).collect();
    let ambients: Float64Array = results.iter().map(|r| Some(r.ambient_temperature_k)).collect();
    let flashes: Float64Array = results.iter().map(|r| Some(r.flash_temperature_k)).collect();
    let wears: Float64Array = results.iter().map(|r| Some(r.cumulative_galling_wear_um)).collect();
    let crysts: Float64Array = results.iter().map(|r| Some(r.phase_crystallization_pct)).collect();
    let mus: Float64Array = results.iter().map(|r| Some(r.friction_coefficient_mu)).collect();
    let seizures: BooleanArray = results.iter().map(|r| Some(r.is_galling_seizure_failed)).collect();
    let proofs: StringArray = results.iter().map(|r| Some(r.proof_hash.clone())).collect();

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(traj_ids),
            Arc::new(pressures),
            Arc::new(vels),
            Arc::new(ambients),
            Arc::new(flashes),
            Arc::new(wears),
            Arc::new(crysts),
            Arc::new(mus),
            Arc::new(seizures),
            Arc::new(proofs),
        ],
    ).expect("Failed to create RecordBatch");

    let file = std::fs::File::create(path)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.to_string()),
            parquet::file::metadata::KeyValue::new("generator".to_string(), "G^G Tribology Accelerated Aging v1.0".to_string()),
        ]))
        .build();

    let mut writer = ArrowWriter::try_new(file, schema, Some(props))
        .expect("Failed to create Parquet ArrowWriter");
    writer.write(&batch).expect("Failed to write Parquet batch");
    writer.close().expect("Failed to close Parquet writer");

    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n_trajectories: u32 = args.get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1_000);

    let out_parquet = args.iter().position(|a| a == "--parquet")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "../../doe-genesis/topic-3-materials-predictable-functionality/data/materials_accelerated_aging_tribology.parquet".to_string());

    println!("====================================================================");
    println!("  G^G KERNEL: ACCELERATED TRIBOLOGY & AGING SWEEP");
    println!("  Target Trajectories: {}", n_trajectories);
    println!("  Simulating Flash Temperatures, Galling Wear & Phase Transitions...");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x5452_4942_4F4C_4F47);
    let start = Instant::now();

    let mut results = Vec::with_capacity(n_trajectories as usize);
    for i in 0..n_trajectories {
        results.push(run_single_aging(i, &mut rng));
    }

    let proof_hashes: Vec<_> = results.iter().map(|r| r.proof_hash.clone()).collect();
    let run_proof = proof::seal_run(&proof_hashes);

    write_parquet_dataset(&out_parquet, &results, &run_proof)
        .expect("Failed to write Parquet dataset");

    let stable_runs = results.iter().filter(|r| !r.is_galling_seizure_failed).count();
    let seizure_runs = n_trajectories as usize - stable_runs;

    println!("====================================================================");
    println!("  ACCELERATED AGING SWEEP COMPLETE");
    println!("  Total Trajectories Simulated: {}", n_trajectories);
    println!("  Stable Surface Lifetime Passes:    {} ({:.1}%)", stable_runs, (stable_runs as f64 / n_trajectories as f64) * 100.0);
    println!("  Galling Seizure / Wear Failures:  {} ({:.1}%)", seizure_runs, (seizure_runs as f64 / n_trajectories as f64) * 100.0);
    println!("  Master SHA-256 Run Proof:           {}", run_proof);
    println!("  Simulation Time:                    {:?}", start.elapsed());
    println!("  Parquet Dataset Written To:          {}", out_parquet);
    println!("====================================================================\n");
}
