//! UGV washboard. ω_n = √(k/m) ≈ 1.04 Hz. Mix on-resonance vs off.
//! Relative damper (ground − chassis). Clock: 100 Hz, 6 s.
//! Gates: on-resonance vs torsion yield. Organ: DynamicOscillator.

use genesis_core::output;
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
const HORIZON_S: f64 = 6.0;
const M: f64 = 35_000.0;
const K: f64 = 1.5e6;
const YIELD_MPA: f64 = 900.0;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    speed_ms: f64,
    max_stress_mpa: f64,
    is_on_resonance: bool,
    is_torsion_bar_snapped: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let lam = rng.range(0.6, 1.8);
    let amp = rng.range(0.04, 0.12);
    let fn_hz = (K / M).sqrt() / (2.0 * std::f64::consts::PI);
    let v_res = fn_hz * lam;
    let on_res = rng.chance(0.32);
    let speed = if on_res {
        v_res * rng.range(0.96, 1.04)
    } else {
        rng.range(2.0, 14.0)
    };

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(lam);
    proof.feed_f64(speed);

    let zeta = 0.12;
    let mut osc = DynamicOscillator::new(fn_hz, zeta);
    let f_exc = speed / lam;
    let mut peak: f64 = 0.0;
    let steps = (HORIZON_S / DT) as usize;
    for k in 0..steps {
        let t = k as f64 * DT;
        // Base excitation accel ~ ω² amp sin(ωt)
        let w = 2.0 * std::f64::consts::PI * f_exc;
        let a_base = (w * w) * amp * (w * t).sin();
        let (y, _) = osc.step(a_base, DT);
        peak = peak.max(y.abs());
    }
    // Stress from spring force through a 40 mm bar, 0.25 m arm — sized to carry 35 t off-resonance.
    let f_n = K * peak;
    let r: f64 = 0.055;
    let j = std::f64::consts::PI * r.powi(4) / 2.0;
    let stress = (f_n * 0.18 * r) / j / 1e6;
    let yield_hit = stress >= YIELD_MPA;

    proof.feed_f64(stress);
    proof.feed_str(if yield_hit {
        "YIELD"
    } else if on_res {
        "ON_RES"
    } else {
        "OFF_RES"
    });

    Run {
        id,
        short_id,
        speed_ms: (speed * 100.0).round() / 100.0,
        max_stress_mpa: (stress * 10.0).round() / 10.0,
        is_on_resonance: on_res,
        is_torsion_bar_snapped: yield_hit,
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
                "{}/../../data/exports/sovereign/suspension_resonance.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    println!("====================================================================");
    println!("  G^G: WASHBOARD  (ω_n=√(k/m), 100 Hz)");
    println!("  n={n}  yield {YIELD_MPA} MPa");
    println!("====================================================================\n");
    let t0 = Instant::now();
    let mut rng = Rng::new(0x5059_0011);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("speed_ms", DataType::Float64, false),
        Field::new("max_stress_mpa", DataType::Float64, false),
        Field::new("is_on_resonance", DataType::Boolean, false),
        Field::new("is_torsion_bar_snapped", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.speed_ms).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.max_stress_mpa).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_on_resonance).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_torsion_bar_snapped).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G washboard SDOF dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let nf = n as f64;
    let a = rows.iter().filter(|r| r.is_on_resonance).count();
    let b = rows.iter().filter(|r| r.is_torsion_bar_snapped).count();
    println!(
        "  on_res {a} ({:.1}%)  yield {b} ({:.1}%)",
        100.0 * a as f64 / nf,
        100.0 * b as f64 / nf
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
