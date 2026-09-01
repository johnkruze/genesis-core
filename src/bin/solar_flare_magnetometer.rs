//! Mag vs GPS heading. The product is trust, not a 40° G3 costume. Storms are a few degrees.
//! Mix quiet/storm, GPS-led vs mag-led, GPS-denied window. Clock: 10 Hz, 180 s.
//! Gates: crab ≥ 40 m vs lost ≥ 160 m.

use genesis_core::output;
use genesis_core::physics::optics::mag_gps_fused_heading_rad;
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
const DT: f64 = 0.10;
const HORIZON_S: f64 = 180.0;
const SPEED: f64 = 14.0;
const CRAB_M: f64 = 40.0;
const LOST_M: f64 = 160.0;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    mag_deviation_deg: f64,
    max_deviation_m: f64,
    is_crabbing: bool,
    is_drone_lost: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let storm = rng.chance(0.40);
    let delta = if storm {
        rng.range(1.6, 5.2).to_radians()
    } else {
        rng.range(0.15, 1.1).to_radians()
    };
    let mag_led = rng.chance(0.48);
    let trust = if mag_led {
        rng.range(0.72, 0.95)
    } else {
        rng.range(0.12, 0.40)
    };
    let denied = rng.chance(0.30);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(delta);
    proof.feed_f64(trust);

    let mut x = 0.0;
    let mut peak: f64 = 0.0;
    let steps = (HORIZON_S / DT) as usize;
    for k in 0..steps {
        let gps_live = !(denied && (k as f64 * DT) > 40.0);
        let w = if gps_live { trust } else { 1.0 };
        // Null fused heading to North: vehicle holds −w·δ.
        let fused = mag_gps_fused_heading_rad(delta, 0.0, w);
        x += SPEED * fused.sin() * DT;
        peak = peak.max(x.abs());
        if k % 40 == 0 {
            proof.feed_f64(x);
        }
    }
    let crab = peak >= CRAB_M;
    let lost = peak >= LOST_M;

    proof.feed_f64(peak);
    proof.feed_str(if lost {
        "LOST"
    } else if crab {
        "CRAB"
    } else {
        "ON_TRACK"
    });

    Run {
        id,
        short_id,
        mag_deviation_deg: (delta.to_degrees() * 10.0).round() / 10.0,
        max_deviation_m: (peak * 10.0).round() / 10.0,
        is_crabbing: crab,
        is_drone_lost: lost,
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
                "{}/../../data/exports/sovereign/solar_flare_magnetometer.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    println!("====================================================================");
    println!("  G^G: MAG TRUST  (degrees, not 40° G3, 10 Hz)");
    println!("  n={n}");
    println!("====================================================================\n");
    let t0 = Instant::now();
    let mut rng = Rng::new(0x2719_00E5);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("mag_deviation_deg", DataType::Float64, false),
        Field::new("max_deviation_m", DataType::Float64, false),
        Field::new("is_crabbing", DataType::Boolean, false),
        Field::new("is_drone_lost", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.mag_deviation_deg).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.max_deviation_m).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_crabbing).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_drone_lost).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G mag-trust dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let nf = n as f64;
    let a = rows.iter().filter(|r| r.is_crabbing).count();
    let b = rows.iter().filter(|r| r.is_drone_lost).count();
    println!(
        "  crab {a} ({:.1}%)  lost {b} ({:.1}%)",
        100.0 * a as f64 / nf,
        100.0 * b as f64 / nf
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
