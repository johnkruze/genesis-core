use std::time::Instant;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output;
use genesis_core::rng::Rng;
use genesis_core::physics::terran::SoilType;
use serde::Serialize;
use std::sync::Arc;
use arrow::array::{Float64Array, StringArray, BooleanArray};
use arrow::record_batch::RecordBatch;
use arrow::datatypes::{Schema, Field, DataType};
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::file::properties::WriterProperties;
use parquet::basic::Compression;

#[derive(Debug, Serialize)]
struct GenotypePhenotypeResult {
    id: u32,
    short_id: String,
    soil_type: String,
    genotype_glomalin_express_mg_g: f64,
    genotype_root_osmotic_gain: f64,
    applied_machinery_load_n: f64,
    phenotype_soil_yield_stress_pa: f64,
    compaction_ratio: f64,
    phenotype_emergence_success: bool,
    phenotype_carbon_sequestration_kg_m2: f64,
    proof_hash: String,
}

fn run_single_genotype_phenotype(
    id: u32,
    rng: &mut Rng,
) -> GenotypePhenotypeResult {
    let short_id = output::short_id(rng);
    
    let soil_idx = rng.range(0.0, 4.0) as usize;
    let (soil_type, soil_name) = match soil_idx {
        0 => (SoilType::Sand, "Sand"),
        1 => (SoilType::Loam, "Loam"),
        2 => (SoilType::Clay, "Clay"),
        _ => (SoilType::Andisol, "Andisol"),
    };

    // Genotype Input Parameters (Genetic Expression Profile)
    let g_glomalin = rng.range(0.2, 8.5); // mg/g glomalin expression capacity
    let g_osmotic = rng.range(0.5, 3.0);  // root osmotic pump factor

    // Environmental Stress (Heavy Machinery Load 10 kN - 100 kN)
    let load_n = rng.range(10_000.0, 100_000.0);


    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_str(soil_name);
    proof.feed_f64(g_glomalin);
    proof.feed_f64(g_osmotic);
    proof.feed_f64(load_n);

    // Boussinesq stress bulb at depth 0.3m (root zone)
    let wheel_radius_m = 0.45;
    let contact_area_m2 = std::f64::consts::PI * wheel_radius_m * wheel_radius_m;
    let contact_pressure_pa = load_n / contact_area_m2;

    // Yield stress boosted by genetic glomalin production
    let yield_stress_pa = soil_type.base_yield_stress() + (g_glomalin * soil_type.glomalin_coefficient());

    // Root emergence is successful if compaction pressure does not exceed yield stress by > 50%
    let compaction_ratio = contact_pressure_pa / yield_stress_pa;
    let emergence_success = compaction_ratio < 1.5 && (g_osmotic > 0.8);

    // Carbon sequestration yield scales with hyphal glomalin binding
    let c_seq_kg_m2 = g_glomalin * 0.42 * (if emergence_success { 1.2 } else { 0.3 });

    proof.feed_f64(yield_stress_pa);
    proof.feed_f64(c_seq_kg_m2);
    proof.feed_str(if emergence_success { "EMERGENCE_PASSED" } else { "COMPACTION_BLOCKED" });

    GenotypePhenotypeResult {
        id,
        short_id,
        soil_type: soil_name.to_string(),
        genotype_glomalin_express_mg_g: g_glomalin,
        genotype_root_osmotic_gain: g_osmotic,
        applied_machinery_load_n: load_n,
        phenotype_soil_yield_stress_pa: yield_stress_pa,
        compaction_ratio,
        phenotype_emergence_success: emergence_success,
        phenotype_carbon_sequestration_kg_m2: c_seq_kg_m2,
        proof_hash: proof.seal(),
    }
}

fn write_parquet_dataset(path: &str, results: &[GenotypePhenotypeResult], run_proof: &str) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::Utf8, false),
        Field::new("soil_type", DataType::Utf8, false),
        Field::new("genotype_glomalin_express_mg_g", DataType::Float64, false),
        Field::new("genotype_root_osmotic_gain", DataType::Float64, false),
        Field::new("applied_machinery_load_n", DataType::Float64, false),
        Field::new("phenotype_soil_yield_stress_pa", DataType::Float64, false),
        Field::new("compaction_ratio", DataType::Float64, false),
        Field::new("phenotype_emergence_success", DataType::Boolean, false),
        Field::new("phenotype_carbon_sequestration_kg_m2", DataType::Float64, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let traj_ids: StringArray = results.iter().map(|r| Some(format!("g_p_{}", r.short_id))).collect();
    let soils: StringArray = results.iter().map(|r| Some(r.soil_type.clone())).collect();
    let glomalins: Float64Array = results.iter().map(|r| Some(r.genotype_glomalin_express_mg_g)).collect();
    let osmotics: Float64Array = results.iter().map(|r| Some(r.genotype_root_osmotic_gain)).collect();
    let loads: Float64Array = results.iter().map(|r| Some(r.applied_machinery_load_n)).collect();
    let yields: Float64Array = results.iter().map(|r| Some(r.phenotype_soil_yield_stress_pa)).collect();
    let compactions: Float64Array = results.iter().map(|r| Some(r.compaction_ratio)).collect();
    let emergences: BooleanArray = results.iter().map(|r| Some(r.phenotype_emergence_success)).collect();
    let c_seqs: Float64Array = results.iter().map(|r| Some(r.phenotype_carbon_sequestration_kg_m2)).collect();
    let proofs: StringArray = results.iter().map(|r| Some(r.proof_hash.clone())).collect();

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(traj_ids),
            Arc::new(soils),
            Arc::new(glomalins),
            Arc::new(osmotics),
            Arc::new(loads),
            Arc::new(yields),
            Arc::new(compactions),
            Arc::new(emergences),
            Arc::new(c_seqs),
            Arc::new(proofs),
        ],
    ).expect("Failed to create RecordBatch");

    let file = std::fs::File::create(path)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.to_string()),
            parquet::file::metadata::KeyValue::new("generator".to_string(), "G^G Genotype-Phenotype Terran Physics v1.0".to_string()),
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
        .unwrap_or_else(|| "../../doe-genesis/topic-1-biotechnology-revolution/data/genotype_phenotype_terran_coupling.parquet".to_string());

    println!("====================================================================");
    println!("  G^G KERNEL: GENOTYPE-PHENOTYPE SOIL/MYCELIAL SWEEP");
    println!("  Target Trajectories: {}", n_trajectories);
    println!("  Mapping Genetic Expressivity -> Boussinesq Stress & Phenotypic Yield...");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x4745_4E4F_5459_5045);
    let start = Instant::now();

    let mut results = Vec::with_capacity(n_trajectories as usize);
    for i in 0..n_trajectories {
        results.push(run_single_genotype_phenotype(i, &mut rng));
    }

    let proof_hashes: Vec<_> = results.iter().map(|r| r.proof_hash.clone()).collect();
    let run_proof = proof::seal_run(&proof_hashes);

    write_parquet_dataset(&out_parquet, &results, &run_proof)
        .expect("Failed to write Parquet dataset");

    let emergence_passed = results.iter().filter(|r| r.phenotype_emergence_success).count();

    println!("====================================================================");
    println!("  GENOTYPE-PHENOTYPE SWEEP COMPLETE");
    println!("  Total Trajectories Simulated: {}", n_trajectories);
    println!("  Phenotypic Emergence Successes:    {} ({:.1}%)", emergence_passed, (emergence_passed as f64 / n_trajectories as f64) * 100.0);
    println!("  Master SHA-256 Run Proof:           {}", run_proof);
    println!("  Simulation Time:                    {:?}", start.elapsed());
    println!("  Parquet Dataset Written To:          {}", out_parquet);
    println!("====================================================================\n");
}
