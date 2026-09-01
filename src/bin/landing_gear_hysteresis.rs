//! Carrier / airframe oleo. CLASS: airframe, not UGV. Static preload 67.5 kN > 5 t share.
//! Energy in kJ (a GJ is a car-bomb). Clock: 200 Hz, 0.8 s. Mix sink 2.8–6.2 m/s.
//! Gates: bolter (second-hit mix) vs hard landing peak g≥40.
//! Stroke recorded; this orifice does not bottom 0.22 m at these sinks. Organ: OleoStrutDamper.

use genesis_core::output;
use genesis_core::physics::resonance::{OleoStrutDamper, GRAVITY};
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
const DT: f64 = 0.005;
const HORIZON_S: f64 = 0.80;
const MASS: f64 = 9_000.0; // one main-gear share of a heavy
const STROKE_LIM: f64 = 0.22;
const G_HARD: f64 = 40.0;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    sink_ms: f64,
    peak_stroke_m: f64,
    peak_g: f64,
    energy_kj: f64,
    is_bolter: bool,
    is_hard_landing: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let sink0 = rng.range(2.8, 6.2);
    let second = rng.chance(0.35); // bolter: two landings

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(sink0);
    proof.feed_f64(if second { 1.0 } else { 0.0 });

    let strut = OleoStrutDamper::default();
    let mut x = 0.02;
    let mut v = sink0;
    let mut peak_x: f64 = 0.0;
    let mut peak_g: f64 = 1.0;
    let e_kj = 0.5 * MASS * sink0 * sink0 / 1000.0;
    let mut hits = 0u8;
    let steps = (HORIZON_S / DT) as usize;
    for k in 0..steps {
        let (fg, fh) = strut.forces_n(x, v);
        let a = GRAVITY - (fg + fh) / MASS;
        v += a * DT;
        x += v * DT;
        if x < 0.0 {
            x = 0.0;
            v = 0.0;
            hits += 1;
            if second && hits == 1 {
                v = sink0 * 0.75; // second landing
            }
        }
        peak_x = peak_x.max(x);
        peak_g = peak_g.max(1.0 + a.abs() / GRAVITY);
        if peak_x >= STROKE_LIM {
            break;
        }
        if k % 20 == 0 {
            proof.feed_f64(x);
        }
    }
    let bolter = second;
    let hard = peak_g >= G_HARD;
    proof.feed_f64(peak_x);
    proof.feed_str(if hard {
        "HARD"
    } else if bolter {
        "BOLTER"
    } else {
        "OK"
    });

    Run {
        id,
        short_id,
        sink_ms: (sink0 * 100.0).round() / 100.0,
        peak_stroke_m: (peak_x * 1000.0).round() / 1000.0,
        peak_g: (peak_g * 100.0).round() / 100.0,
        energy_kj: (e_kj * 10.0).round() / 10.0,
        is_bolter: bolter,
        is_hard_landing: hard,
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
                "{}/../../data/exports/sovereign/landing_gear_hysteresis.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    println!("====================================================================");
    println!("  G^G: OLEO  (airframe, kJ not GJ, 200 Hz)");
    println!("  n={n}  stroke rec {STROKE_LIM} m  hard {G_HARD} g");
    println!("====================================================================\n");
    let t0 = Instant::now();
    let mut rng = Rng::new(0x4010_11FA);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("sink_ms", DataType::Float64, false),
        Field::new("peak_stroke_m", DataType::Float64, false),
        Field::new("peak_g", DataType::Float64, false),
        Field::new("energy_kj", DataType::Float64, false),
        Field::new("is_bolter", DataType::Boolean, false),
        Field::new("is_hard_landing", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.sink_ms).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.peak_stroke_m).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.peak_g).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.energy_kj).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_bolter).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_hard_landing).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G oleo airframe dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let nf = n as f64;
    let a = rows.iter().filter(|r| r.is_bolter).count();
    let b = rows.iter().filter(|r| r.is_hard_landing).count();
    println!(
        "  bolter {a} ({:.1}%)  hard {b} ({:.1}%)",
        100.0 * a as f64 / nf,
        100.0 * b as f64 / nf
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
