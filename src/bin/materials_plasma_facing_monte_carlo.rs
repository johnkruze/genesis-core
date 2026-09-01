use std::time::Instant;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output;
use genesis_core::rng::Rng;
use genesis_core::physics::plasma_facing::{
    PlasmaFacingMaterialState, PlasmaFacingDesignParams, ABLATION_LIMIT_MM, STRESS_LIMIT_MPA,
};
use serde::Serialize;
use std::sync::Arc;
use arrow::array::{Float64Array, StringArray, BooleanArray};
use arrow::record_batch::RecordBatch;
use arrow::datatypes::{Schema, Field, DataType};
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::file::properties::WriterProperties;
use parquet::basic::Compression;

#[derive(Debug, Serialize)]
struct PlasmaRunResult {
    id: u32,
    short_id: String,
    plasma_temperature_k: f64,
    heat_flux_mw_m2: f64,
    surface_temperature_k: f64,
    ablation_recession_depth_mm: f64,
    thermal_stress_mpa: f64,
    thermal_crack_safety_margin: f64,
    is_thermal_stress_failed: bool,
    is_ablation_failed: bool,
    is_ablation_spallation_failed: bool,
    proof_hash: String,
}

fn run_single_plasma(id: u32, rng: &mut Rng) -> PlasmaRunResult {
    let short_id = output::short_id(rng);

    // Sweep plasma temp (1500K to 4500K) and heat flux (2.0 to 25.0 MW/m2)
    let t_plasma = rng.range(1500.0, 4500.0);
    let flux_mw = rng.range(2.0, 25.0);

    let params = PlasmaFacingDesignParams::default();
    let mut state = PlasmaFacingMaterialState::new(t_plasma, flux_mw);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(t_plasma);
    proof.feed_f64(flux_mw);
    proof.feed_f64(STRESS_LIMIT_MPA);
    proof.feed_f64(ABLATION_LIMIT_MM);

    let dt_sec = 0.05; // 50ms pulse timestep
    let total_steps = 100; // 5-second plasma pulse shockwave

    for step in 0..total_steps {
        state.step(&params, dt_sec);

        if step % 20 == 0 {
            proof.feed_f64(state.surface_temperature_k);
            proof.feed_f64(state.thermal_stress_mpa);
            proof.feed_f64(state.ablation_recession_depth_mm);
        }
    }

    proof.feed_f64(state.surface_temperature_k);
    let tag = match (state.is_thermal_stress_failed, state.is_ablation_failed) {
        (true, true) => "STRESS_AND_ABLATION_FAILED",
        (true, false) => "THERMAL_STRESS_FAILED",
        (false, true) => "ABLATION_FAILED",
        (false, false) => "PLASMA_BARRIER_PASSED",
    };
    proof.feed_str(tag);

    PlasmaRunResult {
        id,
        short_id,
        plasma_temperature_k: t_plasma,
        heat_flux_mw_m2: flux_mw,
        surface_temperature_k: state.surface_temperature_k,
        ablation_recession_depth_mm: state.ablation_recession_depth_mm,
        thermal_stress_mpa: state.thermal_stress_mpa,
        thermal_crack_safety_margin: state.thermal_crack_safety_margin,
        is_thermal_stress_failed: state.is_thermal_stress_failed,
        is_ablation_failed: state.is_ablation_failed,
        is_ablation_spallation_failed: state.is_ablation_spallation_failed,
        proof_hash: proof.seal(),
    }
}

fn write_parquet_dataset(path: &str, results: &[PlasmaRunResult], run_proof: &str) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::Utf8, false),
        Field::new("plasma_temperature_k", DataType::Float64, false),
        Field::new("heat_flux_mw_m2", DataType::Float64, false),
        Field::new("surface_temperature_k", DataType::Float64, false),
        Field::new("ablation_recession_depth_mm", DataType::Float64, false),
        Field::new("thermal_stress_mpa", DataType::Float64, false),
        Field::new("thermal_crack_safety_margin", DataType::Float64, false),
        Field::new("is_thermal_stress_failed", DataType::Boolean, false),
        Field::new("is_ablation_failed", DataType::Boolean, false),
        Field::new("is_ablation_spallation_failed", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let traj_ids: StringArray = results.iter().map(|r| Some(format!("plasma_{}", r.short_id))).collect();
    let temps: Float64Array = results.iter().map(|r| Some(r.plasma_temperature_k)).collect();
    let fluxes: Float64Array = results.iter().map(|r| Some(r.heat_flux_mw_m2)).collect();
    let surfaces: Float64Array = results.iter().map(|r| Some(r.surface_temperature_k)).collect();
    let recessions: Float64Array = results.iter().map(|r| Some(r.ablation_recession_depth_mm)).collect();
    let stresses: Float64Array = results.iter().map(|r| Some(r.thermal_stress_mpa)).collect();
    let margins: Float64Array = results.iter().map(|r| Some(r.thermal_crack_safety_margin)).collect();
    let stress_fails: BooleanArray = results.iter().map(|r| Some(r.is_thermal_stress_failed)).collect();
    let abl_fails: BooleanArray = results.iter().map(|r| Some(r.is_ablation_failed)).collect();
    let failures: BooleanArray = results.iter().map(|r| Some(r.is_ablation_spallation_failed)).collect();
    let proofs: StringArray = results.iter().map(|r| Some(r.proof_hash.clone())).collect();

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(traj_ids),
            Arc::new(temps),
            Arc::new(fluxes),
            Arc::new(surfaces),
            Arc::new(recessions),
            Arc::new(stresses),
            Arc::new(margins),
            Arc::new(stress_fails),
            Arc::new(abl_fails),
            Arc::new(failures),
            Arc::new(proofs),
        ],
    )
    .expect("Failed to create RecordBatch");

    let file = std::fs::File::create(path)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.to_string()),
            parquet::file::metadata::KeyValue::new(
                "generator".to_string(),
                "G^G Plasma Facing Extreme Alloys v1.1 dual-axis".to_string(),
            ),
            parquet::file::metadata::KeyValue::new(
                "stress_limit_mpa".to_string(),
                format!("{}", STRESS_LIMIT_MPA),
            ),
            parquet::file::metadata::KeyValue::new(
                "ablation_limit_mm".to_string(),
                format!("{}", ABLATION_LIMIT_MM),
            ),
        ]))
        .build();

    let mut writer = ArrowWriter::try_new(file, schema, Some(props)).expect("ArrowWriter");
    writer.write(&batch).expect("write");
    writer.close().expect("close");
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n_trajectories: u32 = args
        .get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(2_500);

    let out_parquet = args
        .iter()
        .position(|a| a == "--parquet")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| {
            "../../doe-genesis/topic-3-materials-predictable-functionality/data/materials_plasma_facing_extreme_alloys.parquet"
                .to_string()
        });

    println!("====================================================================");
    println!("  G^G KERNEL: PLASMA-FACING EXTREME ENERGY ALLOY SWEEP (dual-axis)");
    println!("  Target Trajectories: {}", n_trajectories);
    println!(
        "  Policy: stress>{} MPa OR ablation>{} mm",
        STRESS_LIMIT_MPA, ABLATION_LIMIT_MM
    );
    println!("====================================================================\n");

    let mut rng = Rng::new(0x504C_4153_4D41_414C);
    let start = Instant::now();

    let mut results = Vec::with_capacity(n_trajectories as usize);
    for i in 0..n_trajectories {
        results.push(run_single_plasma(i, &mut rng));
    }

    let proof_hashes: Vec<_> = results.iter().map(|r| r.proof_hash.clone()).collect();
    let run_proof = proof::seal_run(&proof_hashes);

    write_parquet_dataset(&out_parquet, &results, &run_proof).expect("write parquet");

    let composite = results.iter().filter(|r| r.is_ablation_spallation_failed).count();
    let stress_only = results
        .iter()
        .filter(|r| r.is_thermal_stress_failed && !r.is_ablation_failed)
        .count();
    let abl_only = results
        .iter()
        .filter(|r| r.is_ablation_failed && !r.is_thermal_stress_failed)
        .count();
    let both = results
        .iter()
        .filter(|r| r.is_thermal_stress_failed && r.is_ablation_failed)
        .count();

    println!("====================================================================");
    println!("  PLASMA-FACING SWEEP COMPLETE");
    println!("  Total Trajectories:                 {}", n_trajectories);
    println!(
        "  Composite fail:                     {} ({:.1}%)",
        composite,
        (composite as f64 / n_trajectories as f64) * 100.0
    );
    println!(
        "    stress-only / ablation-only / both: {} / {} / {}",
        stress_only, abl_only, both
    );
    println!("  Master SHA-256 Run Proof:           {}", run_proof);
    println!("  Simulation Time:                    {:?}", start.elapsed());
    println!("  Parquet:                            {}", out_parquet);
    println!("====================================================================\n");
}
