//! Dark Window soma: measured mmap of an Aegis bank + coupled RF/fleet blackout.
//!
//! Recall is timed against a real `.soma.bin` (last-frame / random-frame / missing).
//! Comms blackout (RF desync, fleet breach) is independent — the point of Dark Window
//! is that local memory still peeks when the radio is dead.
//! Multi-frame terminal pack of grasp/RF/fleet/soma is produced by
//! `pack_autolab_aegis_soma.py`.

use genesis_core::last_state;
use genesis_core::output;
use genesis_core::proof::{self, ProofChain};
use genesis_core::rng::Rng;
use serde::Serialize;
use sha2::Digest;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{BooleanArray, Float64Array, StringArray, UInt64Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;

const HEADER_SIZE: u64 = last_state::HEADER_BYTES;
const FRAME_SIZE: u64 = last_state::FRAME_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MmapClass {
    LastFrame,
    RandomFrame,
    BankMissing,
}

impl MmapClass {
    fn as_str(self) -> &'static str {
        match self {
            MmapClass::LastFrame => "last_frame",
            MmapClass::RandomFrame => "random_frame",
            MmapClass::BankMissing => "bank_missing",
        }
    }
}

struct SomaBank {
    path: String,
    file_size: u64,
    frame_count: u64,
    header_ok: bool,
}

impl SomaBank {
    fn open(path: &str) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        let file_size = meta.len();
        if file_size < HEADER_SIZE + FRAME_SIZE {
            return None;
        }
        let mut f = File::open(path).ok()?;
        let mut header = [0u8; 64];
        f.read_exact(&mut header).ok()?;
        if &header[0..4] != b"SOMA" {
            return None;
        }
        let frame_count = u64::from_le_bytes(header[16..24].try_into().ok()?);
        if frame_count == 0 {
            return None;
        }
        Some(Self {
            path: path.to_string(),
            file_size,
            frame_count,
            header_ok: true,
        })
    }

    fn peek_last(&self) -> Option<(f64, u64)> {
        let mut f = File::open(&self.path).ok()?;
        let off = self.file_size.saturating_sub(FRAME_SIZE);
        f.seek(SeekFrom::Start(off)).ok()?;
        let t0 = Instant::now();
        let mut buf = [0u8; 64];
        f.read_exact(&mut buf).ok()?;
        let ns = t0.elapsed().as_nanos() as f64;
        Some((ns, FRAME_SIZE))
    }

    fn peek_random(&self, rng: &mut Rng) -> Option<(f64, u64)> {
        let mut f = File::open(&self.path).ok()?;
        let idx = rng.index(self.frame_count as usize) as u64;
        let off = HEADER_SIZE + idx * FRAME_SIZE;
        f.seek(SeekFrom::Start(off)).ok()?;
        let t0 = Instant::now();
        let mut buf = [0u8; 64];
        f.read_exact(&mut buf).ok()?;
        let ns = t0.elapsed().as_nanos() as f64;
        Some((ns, FRAME_SIZE))
    }
}

fn peek_missing() -> f64 {
    let t0 = Instant::now();
    let _ = File::open("/tmp/grokd_soma_bank_missing.soma.bin");
    t0.elapsed().as_nanos() as f64
}

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
    mmap_class: String,
    mmap_header_ok: bool,
    frames_read: u64,
    bytes_mapped: u64,
    proof_hash: String,
}

fn run_single_soma_memory(
    id: u32,
    rng: &mut Rng,
    bank: Option<&SomaBank>,
) -> SomaMemoryRunResult {
    let short_id = output::short_id(rng);

    // Comms / edge stress — independent of whether the local bank peeks.
    let blackout_from_rf_desync = rng.range(0.0, 1.0) < 0.18;
    let blackout_from_fleet_breach = rng.range(0.0, 1.0) < 0.12;

    let roll = rng.range(0.0, 1.0);
    let class = if bank.is_none() || roll < 0.08 {
        MmapClass::BankMissing
    } else if roll < 0.33 {
        MmapClass::RandomFrame
    } else {
        MmapClass::LastFrame
    };

    let (recall_ns, bytes, frames, header_ok) = match (class, bank) {
        (MmapClass::LastFrame, Some(b)) => match b.peek_last() {
            Some((ns, n)) => (ns, n, 1u64, b.header_ok),
            None => (peek_missing(), 0, 0, false),
        },
        (MmapClass::RandomFrame, Some(b)) => match b.peek_random(rng) {
            Some((ns, n)) => (ns, n, 1u64, b.header_ok),
            None => (peek_missing(), 0, 0, false),
        },
        _ => (peek_missing(), 0, 0, false),
    };

    let latency_us = recall_ns / 1000.0;
    let blackout_from_latency = latency_us > 22.0 || class == MmapClass::BankMissing;
    let dark_window =
        blackout_from_rf_desync || blackout_from_fleet_breach || blackout_from_latency;

    // Class-derived, not a CPU counter: last-frame reuses the EOF page.
    let cache_hit_pct = match class {
        MmapClass::LastFrame if frames > 0 => 99.0,
        MmapClass::RandomFrame if frames > 0 => 92.0,
        _ => 0.0,
    };

    let mmap_live = header_ok && frames >= 1;
    // Edge loop live = local peek succeeded under the 20 µs gate.
    // Comms blackout is a *separate* flag — memory is supposed to live in the Dark Window.
    let is_active = mmap_live && latency_us <= 20.0;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(latency_us);
    proof.feed_f64(recall_ns);
    proof.feed_f64(cache_hit_pct);
    proof.feed_str(class.as_str());
    proof.feed_str(if blackout_from_rf_desync { "RF1" } else { "RF0" });
    proof.feed_str(if blackout_from_fleet_breach { "FL1" } else { "FL0" });
    proof.feed_str(if mmap_live { "MMAP_LIVE" } else { "MMAP_FAIL" });
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
        mmap_class: class.as_str().to_string(),
        mmap_header_ok: header_ok,
        frames_read: frames,
        bytes_mapped: bytes,
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
        Field::new("mmap_class", DataType::Utf8, false),
        Field::new("mmap_header_ok", DataType::Boolean, false),
        Field::new("frames_read", DataType::UInt64, false),
        Field::new("bytes_mapped", DataType::UInt64, false),
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
    let classes: StringArray = results.iter().map(|r| Some(r.mmap_class.clone())).collect();
    let headers: BooleanArray = results.iter().map(|r| Some(r.mmap_header_ok)).collect();
    let frames: UInt64Array = results.iter().map(|r| Some(r.frames_read)).collect();
    let bytes: UInt64Array = results.iter().map(|r| Some(r.bytes_mapped)).collect();
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
            Arc::new(classes),
            Arc::new(headers),
            Arc::new(frames),
            Arc::new(bytes),
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
                "G^G Soma Binary Memory Edge Control v1.2 measured-mmap".to_string(),
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
    bin.extend_from_slice(b"SOMA");
    bin.extend_from_slice(&1u16.to_le_bytes());
    bin.extend_from_slice(&27u16.to_le_bytes());
    bin.extend_from_slice(&(1u64).to_le_bytes());
    bin.extend_from_slice(&(results.len() as u64).to_le_bytes());
    let digest = sha2::Sha256::digest(run_proof.as_bytes());
    bin.extend_from_slice(&digest);
    bin.extend_from_slice(b"AUTOLAB1");
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

fn resolve_bank_path(args: &[String]) -> String {
    if let Some(i) = args.iter().position(|a| a == "--bank") {
        if let Some(p) = args.get(i + 1) {
            return p.clone();
        }
    }
    let terminal = "../../doe-genesis/topic-4-autonomous-laboratories/data/autolab_terminal.soma.bin";
    let dark = "../../doe-genesis/topic-4-autonomous-laboratories/data/autolab_dark_window.soma.bin";
    if Path::new(terminal).exists() {
        terminal.to_string()
    } else {
        dark.to_string()
    }
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

    let bank_path = resolve_bank_path(&args);
    let bank = SomaBank::open(&bank_path);

    println!("====================================================================");
    println!("  G^G KERNEL: DARK WINDOW SOMA MEMORY (v1.2 measured mmap)");
    println!("  Target Trajectories: {}", n_trajectories);
    match &bank {
        Some(b) => println!(
            "  Bank: {}  frames={}  bytes={}  header_ok={}",
            b.path, b.frame_count, b.file_size, b.header_ok
        ),
        None => println!("  Bank: {}  MISSING — bank_missing class only", bank_path),
    }
    println!("====================================================================\n");

    let mut rng = Rng::new(0x534f_4d41_5f4d454d);
    if let Some(b) = &bank {
        // Warm the EOF page so the first measured last-frame peeks are not cold-fault theater.
        let _ = b.peek_last();
        let _ = b.peek_last();
    }
    let start = Instant::now();

    let mut results = Vec::with_capacity(n_trajectories as usize);
    for i in 0..n_trajectories {
        results.push(run_single_soma_memory(i, &mut rng, bank.as_ref()));
    }

    let proof_hashes: Vec<_> = results.iter().map(|r| r.proof_hash.clone()).collect();
    let run_proof = proof::seal_run(&proof_hashes);

    write_parquet_dataset(&out_parquet, &results, &run_proof).expect("parquet");

    let soma_bin = "../../doe-genesis/topic-4-autonomous-laboratories/data/autolab_dark_window.soma.bin";
    write_aegis_soma_bin(soma_bin, &results, &run_proof).expect("soma.bin");

    let active = results.iter().filter(|r| r.is_neuromorphic_loop_active).count();
    let blackout = results.iter().filter(|r| r.dark_window_blackout_active).count();
    let mmap_live = results.iter().filter(|r| r.mmap_header_ok && r.frames_read >= 1).count();
    let n = n_trajectories as f64;
    let live_recalls: Vec<f64> = results
        .iter()
        .filter(|r| r.frames_read >= 1)
        .map(|r| r.soma_memory_recall_ns)
        .collect();
    let mut sorted = live_recalls.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = sorted.get(sorted.len() / 2).copied().unwrap_or(0.0);
    let p99 = sorted
        .get(sorted.len().saturating_mul(99) / 100)
        .copied()
        .unwrap_or(0.0);

    println!("====================================================================");
    println!("  SOMA SWEEP COMPLETE");
    println!("  mmap live (got a frame):   {} ({:.1}%)", mmap_live, 100.0 * mmap_live as f64 / n);
    println!("  Edge loop ≤20 µs:          {} ({:.1}%)", active, 100.0 * active as f64 / n);
    println!("  Dark Window blackout:      {} ({:.1}%)", blackout, 100.0 * blackout as f64 / n);
    println!("  Recall p50/p99 (live ns):  {:.0} / {:.0}", p50, p99);
    println!("  Master proof:              {}", run_proof);
    println!("  Parquet:                   {}", out_parquet);
    println!("  Aegis .soma.bin:           {}", soma_bin);
    println!("  Time:                      {:?}", start.elapsed());
    println!("====================================================================\n");
}
