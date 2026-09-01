//! Non-penetrating dart. Delay r/c_steel, peak g ∝ KE/r². Not KE/1e5·15 parked at 550 g.
//! Mix standoff. Clock: constitutive. Gates: g ≥ 80 stun vs g ≥ 420 brick.

use genesis_core::output;
use genesis_core::physics::resonance::{inverse_square_shock_g, STEEL_BAR_WAVE_MS};
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
const MASS: f64 = 4.8;
const COUPLING: f64 = 1.6e-4;
const STUN_G: f64 = 80.0;
const BRICK_G: f64 = 420.0;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    dart_velocity_ms: f64,
    loom_proximity_m: f64,
    peak_shock_g: f64,
    is_loom_stunned: bool,
    is_central_ai_bricked: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let near = rng.chance(0.38);
    let r = if near {
        rng.range(0.22, 0.70)
    } else {
        rng.range(0.90, 2.40)
    };
    let v = rng.range(1100.0, 1700.0);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(v);
    proof.feed_f64(r);

    let ke = 0.5 * MASS * v * v;
    let t_arr = r / STEEL_BAR_WAVE_MS;
    let g = inverse_square_shock_g(ke, r, COUPLING);
    let stun = g >= STUN_G;
    let brick = g >= BRICK_G;

    proof.feed_f64(t_arr);
    proof.feed_f64(g);
    proof.feed_str(if brick {
        "BRICK"
    } else if stun {
        "STUN"
    } else {
        "HOLD"
    });

    Run {
        id,
        short_id,
        dart_velocity_ms: (v * 10.0).round() / 10.0,
        loom_proximity_m: (r * 100.0).round() / 100.0,
        peak_shock_g: (g * 10.0).round() / 10.0,
        is_loom_stunned: stun,
        is_central_ai_bricked: brick,
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
                "{}/../../data/exports/sovereign/armor_spall_sensor_shearing.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    println!("====================================================================");
    println!("  G^G: HULL SHOCK  (r/c_steel, KE/r²)");
    println!("  n={n}");
    println!("====================================================================\n");
    let t0 = Instant::now();
    let mut rng = Rng::new(0x5A11_00B7);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("dart_velocity_ms", DataType::Float64, false),
        Field::new("loom_proximity_m", DataType::Float64, false),
        Field::new("peak_shock_g", DataType::Float64, false),
        Field::new("is_loom_stunned", DataType::Boolean, false),
        Field::new("is_central_ai_bricked", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.dart_velocity_ms).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.loom_proximity_m).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.peak_shock_g).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_loom_stunned).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_central_ai_bricked).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G hull shock dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let nf = n as f64;
    let a = rows.iter().filter(|r| r.is_loom_stunned).count();
    let b = rows.iter().filter(|r| r.is_central_ai_bricked).count();
    println!(
        "  stun {a} ({:.1}%)  brick {b} ({:.1}%)",
        100.0 * a as f64 / nf,
        100.0 * b as f64 / nf
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
