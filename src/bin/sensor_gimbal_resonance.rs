//! EO/IR gimbal. Engine N1 vs bracket f_n. Wear from cycles × transmissibility, not 200/|Δf|.
//! Frequencies mixed, not drawn into coincidence. Clock: constitutive TR + 10 Hz × 8 s.
//! Gates: coincident (|Δf|/f_n < 0.06) vs jitter ≥ 0.5 mrad.

use genesis_core::output;
use genesis_core::physics::resonance::vibration_transmissibility;
use genesis_core::proof::{self, ProofChain};
use genesis_core::rng::Rng;
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{BooleanArray, Float64Array, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::ArrowWriter;

const DEFAULT_N: usize = 2500;
const JITTER_MRAD: f64 = 0.50;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    engine_hz: f64,
    bracket_hz: f64,
    max_jitter_mrad: f64,
    is_coincident: bool,
    is_targeting_failed: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let f_eng = rng.range(28.0, 58.0);
    let f_n = rng.range(38.0, 48.0);
    let zeta = rng.range(0.03, 0.08);
    let hours = rng.range(20.0, 200.0);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(f_eng);
    proof.feed_f64(f_n);
    proof.feed_f64(hours);

    let tr = vibration_transmissibility(f_eng, f_n, zeta);
    let coincident = (f_eng - f_n).abs() / f_n < 0.06;
    // Brinell pit grows with cycles at TR. 1 hour at 40 Hz = 1.44e5 cycles.
    let cycles = hours * 3600.0 * f_eng;
    let pit_um = 4.0e-8 * cycles * (tr - 1.0).max(0.0);
    let jitter = 0.05 + pit_um * 0.008 * tr;
    let fail = jitter >= JITTER_MRAD;

    proof.feed_f64(tr);
    proof.feed_f64(jitter);
    proof.feed_str(if fail {
        "JITTER"
    } else if coincident {
        "COINCIDENT"
    } else {
        "CLEAR"
    });

    Run {
        id,
        short_id,
        engine_hz: (f_eng * 10.0).round() / 10.0,
        bracket_hz: (f_n * 10.0).round() / 10.0,
        max_jitter_mrad: (jitter * 1000.0).round() / 1000.0,
        is_coincident: coincident,
        is_targeting_failed: fail,
        proof_hash: proof.seal(),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(DEFAULT_N);
    let out = args
        .iter()
        .position(|a| a == "--parquet")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| {
            format!(
                "{}/../../data/exports/sovereign/sensor_gimbal_resonance.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    println!("====================================================================");
    println!("  G^G: GIMBAL TR  (cycles × TR, mixed N1)");
    println!("  n={n}  jitter {JITTER_MRAD} mrad");
    println!("====================================================================\n");
    let t0 = Instant::now();
    let mut rng = Rng::new(0x99A1_4400);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("engine_hz", DataType::Float64, false),
        Field::new("bracket_hz", DataType::Float64, false),
        Field::new("max_jitter_mrad", DataType::Float64, false),
        Field::new("is_coincident", DataType::Boolean, false),
        Field::new("is_targeting_failed", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.engine_hz).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.bracket_hz).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.max_jitter_mrad).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_coincident).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_targeting_failed).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G gimbal TR dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let nf = n as f64;
    let a = rows.iter().filter(|r| r.is_coincident).count();
    let b = rows.iter().filter(|r| r.is_targeting_failed).count();
    println!(
        "  coincident {a} ({:.1}%)  jitter {b} ({:.1}%)",
        100.0 * a as f64 / nf,
        100.0 * b as f64 / nf
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
