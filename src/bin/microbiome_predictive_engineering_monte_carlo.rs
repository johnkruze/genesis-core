use std::time::Instant;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output;
use genesis_core::rng::Rng;
use genesis_core::physics::microbiome::{SpatialMicrobiomeField, MicrobiomeKineticsParams};
use serde::Serialize;
use std::sync::Arc;
use arrow::array::{Float64Array, StringArray, BooleanArray};
use arrow::record_batch::RecordBatch;
use arrow::datatypes::{Schema, Field, DataType};
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::file::properties::WriterProperties;
use parquet::basic::Compression;

#[derive(Debug, Serialize)]
struct MicrobiomeRunResult {
    id: u32,
    short_id: String,
    gene_expression_gain_kp: f64,
    diffusion_species_0: f64,
    growth_species_0: f64,
    final_shannon_diversity_index: f64,
    max_species_fraction: f64,
    is_dysbiosis_collapsed: bool,
    proof_hash: String,
}

fn run_single_microbiome(
    id: u32,
    rng: &mut Rng,
) -> MicrobiomeRunResult {
    let short_id = output::short_id(rng);
    
    // Sweep dynamic gene expression feedback gain Kp (0.1 to 4.0), competitive interaction penalty, and diffusion rates
    let kp = rng.range(0.1, 4.0);
    let diff_0 = rng.range(1e-5, 5e-4);
    let growth_0 = rng.range(0.1, 1.2);
    let alpha_comp = rng.range(-1.2, 0.2); // Strong competitive suppression to induce dysbiosis

    let params = MicrobiomeKineticsParams {
        diffusion_coefficients: vec![diff_0, 5e-5, 2e-4],
        growth_rates: vec![growth_0, 0.8, 0.3],
        interaction_matrix: vec![
            vec![0.0, alpha_comp, 0.1],
            vec![-0.1, 0.0, alpha_comp],
            vec![0.2, alpha_comp, 0.0],
        ],
        gene_expression_gain_kp: kp,
        ..Default::default()
    };

    let mut field = SpatialMicrobiomeField::new(50, 3); // 50 grid nodes, 3 species
    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(kp);
    proof.feed_f64(diff_0);
    proof.feed_f64(growth_0);

    let dt_sec = 0.1;
    let total_steps = 200;

    for step in 0..total_steps {
        field.step(&params, dt_sec);

        if step % 25 == 0 {
            proof.feed_f64(field.community_shannon_diversity_index);
        }
    }

    proof.feed_f64(field.community_shannon_diversity_index);
    proof.feed_str(if field.is_dysbiosis_collapsed { "DYSBIOSIS_COLLAPSED" } else { "COMMUNITY_STABLE" });

    MicrobiomeRunResult {
        id,
        short_id,
        gene_expression_gain_kp: kp,
        diffusion_species_0: diff_0,
        growth_species_0: growth_0,
        final_shannon_diversity_index: field.community_shannon_diversity_index,
        max_species_fraction: field.max_species_fraction,
        is_dysbiosis_collapsed: field.is_dysbiosis_collapsed,
        proof_hash: proof.seal(),
    }
}

fn write_parquet_dataset(path: &str, results: &[MicrobiomeRunResult], run_proof: &str) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::Utf8, false),
        Field::new("gene_expression_gain_kp", DataType::Float64, false),
        Field::new("diffusion_species_0", DataType::Float64, false),
        Field::new("growth_species_0", DataType::Float64, false),
        Field::new("final_shannon_diversity_index", DataType::Float64, false),
        Field::new("max_species_fraction", DataType::Float64, false),
        Field::new("is_dysbiosis_collapsed", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let traj_ids: StringArray = results.iter().map(|r| Some(format!("microbiome_{}", r.short_id))).collect();
    let kps: Float64Array = results.iter().map(|r| Some(r.gene_expression_gain_kp)).collect();
    let diffs: Float64Array = results.iter().map(|r| Some(r.diffusion_species_0)).collect();
    let growths: Float64Array = results.iter().map(|r| Some(r.growth_species_0)).collect();
    let diversities: Float64Array = results.iter().map(|r| Some(r.final_shannon_diversity_index)).collect();
    let max_fracs: Float64Array = results.iter().map(|r| Some(r.max_species_fraction)).collect();
    let collapses: BooleanArray = results.iter().map(|r| Some(r.is_dysbiosis_collapsed)).collect();
    let proofs: StringArray = results.iter().map(|r| Some(r.proof_hash.clone())).collect();

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(traj_ids),
            Arc::new(kps),
            Arc::new(diffs),
            Arc::new(growths),
            Arc::new(diversities),
            Arc::new(max_fracs),
            Arc::new(collapses),
            Arc::new(proofs),
        ],
    ).expect("Failed to create RecordBatch");

    let file = std::fs::File::create(path)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.to_string()),
            parquet::file::metadata::KeyValue::new("generator".to_string(), "G^G Microbiome FKPP Gene Expression Physics v1.0".to_string()),
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
        .unwrap_or_else(|| "../../doe-genesis/topic-1-biotechnology-revolution/data/microbiome_predictive_engineering_fkpp.parquet".to_string());

    println!("====================================================================");
    println!("  G^G KERNEL: PREDICTIVE MICROBIOME FKPP & GENE EXPRESSION SWEEP");
    println!("  Target Trajectories: {}", n_trajectories);
    println!("  Simulating Multi-Species FKPP Reaction-Diffusion & Feedback Kp...");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x4D49_4352_4F42_494F);
    let start = Instant::now();

    let mut results = Vec::with_capacity(n_trajectories as usize);
    for i in 0..n_trajectories {
        results.push(run_single_microbiome(i, &mut rng));
    }

    let proof_hashes: Vec<_> = results.iter().map(|r| r.proof_hash.clone()).collect();
    let run_proof = proof::seal_run(&proof_hashes);

    write_parquet_dataset(&out_parquet, &results, &run_proof)
        .expect("Failed to write Parquet dataset");

    let stable_count = results.iter().filter(|r| !r.is_dysbiosis_collapsed).count();
    let collapsed_count = n_trajectories as usize - stable_count;

    println!("====================================================================");
    println!("  MICROBIOME FKPP SWEEP COMPLETE");
    println!("  Total Trajectories Simulated: {}", n_trajectories);
    println!("  Stable High-Diversity Communities:  {} ({:.1}%)", stable_count, (stable_count as f64 / n_trajectories as f64) * 100.0);
    println!("  Dysbiosis Tipping Point Collapses:  {} ({:.1}%)", collapsed_count, (collapsed_count as f64 / n_trajectories as f64) * 100.0);
    println!("  Master SHA-256 Run Proof:           {}", run_proof);
    println!("  Simulation Time:                    {:?}", start.elapsed());
    println!("  Parquet Dataset Written To:          {}", out_parquet);
    println!("====================================================================\n");
}
