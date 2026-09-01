use std::time::Instant;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output;
use genesis_core::rng::Rng;
use genesis_core::physics::genome::{GenomeEditState, GenomeEngineeringParams};
use serde::Serialize;
use std::sync::Arc;
use arrow::array::{Float64Array, StringArray, BooleanArray, UInt64Array};
use arrow::record_batch::RecordBatch;
use arrow::datatypes::{Schema, Field, DataType};
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::file::properties::WriterProperties;
use parquet::basic::Compression;

#[derive(Debug, Serialize)]
struct GenomeRunResult {
    id: u32,
    short_id: String,
    target_edit_count: u64,
    guide_rna_affinity_dg: f64,
    chromatin_torsional_strain_pa: f64,
    off_target_cleavage_count: u64,
    cellular_repair_energy_kj: f64,
    genomic_integrity_pct: f64,
    is_chromosomal_translocation: bool,
    proof_hash: String,
}

fn run_single_genome(
    id: u32,
    rng: &mut Rng,
) -> GenomeRunResult {
    let short_id = output::short_id(rng);
    
    // Sweep simultaneous edit density (3 to 75 edits) and gRNA affinity (-25 to -5 kJ/mol)
    let edit_count = rng.range(3.0, 75.0) as usize;
    let affinity_dg = rng.range(-25.0, -5.0);

    let params = GenomeEngineeringParams {
        max_repair_energy_budget_kj: 2500.0,
        ..Default::default()
    };
    let mut state = GenomeEditState::new(edit_count, affinity_dg);
    state.cellular_repair_energy_kj = 2500.0;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(edit_count as f64);
    proof.feed_f64(affinity_dg);

    let dt_sec = 0.5; // 0.5 sec editing timestep
    let total_steps = 80;

    for step in 0..total_steps {
        state.step(&params, dt_sec);

        if step % 20 == 0 {
            proof.feed_f64(state.chromatin_torsional_strain_pa);
            proof.feed_f64(state.genomic_integrity_pct);
        }
    }

    proof.feed_f64(state.genomic_integrity_pct);
    proof.feed_str(if state.is_chromosomal_translocation { "TRANSLOCATION_DAMAGED" } else { "STABLE_EDIT_PASSED" });

    GenomeRunResult {
        id,
        short_id,
        target_edit_count: edit_count as u64,
        guide_rna_affinity_dg: affinity_dg,
        chromatin_torsional_strain_pa: state.chromatin_torsional_strain_pa,
        off_target_cleavage_count: state.off_target_cleavage_count as u64,
        cellular_repair_energy_kj: state.cellular_repair_energy_kj,
        genomic_integrity_pct: state.genomic_integrity_pct,
        is_chromosomal_translocation: state.is_chromosomal_translocation,
        proof_hash: proof.seal(),
    }
}

fn write_parquet_dataset(path: &str, results: &[GenomeRunResult], run_proof: &str) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::Utf8, false),
        Field::new("target_edit_count", DataType::UInt64, false),
        Field::new("guide_rna_affinity_dg", DataType::Float64, false),
        Field::new("chromatin_torsional_strain_pa", DataType::Float64, false),
        Field::new("off_target_cleavage_count", DataType::UInt64, false),
        Field::new("cellular_repair_energy_kj", DataType::Float64, false),
        Field::new("genomic_integrity_pct", DataType::Float64, false),
        Field::new("is_chromosomal_translocation", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let traj_ids: StringArray = results.iter().map(|r| Some(format!("genome_{}", r.short_id))).collect();
    let edits: UInt64Array = results.iter().map(|r| Some(r.target_edit_count)).collect();
    let affinities: Float64Array = results.iter().map(|r| Some(r.guide_rna_affinity_dg)).collect();
    let strains: Float64Array = results.iter().map(|r| Some(r.chromatin_torsional_strain_pa)).collect();
    let off_targets: UInt64Array = results.iter().map(|r| Some(r.off_target_cleavage_count)).collect();
    let energies: Float64Array = results.iter().map(|r| Some(r.cellular_repair_energy_kj)).collect();
    let integrities: Float64Array = results.iter().map(|r| Some(r.genomic_integrity_pct)).collect();
    let translocations: BooleanArray = results.iter().map(|r| Some(r.is_chromosomal_translocation)).collect();
    let proofs: StringArray = results.iter().map(|r| Some(r.proof_hash.clone())).collect();

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(traj_ids),
            Arc::new(edits),
            Arc::new(affinities),
            Arc::new(strains),
            Arc::new(off_targets),
            Arc::new(energies),
            Arc::new(integrities),
            Arc::new(translocations),
            Arc::new(proofs),
        ],
    ).expect("Failed to create RecordBatch");

    let file = std::fs::File::create(path)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.to_string()),
            parquet::file::metadata::KeyValue::new("generator".to_string(), "G^G Chromatin Strain Genome Engineering v1.0".to_string()),
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
        .unwrap_or_else(|| "../../doe-genesis/topic-1-biotechnology-revolution/data/genome_engineering_multiplex_edits.parquet".to_string());

    println!("====================================================================");
    println!("  G^G KERNEL: MULTIPLEXED GENOME ENGINEERING & CHROMATIN SWEEP");
    println!("  Target Trajectories: {}", n_trajectories);
    println!("  Simulating 3D Supercoil Strain, gRNA Affinity & Off-Target Breaks...");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x4745_4E4F_4D45_4544);
    let start = Instant::now();

    let mut results = Vec::with_capacity(n_trajectories as usize);
    for i in 0..n_trajectories {
        results.push(run_single_genome(i, &mut rng));
    }

    let proof_hashes: Vec<_> = results.iter().map(|r| r.proof_hash.clone()).collect();
    let run_proof = proof::seal_run(&proof_hashes);

    write_parquet_dataset(&out_parquet, &results, &run_proof)
        .expect("Failed to write Parquet dataset");

    let stable_runs = results.iter().filter(|r| !r.is_chromosomal_translocation).count();
    let damaged_runs = n_trajectories as usize - stable_runs;

    println!("====================================================================");
    println!("  GENOME ENGINEERING SWEEP COMPLETE");
    println!("  Total Trajectories Simulated: {}", n_trajectories);
    println!("  Stable Multiplexed Edit Runs:       {} ({:.1}%)", stable_runs, (stable_runs as f64 / n_trajectories as f64) * 100.0);
    println!("  Chromosomal Translocation Events:   {} ({:.1}%)", damaged_runs, (damaged_runs as f64 / n_trajectories as f64) * 100.0);
    println!("  Master SHA-256 Run Proof:           {}", run_proof);
    println!("  Simulation Time:                    {:?}", start.elapsed());
    println!("  Parquet Dataset Written To:          {}", out_parquet);
    println!("====================================================================\n");
}
