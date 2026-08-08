//! Dark Window soma product: latency dual-regime + blackout coupled to edge stress proxies.
//! Multi-frame Aegis pack of grasp/RF/fleet terminals is produced by
//! `pack_autolab_aegis_soma.py` — this binary seals the latency/cache table and a
//! true Aegis-format companion stream of those rows.

use std::time::Instant;
use sha2::Digest;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output;
use genesis_core::rng::Rng;
use serde::Serialize;
use std::sync::Arc;
use arrow::array::{Float64Array, StringArray, BooleanArray};
use arrow::record_batch::RecordBatch;
use arrow::datatypes::{Schema, Field, DataType};
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::file::properties::WriterProperties;
use parquet::basic::Compression;

#[derive(Debug, Serialize)]
struct SomaMemoryRunResult {
    id: u32,
    short_id: String,
    latency_microseconds: f64,
    dark_window_blackout_active: bool,
    blackout_from_rf_desync: bool,
    blackout_from_fleet_breach: bool,
    blackout_from_latency: bool,
    soma_state_hash: String,
    soma_memory_recall_ns: f64,
    l1_cache_hit_rate_pct: f64,
    is_neuromorphic_loop_active: bool,
    proof_hash: String,
}

fn run_single_soma_memory(id: u32, rng: &mut Rng) -> SomaMemoryRunResult {
    let short_id = output::short_id(rng);

    // Latency can exceed 20 µs (real dual-regime) — was always ≤18 before
    let latency_us = rng.range(2.0, 48.0);
    // Edge stress proxies (correlated with RF/fleet product families)
    let blackout_from_rf_desync = rng.range(0.0, 1.0) < 0.18;
    let blackout_from_fleet_breach = rng.range(0.0, 1.0) < 0.12;
    let blackout_from_latency = latency_us > 22.0;
    let dark_window = blackout_from_rf_desync || blackout_from_fleet_breach || blackout_from_latency;

    // Cache: wider range so "active" is not cache-only with always-pass latency
    let cache_hit_pct = rng.range(90.0, 99.9);
    // Recall: still model-order; pack script uses Aegis mmap for measured path when claimed
    let recall_ns = if dark_window {
        rng.range(40.0, 120.0)
    } else {
        rng.range(12.0, 45.0)
    };

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(latency_us);
    proof.feed_f64(recall_ns);
    proof.feed_f64(cache_hit_pct);
    proof.feed_str(if blackout_from_rf_desync { "RF1" } else { "RF0" });
    proof.feed_str(if blackout_from_fleet_breach { "FL1" } else { "FL0" });

    // Active: edge loop healthy under non-blackout
    let is_active = latency_us <= 20.0 && cache_hit_pct >= 95.0 && !dark_window;

    proof.feed_str(if is_active {
        "SOMA_MEMORY_RECALL_PASSED"
    } else if dark_window {
        "DARK_WINDOW_BLACKOUT"
    } else {
        "LATENCY_OR_CACHE_FAIL"
    });

    let final_proof = proof.seal();
    let soma_hash = format!("0xsoma_{}", &final_proof[..16]);

    SomaMemoryRunResult {
        id,
        short_id,
        latency_microseconds: latency_us,
        dark_window_blackout_active: dark_window,
        blackout_from_rf_desync,
        blackout_from_fleet_breach,
        blackout_from_latency,
        soma_state_hash: soma_hash,
        soma_memory_recall_ns: recall_ns,
        l1_cache_hit_rate_pct: cache_hit_pct,
        is_neuromorphic_loop_active: is_active,
        proof_hash: final_proof,
    }
}

fn write_parquet_dataset(path: &str, results: &[SomaMemoryRunResult], run_proof: &str) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::Utf8, false),
        Field::new("latency_microseconds", DataType::Float64, false),
        Field::new("dark_window_blackout_active", DataType::Boolean, false),
        Field::new("blackout_from_rf_desync", DataType::Boolean, false),
        Field::new("blackout_from_fleet_breach", DataType::Boolean, false),
        Field::new("blackout_from_latency", DataType::Boolean, false),
        Field::new("soma_state_hash", DataType::Utf8, false),
        Field::new("soma_memory_recall_ns", DataType::Float64, false),
        Field::new("l1_cache_hit_rate_pct", DataType::Float64, false),
        Field::new("is_neuromorphic_loop_active", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let ids: StringArray = results.iter().map(|r| Some(format!("soma_{}", r.short_id))).collect();
    let lats: Float64Array = results.iter().map(|r| Some(r.latency_microseconds)).collect();
    let blackouts: BooleanArray = results.iter().map(|r| Some(r.dark_window_blackout_active)).collect();
    let br: BooleanArray = results.iter().map(|r| Some(r.blackout_from_rf_desync)).collect();
    let bf: BooleanArray = results.iter().map(|r| Some(r.blackout_from_fleet_breach)).collect();
    let bl: BooleanArray = results.iter().map(|r| Some(r.blackout_from_latency)).collect();
    let hashes: StringArray = results.iter().map(|r| Some(r.soma_state_hash.clone())).collect();
    let recalls: Float64Array = results.iter().map(|r| Some(r.soma_memory_recall_ns)).collect();
    let hits: Float64Array = results.iter().map(|r| Some(r.l1_cache_hit_rate_pct)).collect();
    let actives: BooleanArray = results.iter().map(|r| Some(r.is_neuromorphic_loop_active)).collect();
    let proofs: StringArray = results.iter().map(|r| Some(r.proof_hash.clone())).collect();

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(ids),
            Arc::new(lats),
            Arc::new(blackouts),
            Arc::new(br),
            Arc::new(bf),
            Arc::new(bl),
            Arc::new(hashes),
            Arc::new(recalls),
            Arc::new(hits),
            Arc::new(actives),
            Arc::new(proofs),
        ],
    )
    .expect("batch");

    let file = std::fs::File::create(path)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.to_string()),
            parquet::file::metadata::KeyValue::new(
                "generator".to_string(),
                "G^G Soma Binary Memory Edge Control v1.1 coupled-blackout".to_string(),
            ),
        ]))
        .build();

    let mut writer = ArrowWriter::try_new(file, schema, Some(props)).expect("writer");
    writer.write(&batch).expect("write");
    writer.close().expect("close");
    Ok(())
}

/// Aegis FRAME_FORMAT <d6f2fQ16s> pack of soma rows (latency/cache as invariants).
fn write_aegis_soma_bin(path: &str, results: &[SomaMemoryRunResult], run_proof: &str) -> std::io::Result<()> {
    let mut bin = Vec::with_capacity(64 + results.len() * 64);
    // HEADER <4sHHQQ32s8s>: magic, ver, body_id, traj_count, frame_count, proof32, reserved8
    bin.extend_from_slice(b"SOMA");
    bin.extend_from_slice(&1u16.to_le_bytes()); // SPEC_VERSION
    bin.extend_from_slice(&27u16.to_le_bytes()); // body_id autolab=27 (see BODY_TYPE_MAP)
    bin.extend_from_slice(&(1u64).to_le_bytes()); // traj_count
    bin.extend_from_slice(&(results.len() as u64).to_le_bytes());
    let digest = sha2::Sha256::digest(run_proof.as_bytes());
    bin.extend_from_slice(&digest);
    bin.extend_from_slice(b"AUTOLAB1"); // 8 B reserved
    assert_eq!(bin.len(), 64);

    for (i, r) in results.iter().enumerate() {
        let t = i as f64 * 0.001;
        let pos = [r.latency_microseconds, r.soma_memory_recall_ns, r.l1_cache_hit_rate_pct];
        let vel = [
            if r.blackout_from_rf_desync { 1.0 } else { 0.0 },
            if r.blackout_from_fleet_breach { 1.0 } else { 0.0 },
            if r.blackout_from_latency { 1.0 } else { 0.0 },
        ];
        let force = r.latency_microseconds as f32;
        let residual = r.l1_cache_hit_rate_pct as f32;
        let flags: u64 = if r.dark_window_blackout_active { 1 } else { 0 }
            | if r.is_neuromorphic_loop_active { 2 } else { 0 };
        let ph = sha2::Sha256::digest(r.proof_hash.as_bytes());
        let proof16 = &ph[..16];

        bin.extend_from_slice(&t.to_le_bytes());
        for v in &pos {
            bin.extend_from_slice(&(*v as f32).to_le_bytes());
        }
        for v in &vel {
            bin.extend_from_slice(&(*v as f32).to_le_bytes());
        }
        bin.extend_from_slice(&force.to_le_bytes());
        bin.extend_from_slice(&residual.to_le_bytes());
        bin.extend_from_slice(&flags.to_le_bytes());
        bin.extend_from_slice(proof16);
    }
    assert_eq!(bin.len(), 64 + results.len() * 64);
    std::fs::write(path, bin)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n_trajectories: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1_000);

    let out_parquet = args
        .iter()
        .position(|a| a == "--parquet")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| {
            "../../doe-genesis/topic-4-autonomous-laboratories/data/autolab_soma_memory_dark_window.parquet"
                .to_string()
        });

    println!("====================================================================");
    println!("  G^G KERNEL: DARK WINDOW SOMA MEMORY (v1.1 coupled blackout)");
    println!("  Target Trajectories: {}", n_trajectories);
    println!("====================================================================\n");

    let mut rng = Rng::new(0x534f_4d41_5f4d454d);
    let start = Instant::now();

    let mut results = Vec::with_capacity(n_trajectories as usize);
    for i in 0..n_trajectories {
        results.push(run_single_soma_memory(i, &mut rng));
    }

    let proof_hashes: Vec<_> = results.iter().map(|r| r.proof_hash.clone()).collect();
    let run_proof = proof::seal_run(&proof_hashes);

    write_parquet_dataset(&out_parquet, &results, &run_proof).expect("parquet");

    let soma_bin = "../../doe-genesis/topic-4-autonomous-laboratories/data/autolab_dark_window.soma.bin";
    write_aegis_soma_bin(soma_bin, &results, &run_proof).expect("soma.bin");

    let active = results.iter().filter(|r| r.is_neuromorphic_loop_active).count();
    let blackout = results.iter().filter(|r| r.dark_window_blackout_active).count();

    println!("====================================================================");
    println!("  SOMA SWEEP COMPLETE");
    println!("  Active neuromorphic loop:  {} ({:.1}%)", active, 100.0 * active as f64 / n_trajectories as f64);
    println!("  Dark Window blackout:      {} ({:.1}%)", blackout, 100.0 * blackout as f64 / n_trajectories as f64);
    println!("  Master proof:              {}", run_proof);
    println!("  Parquet:                   {}", out_parquet);
    println!("  Aegis .soma.bin:           {}", soma_bin);
    println!("  Time:                      {:?}", start.elapsed());
    println!("====================================================================\n");
}
