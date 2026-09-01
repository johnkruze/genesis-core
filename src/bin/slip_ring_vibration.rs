//! Slip-ring brush. F = m a from SDOF at deck vibe. Mass/accel mixed so F can beat preload.
//! Clock: 500 Hz, 1.5 s. Gates: on-resonance vs disconnect ≥ 8 ms.
//! Organ: DynamicOscillator, vibration_transmissibility. No walking-shock RNG.

use genesis_core::output;
use genesis_core::physics::resonance::{vibration_transmissibility, DynamicOscillator};
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
const DT: f64 = 0.002;
const HORIZON_S: f64 = 1.5;
const PRELOAD_N: f64 = 1.60;
const DISCONNECT_MS: f64 = 8.0;
const FN_HZ: f64 = 48.0;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    vibe_hz: f64,
    max_disconnect_ms: f64,
    is_on_resonance: bool,
    is_matrix_starved: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let f_exc = rng.range(20.0, 90.0);
    let g_amp = rng.range(1.2, 12.0);
    let m_brush = rng.range(0.025, 0.070);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(f_exc);
    proof.feed_f64(g_amp);
    proof.feed_f64(m_brush);

    let on_res = (f_exc - FN_HZ).abs() / FN_HZ < 0.08;
    let tr = vibration_transmissibility(f_exc, FN_HZ, 0.06);
    let mut osc = DynamicOscillator::new(FN_HZ, 0.06);
    let mut open_ms = 0.0;
    let mut max_open: f64 = 0.0;
    let steps = (HORIZON_S / DT) as usize;
    for k in 0..steps {
        let t = k as f64 * DT;
        let a_base = g_amp * 9.81 * (2.0 * std::f64::consts::PI * f_exc * t).sin();
        let (_, acc) = osc.step(a_base, DT);
        let f_sep = m_brush * acc.abs();
        if f_sep > PRELOAD_N {
            open_ms += DT * 1000.0;
            max_open = max_open.max(open_ms);
        } else {
            open_ms = 0.0;
        }
    }
    let starve = max_open >= DISCONNECT_MS;
    proof.feed_f64(tr);
    proof.feed_f64(max_open);
    proof.feed_str(if starve {
        "STARVE"
    } else if on_res {
        "ON_RES"
    } else {
        "CONTACT"
    });

    Run {
        id,
        short_id,
        vibe_hz: (f_exc * 10.0).round() / 10.0,
        max_disconnect_ms: (max_open * 10.0).round() / 10.0,
        is_on_resonance: on_res,
        is_matrix_starved: starve,
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
                "{}/../../data/exports/sovereign/slip_ring_vibration.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    println!("====================================================================");
    println!("  G^G: SLIP RING  (F=ma vs {PRELOAD_N} N, 500 Hz)");
    println!("  n={n}  disconnect {DISCONNECT_MS} ms");
    println!("====================================================================\n");
    let t0 = Instant::now();
    let mut rng = Rng::new(0x3110_77A0);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("vibe_hz", DataType::Float64, false),
        Field::new("max_disconnect_ms", DataType::Float64, false),
        Field::new("is_on_resonance", DataType::Boolean, false),
        Field::new("is_matrix_starved", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.vibe_hz).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.max_disconnect_ms).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_on_resonance).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_matrix_starved).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G slip-ring F=ma dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let nf = n as f64;
    let a = rows.iter().filter(|r| r.is_on_resonance).count();
    let b = rows.iter().filter(|r| r.is_matrix_starved).count();
    println!(
        "  on_res {a} ({:.1}%)  starve {b} ({:.1}%)",
        100.0 * a as f64 / nf,
        100.0 * b as f64 / nf
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
