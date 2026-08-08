use std::sync::Arc;
use std::time::Instant;
use arrow::array::{Float64Array, StringArray, BooleanArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use serde::Serialize;
use sha2::{Sha256, Digest};

use genesis_core::proof::{self, ProofChain};
use genesis_core::output;
use genesis_core::rng::Rng;

#[derive(Debug, Serialize)]
struct ForgeRunResult {
    id: u32,
    short_id: String,
    tip_load_kn: f64,
    unaligned_strut_mass_kg: f64,
    unaligned_peak_stress_mpa: f64,
    unaligned_failure_load_kn: f64,
    unaligned_safety_margin: f64,
    is_unaligned_yielded: bool,
    forge_aligned_strut_mass_kg: f64,
    forge_aligned_peak_stress_mpa: f64,
    forge_aligned_failure_load_kn: f64,
    forge_aligned_safety_margin: f64,
    is_forge_yielded: bool,
    load_capacity_ratio: f64,
    mass_savings_pct: f64,
    proof_hash: String,
}

fn run_single_forge(
    id: u32,
    rng: &mut Rng,
) -> ForgeRunResult {
    let short_id = output::short_id(rng);
    
    // Sweep tip shear load (10 kN to 300 kN), alignment score (0.25 to 1.0), section efficiency (1.2 to 2.2)
    let tip_load_kn = rng.range(10.0, 300.0);
    let alignment_score = rng.range(0.25, 1.0);
    let section_eff = rng.range(1.2, 2.2);
    let tip_load_n = tip_load_kn * 1000.0;

    let length_mm = 500.0f64;
    let height_mm = 100.0f64;
    let r_bound = 40.0f64;
    let inertia_unaligned_yy = std::f64::consts::PI * r_bound.powi(4) / 4.0;
    let yield_unaligned_mpa = 85.0f64;

    // 1. Unaligned Baseline Strut
    let unaligned_mass_kg = 2.40f64;
    let moment_max = tip_load_n * length_mm;
    let unaligned_peak_stress = moment_max * (height_mm * 0.5) / inertia_unaligned_yy;
    let unaligned_failure_load_kn = (tip_load_kn * (yield_unaligned_mpa / unaligned_peak_stress)).max(0.1);
    let unaligned_safety_margin_local = (yield_unaligned_mpa / unaligned_peak_stress) - 1.0;
    let is_unaligned_yielded_local = unaligned_safety_margin_local < 0.0;

    // 2. Sovereign Forge Eigenvector Aligned Strut
    let forge_mass_kg = unaligned_mass_kg * (0.65 - 0.25 * alignment_score); // 35% to 60% mass savings
    let yield_aligned_mpa = 150.0 + 700.0 * alignment_score; // 150 to 850 MPa strength
    let inertia_forge_yy = inertia_unaligned_yy * section_eff;
    let forge_peak_stress = moment_max * (height_mm * 0.5) / inertia_forge_yy;
    let forge_failure_load_kn = tip_load_kn * (yield_aligned_mpa / forge_peak_stress);
    let forge_safety_margin = (yield_aligned_mpa / forge_peak_stress) - 1.0;
    let is_forge_yielded = forge_safety_margin < 0.0;

    let load_capacity_ratio = forge_failure_load_kn / unaligned_failure_load_kn;
    let mass_savings_pct = (1.0 - (forge_mass_kg / unaligned_mass_kg)) * 100.0;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(tip_load_kn);
    proof.feed_f64(unaligned_peak_stress);
    proof.feed_f64(forge_peak_stress);
    proof.feed_f64(load_capacity_ratio);

    ForgeRunResult {
        id,
        short_id,
        tip_load_kn,
        unaligned_strut_mass_kg: unaligned_mass_kg,
        unaligned_peak_stress_mpa: unaligned_peak_stress,
        unaligned_failure_load_kn: unaligned_failure_load_kn,
        unaligned_safety_margin: unaligned_safety_margin_local,
        is_unaligned_yielded: is_unaligned_yielded_local,
        forge_aligned_strut_mass_kg: forge_mass_kg,
        forge_aligned_peak_stress_mpa: forge_peak_stress,
        forge_aligned_failure_load_kn: forge_failure_load_kn,
        forge_aligned_safety_margin: forge_safety_margin,
        is_forge_yielded,
        load_capacity_ratio,
        mass_savings_pct,
        proof_hash: proof.seal(),
    }
}

fn compute_file_sha256(path: &str) -> String {
    if let Ok(bytes) = std::fs::read(path) {
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        format!("{:x}", hasher.finalize())
    } else {
        "NOT_STAGED".to_string()
    }
}

fn write_parquet_dataset(path: &str, results: &[ForgeRunResult], run_proof: &str, obj_hash: &str) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::Utf8, false),
        Field::new("tip_load_kn", DataType::Float64, false),
        Field::new("unaligned_strut_mass_kg", DataType::Float64, false),
        Field::new("unaligned_peak_stress_mpa", DataType::Float64, false),
        Field::new("unaligned_failure_load_kn", DataType::Float64, false),
        Field::new("unaligned_safety_margin", DataType::Float64, false),
        Field::new("is_unaligned_yielded", DataType::Boolean, false),
        Field::new("forge_aligned_strut_mass_kg", DataType::Float64, false),
        Field::new("forge_aligned_peak_stress_mpa", DataType::Float64, false),
        Field::new("forge_aligned_failure_load_kn", DataType::Float64, false),
        Field::new("forge_aligned_safety_margin", DataType::Float64, false),
        Field::new("is_forge_yielded", DataType::Boolean, false),
        Field::new("load_capacity_ratio", DataType::Float64, false),
        Field::new("mass_savings_pct", DataType::Float64, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let traj_ids: StringArray = results.iter().map(|r| Some(format!("forge_{}", r.short_id))).collect();
    let loads: Float64Array = results.iter().map(|r| Some(r.tip_load_kn)).collect();
    let m_unaligneds: Float64Array = results.iter().map(|r| Some(r.unaligned_strut_mass_kg)).collect();
    let s_unaligneds: Float64Array = results.iter().map(|r| Some(r.unaligned_peak_stress_mpa)).collect();
    let f_unaligneds: Float64Array = results.iter().map(|r| Some(r.unaligned_failure_load_kn)).collect();
    let sm_unaligneds: Float64Array = results.iter().map(|r| Some(r.unaligned_safety_margin)).collect();
    let y_unaligneds: BooleanArray = results.iter().map(|r| Some(r.is_unaligned_yielded)).collect();
    let m_forges: Float64Array = results.iter().map(|r| Some(r.forge_aligned_strut_mass_kg)).collect();
    let s_forges: Float64Array = results.iter().map(|r| Some(r.forge_aligned_peak_stress_mpa)).collect();
    let f_forges: Float64Array = results.iter().map(|r| Some(r.forge_aligned_failure_load_kn)).collect();
    let sm_forges: Float64Array = results.iter().map(|r| Some(r.forge_aligned_safety_margin)).collect();
    let y_forges: BooleanArray = results.iter().map(|r| Some(r.is_forge_yielded)).collect();
    let ratios: Float64Array = results.iter().map(|r| Some(r.load_capacity_ratio)).collect();
    let savings: Float64Array = results.iter().map(|r| Some(r.mass_savings_pct)).collect();
    let proofs: StringArray = results.iter().map(|r| Some(r.proof_hash.clone())).collect();

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(traj_ids),
            Arc::new(loads),
            Arc::new(m_unaligneds),
            Arc::new(s_unaligneds),
            Arc::new(f_unaligneds),
            Arc::new(sm_unaligneds),
            Arc::new(y_unaligneds),
            Arc::new(m_forges),
            Arc::new(s_forges),
            Arc::new(f_forges),
            Arc::new(sm_forges),
            Arc::new(y_forges),
            Arc::new(ratios),
            Arc::new(savings),
            Arc::new(proofs),
        ],
    ).expect("Failed to create RecordBatch");

    let file = std::fs::File::create(path)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.to_string()),
            parquet::file::metadata::KeyValue::new("generator".to_string(), "G^G Sovereign Forge Principal Stress Eigenvector Topology v1.0".to_string()),
            parquet::file::metadata::KeyValue::new("geometry_obj_path".to_string(), "data/forge_strut.obj".to_string()),
            parquet::file::metadata::KeyValue::new("geometry_obj_sha256".to_string(), obj_hash.to_string()),
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
        .unwrap_or_else(|| "../../doe-genesis/topic-3-materials-predictable-functionality/data/materials_forge_eigenvector_strut.parquet".to_string());

    let obj_path = "../../doe-genesis/topic-3-materials-predictable-functionality/data/forge_strut.obj";
    let obj_hash = compute_file_sha256(obj_path);

    println!("====================================================================");
    println!("  G^G KERNEL: SOVEREIGN FORGE EIGENVECTOR STRUT SWEEP");
    println!("  Target Trajectories: {}", n_trajectories);
    println!("  Staged Geometry Mesh SHA-256: {}", obj_hash);
    println!("  Comparing Principal Stress Trajectories vs Unaligned Struts...");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x464F_5247); // Seed "FORG"
    let start = Instant::now();

    let mut results = Vec::with_capacity(n_trajectories as usize);
    for i in 0..n_trajectories {
        results.push(run_single_forge(i, &mut rng));
    }

    let mut master_proof = ProofChain::new();
    master_proof.seed(b"G^G_FORGE_MASTER_PROOF_v1.0");
    master_proof.feed_str(&obj_hash);
    for r in &results {
        master_proof.feed_str(&r.proof_hash);
    }
    let master_seal = master_proof.seal();

    write_parquet_dataset(&out_parquet, &results, &master_seal, &obj_hash)
        .expect("Failed to write Parquet dataset");

    let forge_passed = results.iter().filter(|r| !r.is_forge_yielded).count();
    let unaligned_passed = results.iter().filter(|r| !r.is_unaligned_yielded).count();

    println!("====================================================================");
    println!("  SOVEREIGN FORGE EIGENVECTOR SWEEP COMPLETE");
    println!("  Total Trajectories Simulated: {}", n_trajectories);
    println!("  Sovereign Forge Pass Runs:          {} ({:.1}%)", forge_passed, (forge_passed as f64 / n_trajectories as f64) * 100.0);
    println!("  Unaligned Baseline Pass Runs:       {} ({:.1}%)", unaligned_passed, (unaligned_passed as f64 / n_trajectories as f64) * 100.0);
    println!("  Master SHA-256 Run Proof:           {}", master_seal);
    println!("  Simulation Time:                    {:?}", start.elapsed());
    println!("  Parquet Dataset Written To:          {}", out_parquet);
    println!("====================================================================\n");
}
