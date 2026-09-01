use std::time::Instant;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output;
use genesis_core::rng::Rng;
use genesis_core::physics::bioreactor::{BioreactorVesselState, BioreactorDesignParams};
use serde::Serialize;
use std::sync::Arc;
use arrow::array::{Float64Array, StringArray, BooleanArray};
use arrow::record_batch::RecordBatch;
use arrow::datatypes::{Schema, Field, DataType};
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::file::properties::WriterProperties;
use parquet::basic::Compression;

#[derive(Debug, Serialize)]
struct BioreactorRunResult {
    id: u32,
    short_id: String,
    vessel_volume_liters: f64,
    impeller_speed_rpm: f64,
    fluid_viscosity_pascal_sec: f64,
    max_shear_stress_pa: f64,
    kla_mass_transfer_hr: f64,
    cellular_viability_pct: f64,
    product_yield_g_l: f64,
    is_shear_damaged: bool,
    /// Dual-axis O2 transfer: kLa below process minimum
    is_kla_starved: bool,
    proof_hash: String,
}

fn run_single_bioreactor(
    id: u32,
    rng: &mut Rng,
) -> BioreactorRunResult {
    let short_id = output::short_id(rng);
    
    // Sweep vessel scale (1,000L to 50,000L), impeller RPM (60 to 450), viscosity (0.01 to 0.5 Pa*s)
    let volume_l = rng.range(1_000.0, 50_000.0);
    let rpm = rng.range(60.0, 450.0);
    let viscosity = rng.range(0.01, 0.5);

    let params = BioreactorDesignParams::default();
    let mut vessel = BioreactorVesselState::new(volume_l, rpm, viscosity);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(volume_l);
    proof.feed_f64(rpm);
    proof.feed_f64(viscosity);

    let dt_hr = 0.1; // 6 minute timesteps
    let total_steps = 100; // 10 hour continuous run

    for step in 0..total_steps {
        vessel.step(&params, dt_hr);

        if step % 20 == 0 {
            proof.feed_f64(vessel.max_shear_stress_pa);
            proof.feed_f64(vessel.product_yield_g_l);
        }
    }

    // Dual-axis product policy: shear damage OR oxygen-transfer starvation
    const KLA_PROCESS_MIN_HR: f64 = 45.0;
    let is_kla_starved = vessel.kla_mass_transfer_hr < KLA_PROCESS_MIN_HR;

    proof.feed_f64(vessel.product_yield_g_l);
    proof.feed_f64(vessel.kla_mass_transfer_hr);
    let tag = match (vessel.is_shear_damaged, is_kla_starved) {
        (true, true) => "SHEAR_AND_KLA_FAIL",
        (true, false) => "SHEAR_DAMAGED",
        (false, true) => "KLA_STARVED",
        (false, false) => "OPTIMAL_YIELD",
    };
    proof.feed_str(tag);

    BioreactorRunResult {
        id,
        short_id,
        vessel_volume_liters: volume_l,
        impeller_speed_rpm: rpm,
        fluid_viscosity_pascal_sec: viscosity,
        max_shear_stress_pa: vessel.max_shear_stress_pa,
        kla_mass_transfer_hr: vessel.kla_mass_transfer_hr,
        cellular_viability_pct: vessel.cellular_viability_pct,
        product_yield_g_l: vessel.product_yield_g_l,
        is_shear_damaged: vessel.is_shear_damaged,
        is_kla_starved,
        proof_hash: proof.seal(),
    }
}

fn write_parquet_dataset(path: &str, results: &[BioreactorRunResult], run_proof: &str) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::Utf8, false),
        Field::new("vessel_volume_liters", DataType::Float64, false),
        Field::new("impeller_speed_rpm", DataType::Float64, false),
        Field::new("fluid_viscosity_pascal_sec", DataType::Float64, false),
        Field::new("max_shear_stress_pa", DataType::Float64, false),
        Field::new("kla_mass_transfer_hr", DataType::Float64, false),
        Field::new("cellular_viability_pct", DataType::Float64, false),
        Field::new("product_yield_g_l", DataType::Float64, false),
        Field::new("is_shear_damaged", DataType::Boolean, false),
        Field::new("is_kla_starved", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let traj_ids: StringArray = results.iter().map(|r| Some(format!("bioreactor_{}", r.short_id))).collect();
    let vols: Float64Array = results.iter().map(|r| Some(r.vessel_volume_liters)).collect();
    let rpms: Float64Array = results.iter().map(|r| Some(r.impeller_speed_rpm)).collect();
    let viscosities: Float64Array = results.iter().map(|r| Some(r.fluid_viscosity_pascal_sec)).collect();
    let shears: Float64Array = results.iter().map(|r| Some(r.max_shear_stress_pa)).collect();
    let klas: Float64Array = results.iter().map(|r| Some(r.kla_mass_transfer_hr)).collect();
    let viabilities: Float64Array = results.iter().map(|r| Some(r.cellular_viability_pct)).collect();
    let yields: Float64Array = results.iter().map(|r| Some(r.product_yield_g_l)).collect();
    let damages: BooleanArray = results.iter().map(|r| Some(r.is_shear_damaged)).collect();
    let kla_starved: BooleanArray = results.iter().map(|r| Some(r.is_kla_starved)).collect();
    let proofs: StringArray = results.iter().map(|r| Some(r.proof_hash.clone())).collect();

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(traj_ids),
            Arc::new(vols),
            Arc::new(rpms),
            Arc::new(viscosities),
            Arc::new(shears),
            Arc::new(klas),
            Arc::new(viabilities),
            Arc::new(yields),
            Arc::new(damages),
            Arc::new(kla_starved),
            Arc::new(proofs),
        ],
    ).expect("Failed to create RecordBatch");

    let file = std::fs::File::create(path)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.to_string()),
            parquet::file::metadata::KeyValue::new(
                "generator".to_string(),
                "G^G Bioreactor Impeller Tip Shear & kLa Transfer v1.1 dual-axis".to_string(),
            ),
            parquet::file::metadata::KeyValue::new("kla_process_min_hr".to_string(), "45".to_string()),
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
        .unwrap_or_else(|| "../../doe-genesis/topic-1-biotechnology-revolution/data/bioreactor_biomanufacturing_impeller_shear.parquet".to_string());

    println!("====================================================================");
    println!("  G^G KERNEL: BIOREACTOR IMPELLER TIP SHEAR & kLa SWEEP");
    println!("  Target Trajectories: {}", n_trajectories);
    println!("  Simulating 3D Non-Newtonian Tip Shear & kLa Transfer...");
    println!("====================================================================");

    let start = Instant::now();
    let mut rng = Rng::new(0x4249_4F52); // Seed "BIOR"

    let results: Vec<BioreactorRunResult> = (0..n_trajectories)
        .map(|id| run_single_bioreactor(id, &mut rng))
        .collect();

    let duration = start.elapsed();
    let total_count = results.len();
    let undamaged_count = results.iter().filter(|r| !r.is_shear_damaged).count();
    let damaged_count = total_count - undamaged_count;

    let mut master_proof = ProofChain::new();
    master_proof.seed(b"G^G_BIOREACTOR_MASTER_PROOF_v1.0");
    for r in &results {
        master_proof.feed_str(&r.proof_hash);
    }
    let master_seal = master_proof.seal();

    // Execute inverse design optimization over sealed trajectory space
    let optimal = results.iter()
        .filter(|r| !r.is_shear_damaged)
        .max_by(|a, b| a.product_yield_g_l.partial_cmp(&b.product_yield_g_l).unwrap());

    if let Some(opt) = optimal {
        println!("  [INVERSE OPTIMIZER] Optimal Yield: {:.2} g/L | Vol: {:.0}L | Shear: {:.1} Pa | kLa: {:.1}/hr",
            opt.product_yield_g_l, opt.vessel_volume_liters, opt.max_shear_stress_pa, opt.kla_mass_transfer_hr);
    }

    write_parquet_dataset(&out_parquet, &results, &master_seal).expect("Failed to write Parquet dataset");

    let optimal_runs = results.iter().filter(|r| !r.is_shear_damaged).count();
    let damaged_runs = n_trajectories as usize - optimal_runs;

    println!("====================================================================");
    println!("  BIOREACTOR SWEEP COMPLETE");
    println!("  Total Trajectories Simulated: {}", n_trajectories);
    println!("  Optimal Bio-Product Yield Runs:     {} ({:.1}%)", optimal_runs, (optimal_runs as f64 / n_trajectories as f64) * 100.0);
    println!("  Shear Stress Cellular Lysis Runs:   {} ({:.1}%)", damaged_runs, (damaged_runs as f64 / n_trajectories as f64) * 100.0);
    println!("  Master SHA-256 Run Proof:           {}", master_seal);
    println!("  Simulation Time:                    {:?}", start.elapsed());
    println!("  Parquet Dataset Written To:          {}", out_parquet);
    println!("====================================================================\n");
}
