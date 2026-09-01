//! FMCW LiDAR in rain. Atlas extinction, two-way Beer range, Marshall-Palmer Doppler noise.
//! Mix light vs heavy rain. Clock: constitutive (one look). Not 1000 Hz.
//! Gates: Doppler noise ≥ 1.4 m/s vs range < stop (v t_r + v²/2a).

use genesis_core::output;
use genesis_core::physics::optics::{
    fmcw_rain_doppler_noise_power, lidar_two_way_range_m, rain_extinction_per_m, stopping_distance_m,
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
const R0: f64 = 240.0;
const NOISE_MS: f64 = 1.40;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    rain_mmhr: f64,
    speed_ms: f64,
    range_m: f64,
    is_doppler_noisy: bool,
    is_collision: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let heavy = rng.chance(0.42);
    let rain = if heavy {
        rng.range(22.0, 85.0)
    } else {
        rng.range(1.5, 14.0)
    };
    let speed = rng.range(16.0, 34.0);
    let a_brake = rng.range(0.32, 0.55) * 9.81;
    let t_r = rng.range(0.9, 1.7);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(rain);
    proof.feed_f64(speed);

    let alpha = rain_extinction_per_m(rain);
    let range = lidar_two_way_range_m(R0, alpha);
    let noise = fmcw_rain_doppler_noise_power(rain, speed);
    let stop = stopping_distance_m(speed, a_brake, t_r);
    let noisy = noise >= NOISE_MS;
    let hit = range < stop;

    proof.feed_f64(range);
    proof.feed_str(if hit {
        "COLLISION"
    } else if noisy {
        "NOISY"
    } else {
        "CLEAR"
    });

    Run {
        id,
        short_id,
        rain_mmhr: (rain * 10.0).round() / 10.0,
        speed_ms: (speed * 10.0).round() / 10.0,
        range_m: (range * 10.0).round() / 10.0,
        is_doppler_noisy: noisy,
        is_collision: hit,
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
                "{}/../../data/exports/sovereign/doppler_lidar_rain_scatter.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    println!("====================================================================");
    println!("  G^G: FMCW RAIN  (Atlas β, two-way Beer, constitutive)");
    println!("  n={n}");
    println!("====================================================================\n");
    let t0 = Instant::now();
    let mut rng = Rng::new(0x61DA_0023);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("rain_mmhr", DataType::Float64, false),
        Field::new("speed_ms", DataType::Float64, false),
        Field::new("range_m", DataType::Float64, false),
        Field::new("is_doppler_noisy", DataType::Boolean, false),
        Field::new("is_collision", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.rain_mmhr).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.speed_ms).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.range_m).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_doppler_noisy).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_collision).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G FMCW rain dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let nf = n as f64;
    let a = rows.iter().filter(|r| r.is_doppler_noisy).count();
    let b = rows.iter().filter(|r| r.is_collision).count();
    println!(
        "  noisy {a} ({:.1}%)  collision {b} ({:.1}%)",
        100.0 * a as f64 / nf,
        100.0 * b as f64 / nf
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
