//! Terminal shadow on the target. Pinhole α = D/R, not a 1000 Hz costume.
//! Mix sun-aligned vs off-axis. Clock: 200 Hz last ~2.5 s. Gates: evasion vs miss ≥ 2.0 m.

use genesis_core::output;
use genesis_core::physics::optics::pinhole_apparent_px;
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
const SIZE_M: f64 = 0.38;
const FOCAL_PX: f64 = 2000.0;
const THREAT_PX: f64 = 32.0;
const MISS_M: f64 = 2.0;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    sun_offset_deg: f64,
    final_miss_m: f64,
    is_evasion: bool,
    is_target_missed: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let aligned = rng.chance(0.44);
    let offset = if aligned {
        rng.range(0.0, 8.0)
    } else {
        rng.range(18.0, 48.0)
    };
    let vz = rng.range(95.0, 128.0);
    let a_lat = rng.range(55.0, 108.0);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(offset);
    proof.feed_f64(vz);

    let mut z = 280.0;
    let mut x = 0.0;
    let mut vx = 0.0;
    let mut evaded = false;
    let cos = offset.to_radians().cos();
    while z > 0.0 {
        z -= vz * DT;
        x += vx * DT;
        let r = z.max(0.05);
        let px = pinhole_apparent_px(SIZE_M, r, FOCAL_PX) * cos.max(0.0);
        if aligned && px >= THREAT_PX && !evaded {
            evaded = true;
            vx = 0.0;
        }
        if evaded {
            vx += a_lat * DT;
        }
        if z <= 0.0 {
            break;
        }
    }
    let miss = x.abs();
    let hard = miss >= MISS_M;

    proof.feed_f64(miss);
    proof.feed_str(if hard {
        "MISS"
    } else if evaded {
        "EVADE"
    } else {
        "HIT"
    });

    Run {
        id,
        short_id,
        sun_offset_deg: (offset * 10.0).round() / 10.0,
        final_miss_m: (miss * 100.0).round() / 100.0,
        is_evasion: evaded,
        is_target_missed: hard,
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
                "{}/../../data/exports/sovereign/terminal_dive_shadow.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    println!("====================================================================");
    println!("  G^G: DIVE SHADOW  (pinhole D/R, 200 Hz)");
    println!("  n={n}");
    println!("====================================================================\n");
    let t0 = Instant::now();
    let mut rng = Rng::new(0x4018_007F);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("sun_offset_deg", DataType::Float64, false),
        Field::new("final_miss_m", DataType::Float64, false),
        Field::new("is_evasion", DataType::Boolean, false),
        Field::new("is_target_missed", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.sun_offset_deg).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.final_miss_m).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_evasion).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_target_missed).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G dive shadow dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let nf = n as f64;
    let a = rows.iter().filter(|r| r.is_evasion).count();
    let b = rows.iter().filter(|r| r.is_target_missed).count();
    println!(
        "  evasion {a} ({:.1}%)  miss {b} ({:.1}%)",
        100.0 * a as f64 / nf,
        100.0 * b as f64 / nf
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
