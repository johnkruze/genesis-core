//! Marine salt film. Beer-Lambert on areal density, not 100 RNG hours.
//! Mix sea state, patrol hours, one wash. Clock: constitutive.
//! Gates: T < 0.70 haze vs T < 0.35 horizon lost.

use genesis_core::output;
use genesis_core::physics::optics::salt_film_transmittance;
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
const WINDOW_M2: f64 = 0.04;
const HAZE: f64 = 0.70;
const LOST: f64 = 0.35;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    sea_state: f64,
    final_transmission: f64,
    is_haze: bool,
    is_horizon_lost: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let ss = rng.range(1.5, 6.5);
    let hours = rng.range(0.5, 2.2);
    let wash = rng.chance(0.32);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(ss);
    proof.feed_f64(hours);

    let rate_mg_s = 0.018 * ss;
    let mut mass = rate_mg_s * hours * 3600.0;
    if wash {
        mass *= 0.12;
    }
    let t = salt_film_transmittance(mass, WINDOW_M2);
    let haze = t < HAZE;
    let lost = t < LOST;

    proof.feed_f64(t);
    proof.feed_str(if lost {
        "LOST"
    } else if haze {
        "HAZE"
    } else {
        "CLEAR"
    });

    Run {
        id,
        short_id,
        sea_state: (ss * 10.0).round() / 10.0,
        final_transmission: (t * 1000.0).round() / 1000.0,
        is_haze: haze,
        is_horizon_lost: lost,
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
                "{}/../../data/exports/sovereign/optical_salt_occlusion.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    println!("====================================================================");
    println!("  G^G: SALT FILM  (Beer-Lambert areal, wash mix)");
    println!("  n={n}");
    println!("====================================================================\n");
    let t0 = Instant::now();
    let mut rng = Rng::new(0x3541_77B2);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("sea_state", DataType::Float64, false),
        Field::new("final_transmission", DataType::Float64, false),
        Field::new("is_haze", DataType::Boolean, false),
        Field::new("is_horizon_lost", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.sea_state).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.final_transmission).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_haze).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_horizon_lost).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G salt film dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let nf = n as f64;
    let a = rows.iter().filter(|r| r.is_haze).count();
    let b = rows.iter().filter(|r| r.is_horizon_lost).count();
    println!(
        "  haze {a} ({:.1}%)  lost {b} ({:.1}%)",
        100.0 * a as f64 / nf,
        100.0 * b as f64 / nf
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
