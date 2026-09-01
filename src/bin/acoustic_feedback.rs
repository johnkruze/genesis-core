//! Vertiport ground-effect acoustics. Image-source pressure, then SDOF at blade-pass.
//! Mix abort above deck vs continue into GE. Clock: 100 Hz, 8 s. Not 15 s × 1000 Hz of nothing.
//! Gates: on-resonance vs |θ| ≥ 0.16 rad diverge.

use genesis_core::output;
use genesis_core::physics::optics::image_source_pressure_ratio;
use genesis_core::physics::resonance::DynamicOscillator;
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
const DT: f64 = 0.01;
const HORIZON_S: f64 = 8.0;
const BLADE_HZ: f64 = 22.0;
const REF_H: f64 = 2.2;
const DIVERGE: f64 = 0.16;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    structural_freq_hz: f64,
    max_pitch_rad: f64,
    is_on_resonance: bool,
    is_fbw_diverged: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let on_res = rng.chance(0.34);
    let fn_hz = if on_res {
        BLADE_HZ * rng.range(0.96, 1.04)
    } else {
        rng.range(12.0, 34.0)
    };
    let into_ge = rng.chance(0.58);
    let floor = if into_ge { 0.4 } else { 6.2 };

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(fn_hz);
    proof.feed_f64(floor);

    let mut osc = DynamicOscillator::new(fn_hz, 0.07);
    let mut h = 10.0;
    let vz = 1.15;
    let mut peak: f64 = 0.0;
    let steps = (HORIZON_S / DT) as usize;
    for k in 0..steps {
        if h <= floor {
            break;
        }
        h -= vz * DT;
        let p = image_source_pressure_ratio(h, REF_H);
        let t = k as f64 * DT;
        let a = (p - 1.0) * 5.5 * (2.0 * std::f64::consts::PI * BLADE_HZ * t).sin();
        let (th, _) = osc.step(a, DT);
        peak = peak.max(th.abs());
        if k % 20 == 0 {
            proof.feed_f64(th);
        }
    }
    let diverge = peak >= DIVERGE;

    proof.feed_f64(peak);
    proof.feed_str(if diverge {
        "DIVERGED"
    } else if on_res {
        "ON_RES"
    } else {
        "HELD"
    });

    Run {
        id,
        short_id,
        structural_freq_hz: (fn_hz * 10.0).round() / 10.0,
        max_pitch_rad: (peak * 1000.0).round() / 1000.0,
        is_on_resonance: on_res,
        is_fbw_diverged: diverge,
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
                "{}/../../data/exports/sovereign/acoustic_feedback.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    println!("====================================================================");
    println!("  G^G: VERTIPORT GE  (image-source, SDOF, 100 Hz)");
    println!("  n={n}");
    println!("====================================================================\n");
    let t0 = Instant::now();
    let mut rng = Rng::new(0x1928_3746);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("structural_freq_hz", DataType::Float64, false),
        Field::new("max_pitch_rad", DataType::Float64, false),
        Field::new("is_on_resonance", DataType::Boolean, false),
        Field::new("is_fbw_diverged", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.structural_freq_hz).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.max_pitch_rad).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_on_resonance).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_fbw_diverged).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G vertiport GE dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let nf = n as f64;
    let a = rows.iter().filter(|r| r.is_on_resonance).count();
    let b = rows.iter().filter(|r| r.is_fbw_diverged).count();
    println!(
        "  on_res {a} ({:.1}%)  diverge {b} ({:.1}%)",
        100.0 * a as f64 / nf,
        100.0 * b as f64 / nf
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
