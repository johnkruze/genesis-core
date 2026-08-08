use std::time::Instant;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output;
use genesis_core::rng::Rng;
use genesis_core::physics::materials::{MaterialSampleState, MaterialInverseParams};
use serde::Serialize;
use std::sync::Arc;
use arrow::array::{Float64Array, StringArray, BooleanArray};
use arrow::record_batch::RecordBatch;
use arrow::datatypes::{Schema, Field, DataType};
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::file::properties::WriterProperties;
use parquet::basic::Compression;

#[derive(Debug, Serialize)]
struct MaterialRunResult {
    id: u32,
    short_id: String,
    density_kg_m3: f64,
    applied_load_kn: f64,
    yield_strength_mpa: f64,
    eigenvector_alignment_score: f64,
    sigma_xx_mpa: f64,
    sigma_yy_mpa: f64,
    sigma_zz_mpa: f64,
    tau_xy_mpa: f64,
    tau_xz_mpa: f64,
    tau_yz_mpa: f64,
    principal_stress_1_mpa: f64,
    principal_stress_2_mpa: f64,
    principal_stress_3_mpa: f64,
    von_mises_stress_mpa: f64,
    safety_margin: f64,
    is_yield_failed: bool,
    proof_hash: String,
}

fn run_single_material(
    id: u32,
    rng: &mut Rng,
) -> MaterialRunResult {
    let short_id = output::short_id(rng);
    
    // Sweep density (1800 to 4500 kg/m3), yield strength (150 to 500 MPa), load (200 to 1400 kN), alignment (0.0 to 1.0)
    let density = rng.range(1800.0, 4500.0);
    let yield_mpa = rng.range(150.0, 500.0);
    let load_kn = rng.range(200.0, 1400.0);
    let alignment = rng.range(0.0, 1.0);

    let params = MaterialInverseParams::default();
    let mut state = MaterialSampleState::new(density, yield_mpa, load_kn, alignment);

    state.step(&params, 0.1);

    // Full 3D Cauchy tensor from stepped physics (not post-hoc k×von_mises)
    let t = state.stress_tensor;
    let sigma_xx = t.sigma_xx;
    let sigma_yy = t.sigma_yy;
    let sigma_zz = t.sigma_zz;
    let tau_xy = t.tau_xy;
    let tau_xz = t.tau_xz;
    let tau_yz = t.tau_yz;

    // 2D principal pair in the primary shear plane (xx–yy–xy) + zz as third principal approx
    let avg = 0.5 * (sigma_xx + sigma_yy);
    let diff = 0.5 * (sigma_xx - sigma_yy);
    let radius = (diff.powi(2) + tau_xy.powi(2)).sqrt();
    let p1 = avg + radius;
    let p2 = avg - radius;
    let p3 = sigma_zz;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(density);
    proof.feed_f64(load_kn);
    proof.feed_f64(yield_mpa);
    proof.feed_f64(alignment);
    proof.feed_f64(state.von_mises_stress_mpa);
    proof.feed_f64(sigma_zz);
    proof.feed_f64(tau_xz);

    MaterialRunResult {
        id,
        short_id,
        density_kg_m3: density,
        applied_load_kn: load_kn,
        yield_strength_mpa: yield_mpa,
        eigenvector_alignment_score: alignment,
        sigma_xx_mpa: sigma_xx,
        sigma_yy_mpa: sigma_yy,
        sigma_zz_mpa: sigma_zz,
        tau_xy_mpa: tau_xy,
        tau_xz_mpa: tau_xz,
        tau_yz_mpa: tau_yz,
        principal_stress_1_mpa: p1,
        principal_stress_2_mpa: p2,
        principal_stress_3_mpa: p3,
        von_mises_stress_mpa: state.von_mises_stress_mpa,
        safety_margin: state.safety_margin,
        is_yield_failed: state.is_yield_failed,
        proof_hash: proof.seal(),
    }
}

fn write_parquet_dataset(path: &str, results: &[MaterialRunResult], run_proof: &str) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::Utf8, false),
        Field::new("density_kg_m3", DataType::Float64, false),
        Field::new("applied_load_kn", DataType::Float64, false),
        Field::new("yield_strength_mpa", DataType::Float64, false),
        Field::new("eigenvector_alignment_score", DataType::Float64, false),
        Field::new("sigma_xx_mpa", DataType::Float64, false),
        Field::new("sigma_yy_mpa", DataType::Float64, false),
        Field::new("sigma_zz_mpa", DataType::Float64, false),
        Field::new("tau_xy_mpa", DataType::Float64, false),
        Field::new("tau_xz_mpa", DataType::Float64, false),
        Field::new("tau_yz_mpa", DataType::Float64, false),
        Field::new("principal_stress_1_mpa", DataType::Float64, false),
        Field::new("principal_stress_2_mpa", DataType::Float64, false),
        Field::new("principal_stress_3_mpa", DataType::Float64, false),
        Field::new("von_mises_stress_mpa", DataType::Float64, false),
        Field::new("safety_margin", DataType::Float64, false),
        Field::new("is_yield_failed", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let traj_ids: StringArray = results.iter().map(|r| Some(format!("material_{}", r.short_id))).collect();
    let densities: Float64Array = results.iter().map(|r| Some(r.density_kg_m3)).collect();
    let loads: Float64Array = results.iter().map(|r| Some(r.applied_load_kn)).collect();
    let yields: Float64Array = results.iter().map(|r| Some(r.yield_strength_mpa)).collect();
    let alignments: Float64Array = results.iter().map(|r| Some(r.eigenvector_alignment_score)).collect();
    let s_xx: Float64Array = results.iter().map(|r| Some(r.sigma_xx_mpa)).collect();
    let s_yy: Float64Array = results.iter().map(|r| Some(r.sigma_yy_mpa)).collect();
    let s_zz: Float64Array = results.iter().map(|r| Some(r.sigma_zz_mpa)).collect();
    let t_xy: Float64Array = results.iter().map(|r| Some(r.tau_xy_mpa)).collect();
    let t_xz: Float64Array = results.iter().map(|r| Some(r.tau_xz_mpa)).collect();
    let t_yz: Float64Array = results.iter().map(|r| Some(r.tau_yz_mpa)).collect();
    let p1s: Float64Array = results.iter().map(|r| Some(r.principal_stress_1_mpa)).collect();
    let p2s: Float64Array = results.iter().map(|r| Some(r.principal_stress_2_mpa)).collect();
    let p3s: Float64Array = results.iter().map(|r| Some(r.principal_stress_3_mpa)).collect();
    let stresses: Float64Array = results.iter().map(|r| Some(r.von_mises_stress_mpa)).collect();
    let margins: Float64Array = results.iter().map(|r| Some(r.safety_margin)).collect();
    let failures: BooleanArray = results.iter().map(|r| Some(r.is_yield_failed)).collect();
    let proofs: StringArray = results.iter().map(|r| Some(r.proof_hash.clone())).collect();

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(traj_ids),
            Arc::new(densities),
            Arc::new(loads),
            Arc::new(yields),
            Arc::new(alignments),
            Arc::new(s_xx),
            Arc::new(s_yy),
            Arc::new(s_zz),
            Arc::new(t_xy),
            Arc::new(t_xz),
            Arc::new(t_yz),
            Arc::new(p1s),
            Arc::new(p2s),
            Arc::new(p3s),
            Arc::new(stresses),
            Arc::new(margins),
            Arc::new(failures),
            Arc::new(proofs),
        ],
    ).expect("Failed to create RecordBatch");

    let file = std::fs::File::create(path)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.to_string()),
            parquet::file::metadata::KeyValue::new("generator".to_string(), "G^G Eigenvector Inverse Material Design v1.0".to_string()),
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
        .unwrap_or_else(|| "../../doe-genesis/topic-3-materials-predictable-functionality/data/materials_inverse_design_eigenvectors.parquet".to_string());

    println!("====================================================================");
    println!("  G^G KERNEL: ADVANCED MATERIALS INVERSE DESIGN SWEEP");
    println!("  Target Trajectories: {}", n_trajectories);
    println!("  Simulating Cauchy Stress Tensors, Von Mises Yield & Eigenvector Alignment...");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x4D41_5445_5249_414C);
    let start = Instant::now();

    let mut results = Vec::with_capacity(n_trajectories as usize);
    for i in 0..n_trajectories {
        results.push(run_single_material(i, &mut rng));
    }

    let proof_hashes: Vec<_> = results.iter().map(|r| r.proof_hash.clone()).collect();
    let run_proof = proof::seal_run(&proof_hashes);

    write_parquet_dataset(&out_parquet, &results, &run_proof)
        .expect("Failed to write Parquet dataset");

    let passed_runs = results.iter().filter(|r| !r.is_yield_failed).count();
    let failed_runs = n_trajectories as usize - passed_runs;

    println!("====================================================================");
    println!("  MATERIALS INVERSE DESIGN SWEEP COMPLETE");
    println!("  Total Trajectories Simulated: {}", n_trajectories);
    println!("  Structural Inverse Design Passes:   {} ({:.1}%)", passed_runs, (passed_runs as f64 / n_trajectories as f64) * 100.0);
    println!("  Yield Limit Exceeded Failures:     {} ({:.1}%)", failed_runs, (failed_runs as f64 / n_trajectories as f64) * 100.0);
    println!("  Master SHA-256 Run Proof:           {}", run_proof);
    println!("  Simulation Time:                    {:?}", start.elapsed());
    println!("  Parquet Dataset Written To:          {}", out_parquet);
    println!("====================================================================\n");
}
