//! Empty 53-ft box. q A C_D yaw about the kingpin. Dryden gust, not rng/ms.
//! Clock: 50 Hz, 12 s. Gates: yaw ≥ 0.12 rad whip vs ≥ 0.28 rad jackknife.
//! Organ: DynamicOscillator, dryden_gust_step.

use genesis_core::output;
use genesis_core::physics::resonance::{dryden_gust_step, DynamicOscillator};
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
const DT: f64 = 0.02;
const HORIZON_S: f64 = 12.0;
const AREA: f64 = 43.0;
const MASS: f64 = 6_000.0;
const LEN: f64 = 16.0;
const CD: f64 = 1.1;
const RHO: f64 = 1.225;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    mean_wind_ms: f64,
    max_yaw_rad: f64,
    is_yaw_whip: bool,
    is_jackknife: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let mean = rng.range(4.0, 16.0);
    let sigma = rng.range(1.5, 5.0);
    let tau = rng.range(1.5, 4.0);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(mean);
    proof.feed_f64(sigma);

    let i_yaw = MASS * LEN * LEN / 12.0;
    let k_king = 4.0e4; // tractor exists
    let fn_hz = (k_king / i_yaw).sqrt() / (2.0 * std::f64::consts::PI);
    let mut osc = DynamicOscillator::new(fn_hz.max(0.15), 0.18);
    let mut gust = 0.0;
    let mut peak: f64 = 0.0;
    let arm = 8.0;
    let steps = (HORIZON_S / DT) as usize;
    for k in 0..steps {
        let white = rng.gaussian(0.0, 1.0);
        gust = dryden_gust_step(gust, white, tau, sigma, DT);
        let v = mean + gust;
        let f = 0.5 * RHO * v * v.abs() * CD * AREA;
        let alpha = (f * arm) / i_yaw; // yaw accel forcing
        let (y, _) = osc.step(alpha, DT);
        peak = peak.max(y.abs());
        if k % 20 == 0 {
            proof.feed_f64(y);
        }
    }
    let whip = peak >= 0.12;
    let knife = peak >= 0.28;
    proof.feed_f64(peak);
    proof.feed_str(if knife {
        "JACKKNIFE"
    } else if whip {
        "WHIP"
    } else {
        "HELD"
    });

    Run {
        id,
        short_id,
        mean_wind_ms: (mean * 10.0).round() / 10.0,
        max_yaw_rad: (peak * 1000.0).round() / 1000.0,
        is_yaw_whip: whip,
        is_jackknife: knife,
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
                "{}/../../data/exports/sovereign/trailer_whip_resonance.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    println!("====================================================================");
    println!("  G^G: TRAILER WHIP  (Dryden gust, kingpin SDOF, 50 Hz)");
    println!("  n={n}");
    println!("====================================================================\n");
    let t0 = Instant::now();
    let mut rng = Rng::new(0x789A_11EF);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("mean_wind_ms", DataType::Float64, false),
        Field::new("max_yaw_rad", DataType::Float64, false),
        Field::new("is_yaw_whip", DataType::Boolean, false),
        Field::new("is_jackknife", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.mean_wind_ms).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.max_yaw_rad).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_yaw_whip).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_jackknife).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G trailer Dryden dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let nf = n as f64;
    let a = rows.iter().filter(|r| r.is_yaw_whip).count();
    let b = rows.iter().filter(|r| r.is_jackknife).count();
    println!(
        "  whip {a} ({:.1}%)  jackknife {b} ({:.1}%)",
        100.0 * a as f64 / nf,
        100.0 * b as f64 / nf
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
