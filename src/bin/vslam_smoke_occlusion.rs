//! Indoor VSLAM in smoke. Beer-Lambert on wall features; tendrils are the complement.
//! Mix haze vs fire. Clock: 30 Hz, 4 s (camera, not 1000 Hz empty ticks).
//! Gates: static features < 22 vs hallucinated ratio ≥ 0.62 (crash).

use genesis_core::output;
use genesis_core::physics::optics::{beer_lambert_transmittance, smoke_extinction_per_m};
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
const DT: f64 = 1.0 / 30.0;
const HORIZON_S: f64 = 4.0;
const PATH_M: f64 = 2.2;
const N0: f64 = 110.0;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    smoke_density_g_m3: f64,
    min_static: f64,
    is_occluded: bool,
    is_tracking_lost: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let fire = rng.chance(0.40);
    let rho0 = if fire {
        rng.range(3.2, 9.0)
    } else {
        rng.range(0.4, 2.0)
    };
    let advect = rng.range(0.05, 0.35);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(rho0);

    let mut min_static = N0;
    let mut peak_ratio: f64 = 0.0;
    let steps = (HORIZON_S / DT) as usize;
    for k in 0..steps {
        let t = k as f64 * DT;
        let rho = (rho0 + advect * t).max(0.2);
        let tr = beer_lambert_transmittance(smoke_extinction_per_m(rho), PATH_M);
        let static_n = N0 * tr;
        let hallu = (6.5 * rho * (1.0 - tr)).max(0.0);
        let ratio = hallu / (static_n + hallu).max(1.0);
        min_static = min_static.min(static_n);
        peak_ratio = peak_ratio.max(ratio);
        if k % 8 == 0 {
            proof.feed_f64(static_n);
        }
    }
    let occluded = min_static < 32.0;
    let crash = peak_ratio >= 0.55 && min_static < 16.0;

    proof.feed_f64(peak_ratio);
    proof.feed_str(if crash {
        "CRASH"
    } else if occluded {
        "OCCLUDED"
    } else {
        "LOCKED"
    });

    Run {
        id,
        short_id,
        smoke_density_g_m3: (rho0 * 10.0).round() / 10.0,
        min_static: (min_static * 10.0).round() / 10.0,
        is_occluded: occluded,
        is_tracking_lost: crash,
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
                "{}/../../data/exports/sovereign/vslam_smoke_occlusion.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    println!("====================================================================");
    println!("  G^G: VSLAM SMOKE  (Beer walls, 30 Hz camera)");
    println!("  n={n}");
    println!("====================================================================\n");
    let t0 = Instant::now();
    let mut rng = Rng::new(0x5011_88A0);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("smoke_density_g_m3", DataType::Float64, false),
        Field::new("min_static", DataType::Float64, false),
        Field::new("is_occluded", DataType::Boolean, false),
        Field::new("is_tracking_lost", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.smoke_density_g_m3).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.min_static).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_occluded).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_tracking_lost).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G VSLAM smoke dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let nf = n as f64;
    let a = rows.iter().filter(|r| r.is_occluded).count();
    let b = rows.iter().filter(|r| r.is_tracking_lost).count();
    println!(
        "  occluded {a} ({:.1}%)  crash {b} ({:.1}%)",
        100.0 * a as f64 / nf,
        100.0 * b as f64 / nf
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
