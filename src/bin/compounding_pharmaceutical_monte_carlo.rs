use std::sync::Arc;
use std::time::Instant;
use arrow::array::{Float64Array, StringArray, BooleanArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use serde::Serialize;

use genesis_core::proof::{self, ProofChain};
use genesis_core::output;
use genesis_core::rng::Rng;
use genesis_core::physics::compounding::CompoundingState;

#[derive(Debug, Serialize)]
struct CompoundingRunResult {
    id: u32,
    short_id: String,
    initial_solid_mass_kg: f64,
    flow_consistency_k: f64,
    flow_behavior_n: f64,
    shear_rate_s1: f64,
    ph_level: f64,
    final_viscosity_pas: f64,
    final_api_concentration_kg_m3: f64,
    dissolution_pct: f64,
    accumulated_shear_stress_pa: f64,
    active_potency_pct: f64,
    is_potency_collapsed: bool,
    is_dissolution_stalled: bool,
    proof_hash: String,
}

fn run_single_compounding(
    id: u32,
    rng: &mut Rng,
) -> CompoundingRunResult {
    let short_id = output::short_id(rng);
    
    // Sweep API initial mass (0.01 to 0.50 kg), K (0.005 to 0.08 Pa*s^n), n (0.45 to 0.95), shear rate (10 to 800 s^-1)
    let initial_mass = rng.range(0.01, 0.50);
    let k_index = rng.range(0.005, 0.08);
    let n_index = rng.range(0.45, 0.95);
    let shear = rng.range(10.0, 800.0);
    let ph = rng.range(1.5, 7.8);
    let state_type = rng.range(0.0, 3.0) as usize;

    let mut state = match state_type {
        0 => CompoundingState::new_stomach_state(),
        1 => CompoundingState::new_blood_state(),
        _ => CompoundingState::new_bioreactor_state(),
    };

    state.solid_mass_kg = initial_mass;
    // Specific surface sweep — coarse pellets vs fine granules (dual-regime dissolution)
    let specific_area = rng.range(0.4, 5.5);
    state.solid_surface_area_m2 = (initial_mass * specific_area).clamp(0.02, 2.5);
    state.flow_consistency_index_k = k_index;
    state.flow_behavior_index_n = n_index;
    state.ph = ph;
    // Solubility / diffusion / boundary layer sweep
    state.solubility_limit_cs = rng.range(25.0, 85.0);
    state.diffusion_coefficient = rng.range(1.5e-10, 1.8e-9);
    state.boundary_layer_h = rng.range(8.0e-6, 4.0e-5);
    // Potency dual-regime: not all runs use fragile bioreactor critical shear
    state.critical_shear_limit = match state_type {
        2 => rng.range(12.0, 40.0),   // fragile protein broth
        1 => rng.range(120.0, 450.0), // blood-scale more robust
        _ => rng.range(60.0, 280.0),  // gastric slurry intermediate
    };

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(initial_mass);
    proof.feed_f64(k_index);
    proof.feed_f64(n_index);
    proof.feed_f64(shear);
    proof.feed_f64(ph);
    proof.feed_f64(state.solubility_limit_cs);
    proof.feed_f64(state.diffusion_coefficient);
    proof.feed_f64(state.critical_shear_limit);

    // 350 s horizon — incomplete vs complete dissolution both reachable
    let steps = 350;
    let dt = 1.0;
    for _ in 0..steps {
        state.step(shear, 0.0, dt);
    }

    let final_solid = state.solid_mass_kg;
    let dissolution_pct = ((initial_mass - final_solid) / initial_mass * 100.0).clamp(0.0, 100.0);
    let potency_pct = (state.active_potency * 100.0).clamp(0.0, 100.0);

    let is_potency_collapsed = potency_pct < 80.0;
    // Process gate: <70% dissolved in horizon = stalled
    let is_dissolution_stalled = dissolution_pct < 70.0;

    proof.feed_f64(state.viscosity);
    proof.feed_f64(state.api_concentration);
    proof.feed_f64(dissolution_pct);
    proof.feed_f64(potency_pct);

    CompoundingRunResult {
        id,
        short_id,
        initial_solid_mass_kg: initial_mass,
        flow_consistency_k: k_index,
        flow_behavior_n: n_index,
        shear_rate_s1: shear,
        ph_level: ph,
        final_viscosity_pas: state.viscosity,
        final_api_concentration_kg_m3: state.api_concentration,
        dissolution_pct,
        accumulated_shear_stress_pa: state.accumulated_shear_stress,
        active_potency_pct: potency_pct,
        is_potency_collapsed,
        is_dissolution_stalled,
        proof_hash: proof.seal(),
    }
}

fn write_parquet_dataset(path: &str, results: &[CompoundingRunResult], run_proof: &str) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::Utf8, false),
        Field::new("initial_solid_mass_kg", DataType::Float64, false),
        Field::new("flow_consistency_k", DataType::Float64, false),
        Field::new("flow_behavior_n", DataType::Float64, false),
        Field::new("shear_rate_s1", DataType::Float64, false),
        Field::new("ph_level", DataType::Float64, false),
        Field::new("final_viscosity_pas", DataType::Float64, false),
        Field::new("final_api_concentration_kg_m3", DataType::Float64, false),
        Field::new("dissolution_pct", DataType::Float64, false),
        Field::new("accumulated_shear_stress_pa", DataType::Float64, false),
        Field::new("active_potency_pct", DataType::Float64, false),
        Field::new("is_potency_collapsed", DataType::Boolean, false),
        Field::new("is_dissolution_stalled", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let traj_ids: StringArray = results.iter().map(|r| Some(format!("compounding_{}", r.short_id))).collect();
    let m_inits: Float64Array = results.iter().map(|r| Some(r.initial_solid_mass_kg)).collect();
    let ks: Float64Array = results.iter().map(|r| Some(r.flow_consistency_k)).collect();
    let ns: Float64Array = results.iter().map(|r| Some(r.flow_behavior_n)).collect();
    let shears: Float64Array = results.iter().map(|r| Some(r.shear_rate_s1)).collect();
    let phs: Float64Array = results.iter().map(|r| Some(r.ph_level)).collect();
    let viscosities: Float64Array = results.iter().map(|r| Some(r.final_viscosity_pas)).collect();
    let concs: Float64Array = results.iter().map(|r| Some(r.final_api_concentration_kg_m3)).collect();
    let dissolutions: Float64Array = results.iter().map(|r| Some(r.dissolution_pct)).collect();
    let accum_shears: Float64Array = results.iter().map(|r| Some(r.accumulated_shear_stress_pa)).collect();
    let potencies: Float64Array = results.iter().map(|r| Some(r.active_potency_pct)).collect();
    let potency_fails: BooleanArray = results.iter().map(|r| Some(r.is_potency_collapsed)).collect();
    let dissolution_fails: BooleanArray = results.iter().map(|r| Some(r.is_dissolution_stalled)).collect();
    let proofs: StringArray = results.iter().map(|r| Some(r.proof_hash.clone())).collect();

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(traj_ids),
            Arc::new(m_inits),
            Arc::new(ks),
            Arc::new(ns),
            Arc::new(shears),
            Arc::new(phs),
            Arc::new(viscosities),
            Arc::new(concs),
            Arc::new(dissolutions),
            Arc::new(accum_shears),
            Arc::new(potencies),
            Arc::new(potency_fails),
            Arc::new(dissolution_fails),
            Arc::new(proofs),
        ],
    ).expect("Failed to create RecordBatch");

    let file = std::fs::File::create(path)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.to_string()),
            parquet::file::metadata::KeyValue::new("generator".to_string(), "G^G Biological Compounding & Noyes-Whitney Dissolution v1.0".to_string()),
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
        .unwrap_or_else(|| "../../doe-genesis/topic-1-biotechnology-revolution/data/compounding_pharmaceutical_potency.parquet".to_string());

    println!("====================================================================");
    println!("  G^G KERNEL: BIOLOGICAL COMPOUNDING & DISSOLUTION SWEEP");
    println!("  Target Trajectories: {}", n_trajectories);
    println!("  Simulating Ostwald Rheology, Noyes-Whitney & Shear Potency...");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x434F_4D50); // Seed "COMP"
    let start = Instant::now();

    let mut results = Vec::with_capacity(n_trajectories as usize);
    for i in 0..n_trajectories {
        results.push(run_single_compounding(i, &mut rng));
    }

    let mut master_proof = ProofChain::new();
    master_proof.seed(b"G^G_COMPOUNDING_MASTER_PROOF_v1.0");
    for r in &results {
        master_proof.feed_str(&r.proof_hash);
    }
    let master_seal = master_proof.seal();

    write_parquet_dataset(&out_parquet, &results, &master_seal)
        .expect("Failed to write Parquet dataset");

    let potency_collapsed = results.iter().filter(|r| r.is_potency_collapsed).count();
    let dissolution_stalled = results.iter().filter(|r| r.is_dissolution_stalled).count();

    println!("====================================================================");
    println!("  BIOLOGICAL COMPOUNDING SWEEP COMPLETE");
    println!("  Total Trajectories Simulated: {}", n_trajectories);
    println!("  API Potency Collapse Events:       {} ({:.1}%)", potency_collapsed, (potency_collapsed as f64 / n_trajectories as f64) * 100.0);
    println!("  Dissolution Stalled Events:        {} ({:.1}%)", dissolution_stalled, (dissolution_stalled as f64 / n_trajectories as f64) * 100.0);
    println!("  Master SHA-256 Run Proof:           {}", master_seal);
    println!("  Simulation Time:                    {:?}", start.elapsed());
    println!("  Parquet Dataset Written To:          {}", out_parquet);
    println!("====================================================================\n");
}
