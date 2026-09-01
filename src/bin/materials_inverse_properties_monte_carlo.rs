use std::time::Instant;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output;
use genesis_core::rng::Rng;
use genesis_core::physics::inverse_properties::{InversePropertyState, InversePropertyParams};
use serde::Serialize;
use std::sync::Arc;
use arrow::array::{Float64Array, StringArray, BooleanArray};
use arrow::record_batch::RecordBatch;
use arrow::datatypes::{Schema, Field, DataType};
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::file::properties::WriterProperties;
use parquet::basic::Compression;

#[derive(Debug, Serialize)]
struct InversePropRunResult {
    id: u32,
    short_id: String,
    target_load_kn: f64,
    target_fatigue_cycles: f64,
    alignment_score: f64,
    achieved_mass_kg: f64,
    fatigue_endurance_limit_mpa: f64,
    von_mises_stress_mpa: f64,
    structural_mass_efficiency_index: f64,
    safety_margin: f64,
    is_multi_objective_satisfied: bool,
    proof_hash: String,
}

fn run_single_inverse_prop(
    id: u32,
    rng: &mut Rng,
) -> InversePropRunResult {
    let short_id = output::short_id(rng);
    
    // Sweep target load (10 to 200 kN), fatigue cycles (1e5 to 1e7), alignment (0.2 to 1.0)
    let target_load = rng.range(10.0, 200.0);
    let fatigue_n = rng.range(1e5, 1e7);
    let alignment = rng.range(0.2, 1.0);

    let params = InversePropertyParams::default();
    let mut state = InversePropertyState::new(target_load, fatigue_n);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(target_load);
    proof.feed_f64(fatigue_n);
    proof.feed_f64(alignment);

    state.step(&params, alignment);

    proof.feed_f64(state.achieved_mass_kg);
    proof.feed_f64(state.safety_margin);
    proof.feed_str(if state.is_multi_objective_satisfied {
        "INVERSE_SPECS_PASSED"
    } else if state.achieved_mass_kg > params.max_allowable_mass_kg
        && state.safety_margin < params.min_required_safety_margin
    {
        "MASS_AND_SAFETY_BOUND_EXCEEDED"
    } else if state.achieved_mass_kg > params.max_allowable_mass_kg {
        "MASS_BOUND_EXCEEDED"
    } else {
        "SAFETY_BOUND_EXCEEDED"
    });

    InversePropRunResult {
        id,
        short_id,
        target_load_kn: target_load,
        target_fatigue_cycles: fatigue_n,
        alignment_score: alignment,
        achieved_mass_kg: state.achieved_mass_kg,
        fatigue_endurance_limit_mpa: state.fatigue_endurance_limit_mpa,
        von_mises_stress_mpa: state.von_mises_stress_mpa,
        structural_mass_efficiency_index: state.structural_mass_efficiency_index,
        safety_margin: state.safety_margin,
        is_multi_objective_satisfied: state.is_multi_objective_satisfied,
        proof_hash: proof.seal(),
    }
}

fn write_parquet_dataset(path: &str, results: &[InversePropRunResult], run_proof: &str) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::Utf8, false),
        Field::new("target_load_kn", DataType::Float64, false),
        Field::new("target_fatigue_cycles", DataType::Float64, false),
        Field::new("alignment_score", DataType::Float64, false),
        Field::new("achieved_mass_kg", DataType::Float64, false),
        Field::new("fatigue_endurance_limit_mpa", DataType::Float64, false),
        Field::new("von_mises_stress_mpa", DataType::Float64, false),
        Field::new("structural_mass_efficiency_index", DataType::Float64, false),
        Field::new("safety_margin", DataType::Float64, false),
        Field::new("is_multi_objective_satisfied", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let traj_ids: StringArray = results.iter().map(|r| Some(format!("inv_prop_{}", r.short_id))).collect();
    let loads: Float64Array = results.iter().map(|r| Some(r.target_load_kn)).collect();
    let cycles: Float64Array = results.iter().map(|r| Some(r.target_fatigue_cycles)).collect();
    let aligns: Float64Array = results.iter().map(|r| Some(r.alignment_score)).collect();
    let masses: Float64Array = results.iter().map(|r| Some(r.achieved_mass_kg)).collect();
    let fatigues: Float64Array = results.iter().map(|r| Some(r.fatigue_endurance_limit_mpa)).collect();
    let stresses: Float64Array = results.iter().map(|r| Some(r.von_mises_stress_mpa)).collect();
    let effs: Float64Array = results.iter().map(|r| Some(r.structural_mass_efficiency_index)).collect();
    let margins: Float64Array = results.iter().map(|r| Some(r.safety_margin)).collect();
    let passes: BooleanArray = results.iter().map(|r| Some(r.is_multi_objective_satisfied)).collect();
    let proofs: StringArray = results.iter().map(|r| Some(r.proof_hash.clone())).collect();

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(traj_ids),
            Arc::new(loads),
            Arc::new(cycles),
            Arc::new(aligns),
            Arc::new(masses),
            Arc::new(fatigues),
            Arc::new(stresses),
            Arc::new(effs),
            Arc::new(margins),
            Arc::new(passes),
            Arc::new(proofs),
        ],
    ).expect("Failed to create RecordBatch");

    let file = std::fs::File::create(path)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.to_string()),
            parquet::file::metadata::KeyValue::new("generator".to_string(), "G^G Multi-Objective Inverse Property Design v1.0".to_string()),
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
        .unwrap_or(2_500);

    let out_parquet = args.iter().position(|a| a == "--parquet")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "../../doe-genesis/topic-3-materials-predictable-functionality/data/materials_inverse_properties_multi_objective.parquet".to_string());

    println!("====================================================================");
    println!("  G^G KERNEL: MULTI-OBJECTIVE INVERSE PROPERTY DESIGN SWEEP");
    println!("  Target Trajectories: {}", n_trajectories);
    println!("  Backward Engineering from F_target, N_fatigue & Mass Invariants...");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x494E_5645_5253_4550);
    let start = Instant::now();

    let mut results = Vec::with_capacity(n_trajectories as usize);
    for i in 0..n_trajectories {
        results.push(run_single_inverse_prop(i, &mut rng));
    }

    let proof_hashes: Vec<_> = results.iter().map(|r| r.proof_hash.clone()).collect();
    let run_proof = proof::seal_run(&proof_hashes);

    write_parquet_dataset(&out_parquet, &results, &run_proof)
        .expect("Failed to write Parquet dataset");

    let satisfied_runs = results.iter().filter(|r| r.is_multi_objective_satisfied).count();
    let failed_runs = n_trajectories as usize - satisfied_runs;

    println!("====================================================================");
    println!("  INVERSE PROPERTY SWEEP COMPLETE");
    println!("  Total Trajectories Simulated: {}", n_trajectories);
    println!("  Multi-Objective Inverse Passes:     {} ({:.1}%)", satisfied_runs, (satisfied_runs as f64 / n_trajectories as f64) * 100.0);
    println!("  Mass Limit Exceeded Failures:       {} ({:.1}%)", failed_runs, (failed_runs as f64 / n_trajectories as f64) * 100.0);
    println!("  Master SHA-256 Run Proof:           {}", run_proof);
    println!("  Simulation Time:                    {:?}", start.elapsed());
    println!("  Parquet Dataset Written To:          {}", out_parquet);
    println!("====================================================================\n");
}
