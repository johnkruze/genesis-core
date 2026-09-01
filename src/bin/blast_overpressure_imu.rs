//! Friedlander P(t) on the hull. Hopkinson–Cranz P_so. Clip while P(t) A / m > 40 g.
//! Cov grows only during clip. Clock: 2 kHz, 20 ms positive phase. Mix R and W.
//! Gates: P_so ≥ 25 kPa overpressure vs nav clip ≥ 2 ms.

use genesis_core::output;
use genesis_core::physics::resonance::{
    friedlander_blast_overpressure_pa, hopkinson_cranz_peak_overpressure_kpa,
};
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
const DT: f64 = 5e-4;
const T_POS: f64 = 0.012;
const MASS: f64 = 5_000.0;
const AREA: f64 = 12.0;
const CLIP_G: f64 = 5.0;
const P_WARN_KPA: f64 = 25.0;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    tnt_kg: f64,
    range_m: f64,
    peak_g: f64,
    clip_ms: f64,
    is_overpressure: bool,
    is_ekf_diverged: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let w = rng.range(1.5, 18.0);
    let r = rng.range(10.0, 45.0);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(w);
    proof.feed_f64(r);

    let p_kpa = hopkinson_cranz_peak_overpressure_kpa(r, w);
    let p0 = p_kpa * 1000.0;
    let mut peak_g: f64 = 0.0;
    let mut clip_s = 0.0;
    let steps = (T_POS / DT) as usize;
    for k in 0..steps {
        let t = k as f64 * DT;
        let p = friedlander_blast_overpressure_pa(p0, T_POS, t);
        let a_g = (p * AREA / MASS) / 9.81;
        peak_g = peak_g.max(a_g);
        if a_g >= CLIP_G {
            clip_s += DT;
        }
        if k % 4 == 0 {
            proof.feed_f64(p);
        }
    }
    // Nav fail: clip duration, not a one-shot g. σ grows ~ (40 g) × t_clip.
    let cov_m = 400.0 * clip_s;
    let over = p_kpa >= P_WARN_KPA;
    let nav = clip_s >= 0.002;
    proof.feed_f64(peak_g);
    proof.feed_f64(clip_s);
    proof.feed_str(if nav {
        "NAV_FAIL"
    } else if over {
        "OVERPRESSURE"
    } else {
        "OK"
    });

    Run {
        id,
        short_id,
        tnt_kg: (w * 10.0).round() / 10.0,
        range_m: (r * 10.0).round() / 10.0,
        peak_g: (peak_g * 10.0).round() / 10.0,
        clip_ms: (clip_s * 1e3 * 10.0).round() / 10.0,
        is_overpressure: over,
        is_ekf_diverged: nav,
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
                "{}/../../data/exports/sovereign/blast_overpressure_imu.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    println!("====================================================================");
    println!("  G^G: FRIEDLANDER IMU  (P(t), clip duration, 2 kHz)");
    println!("  n={n}  P_so {P_WARN_KPA} kPa  nav clip-cov");
    println!("====================================================================\n");
    let t0 = Instant::now();
    let mut rng = Rng::new(0x3011_99AF);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("tnt_kg", DataType::Float64, false),
        Field::new("range_m", DataType::Float64, false),
        Field::new("peak_g", DataType::Float64, false),
        Field::new("clip_ms", DataType::Float64, false),
        Field::new("is_overpressure", DataType::Boolean, false),
        Field::new("is_ekf_diverged", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.tnt_kg).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.range_m).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.peak_g).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.clip_ms).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_overpressure).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_ekf_diverged).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G Friedlander IMU dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let nf = n as f64;
    let a = rows.iter().filter(|r| r.is_overpressure).count();
    let b = rows.iter().filter(|r| r.is_ekf_diverged).count();
    println!(
        "  overpressure {a} ({:.1}%)  nav_fail {b} ({:.1}%)",
        100.0 * a as f64 / nf,
        100.0 * b as f64 / nf
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
