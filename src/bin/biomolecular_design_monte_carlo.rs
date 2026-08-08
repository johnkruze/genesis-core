use std::time::Instant;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output::{self, DatasetMetadata, TrajectoryRecord, Dataset};
use genesis_core::rng::Rng;
use genesis_core::physics::molecular::{BiomoleculeState, ForceFieldParams};
use serde::Serialize;
use std::sync::Arc;
use arrow::array::{Float32Array, Float64Array, StringArray, BooleanArray};
use arrow::record_batch::RecordBatch;
use arrow::datatypes::{Schema, Field, DataType};
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::file::properties::WriterProperties;
use parquet::basic::Compression;

#[derive(Debug, Serialize)]
struct BiomolecularRunResult {
    id: u32,
    short_id: String,
    temperature_k: f64,
    ph_level: f64,
    shear_rate_s1: f64,
    final_potential_energy_kj: f64,
    binding_affinity_score: f64,
    thermal_stress_residual: f64,
    is_denatured: bool,
    proof_hash: String,
    telemetry: Vec<serde_json::Value>,
}

fn run_single_biomolecules(
    id: u32,
    rng: &mut Rng,
    record_telemetry: bool,
) -> BiomolecularRunResult {
    let short_id = output::short_id(rng);
    
    // Sweep extreme temperature (300K - 450K) and pH (1.5 - 12.5) for industrial catalysis
    let temp_k = rng.range(300.0, 450.0);
    let ph = rng.range(1.5, 12.5);
    let shear = rng.range(10.0, 1000.0);

    let params = ForceFieldParams {
        temperature_k: temp_k,
        ph_level: ph,
        shear_rate_s1: shear,
        ..Default::default()
    };

    let mut state = BiomoleculeState::new(30, &params); // 30 residue catalytic pocket
    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(temp_k);
    proof.feed_f64(ph);
    proof.feed_f64(shear);

    let mut telemetry = Vec::new();
    let dt_ps = 0.05; // 0.05 picoseconds timestep
    let total_steps = 200;

    for step in 0..total_steps {
        state.step(&params, dt_ps);

        if step % 20 == 0 {
            proof.feed_f64(state.total_potential_energy_kj);
            proof.feed_f64(state.binding_affinity_score);

            if record_telemetry {
                telemetry.push(serde_json::json!({
                    "t_ps": step as f64 * dt_ps,
                    "pos": [state.atoms[0].pos.x, state.atoms[0].pos.y, state.atoms[0].pos.z],
                    "vel": [state.atoms[0].vel.x, state.atoms[0].vel.y, state.atoms[0].vel.z],
                    "force": state.total_potential_energy_kj,
                    "residual": state.binding_affinity_score,
                    "is_denatured": state.is_denatured,
                }));
            }
        }
    }

    proof.feed_f64(state.total_potential_energy_kj);
    proof.feed_str(if state.is_denatured { "DENATURED" } else { "CATALYTIC_STABLE" });

    BiomolecularRunResult {
        id,
        short_id,
        temperature_k: temp_k,
        ph_level: ph,
        shear_rate_s1: shear,
        final_potential_energy_kj: state.total_potential_energy_kj,
        binding_affinity_score: state.binding_affinity_score,
        thermal_stress_residual: state.thermal_stress_residual,
        is_denatured: state.is_denatured,
        proof_hash: proof.seal(),
        telemetry,
    }
}

fn write_parquet_dataset(path: &str, results: &[BiomolecularRunResult], run_proof: &str) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::Utf8, false),
        Field::new("temperature_k", DataType::Float64, false),
        Field::new("ph_level", DataType::Float64, false),
        Field::new("shear_rate_s1", DataType::Float64, false),
        Field::new("final_potential_energy_kj", DataType::Float64, false),
        Field::new("binding_affinity_score", DataType::Float64, false),
        Field::new("thermal_stress_residual", DataType::Float64, false),
        Field::new("is_denatured", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let traj_ids: StringArray = results.iter().map(|r| Some(format!("biomolecule_{}", r.short_id))).collect();
    let temps: Float64Array = results.iter().map(|r| Some(r.temperature_k)).collect();
    let phs: Float64Array = results.iter().map(|r| Some(r.ph_level)).collect();
    let shears: Float64Array = results.iter().map(|r| Some(r.shear_rate_s1)).collect();
    let energies: Float64Array = results.iter().map(|r| Some(r.final_potential_energy_kj)).collect();
    let affinities: Float64Array = results.iter().map(|r| Some(r.binding_affinity_score)).collect();
    let resids: Float64Array = results.iter().map(|r| Some(r.thermal_stress_residual)).collect();
    let denatured: BooleanArray = results.iter().map(|r| Some(r.is_denatured)).collect();
    let proofs: StringArray = results.iter().map(|r| Some(r.proof_hash.clone())).collect();

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(traj_ids),
            Arc::new(temps),
            Arc::new(phs),
            Arc::new(shears),
            Arc::new(energies),
            Arc::new(affinities),
            Arc::new(resids),
            Arc::new(denatured),
            Arc::new(proofs),
        ],
    ).expect("Failed to create RecordBatch");

    let file = std::fs::File::create(path)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.to_string()),
            parquet::file::metadata::KeyValue::new("generator".to_string(), "G^G Biomolecular Inverse Physics v1.0".to_string()),
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
        .unwrap_or_else(|| "../../doe-genesis/topic-1-biotechnology-revolution/data/biomolecular_design_inverse_catalysis.parquet".to_string());

    println!("====================================================================");
    println!("  G^G KERNEL: INVERSE BIOMOLECULAR FORCE-FIELD SWEEP");
    println!("  Target Trajectories: {}", n_trajectories);
    println!("  Simulating 3D Force Fields, LJ/Coulomb & Thermal Hydrodynamics...");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x4210_B100_EC10_9988);
    let start = Instant::now();

    let mut results = Vec::with_capacity(n_trajectories as usize);
    for i in 0..n_trajectories {
        results.push(run_single_biomolecules(i, &mut rng, true));
    }

    let proof_hashes: Vec<_> = results.iter().map(|r| r.proof_hash.clone()).collect();
    let run_proof = proof::seal_run(&proof_hashes);

    write_parquet_dataset(&out_parquet, &results, &run_proof)
        .expect("Failed to write Parquet dataset");

    let denatured_count = results.iter().filter(|r| r.is_denatured).count();
    let stable_count = n_trajectories as usize - denatured_count;

    println!("====================================================================");
    println!("  BIOMOLECULAR SWEEP COMPLETE");
    println!("  Total Trajectories Simulated: {}", n_trajectories);
    println!("  Catalytically Stable Enzyme Designs: {} ({:.1}%)", stable_count, (stable_count as f64 / n_trajectories as f64) * 100.0);
    println!("  Thermal Denaturation Events:        {} ({:.1}%)", denatured_count, (denatured_count as f64 / n_trajectories as f64) * 100.0);
    println!("  Master SHA-256 Run Proof:           {}", run_proof);
    println!("  Simulation Time:                    {:?}", start.elapsed());
    println!("  Parquet Dataset Written To:          {}", out_parquet);
    println!("====================================================================\n");
}
