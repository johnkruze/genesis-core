//! 25 mm burst. Barrel heat is a kJ fraction, not 200–300 kJ/shot. ε=αΔT, drift = R tan(ε L/d).
//! Clock: constitutive n_shots. Gates: drift ≥ 1.5 m vs ≥ 5.0 m at 800 m.

use genesis_core::output;
use genesis_core::physics::thermal::{thermal_expansion_strain, LumpedThermalNode};
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
const ALPHA: f64 = 12.0e-6;
const L: f64 = 2.0;
const D: f64 = 0.06;
const RANGE: f64 = 800.0;
const WARN_M: f64 = 1.5;
const HARD_M: f64 = 5.0;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    n_shots: f64,
    max_projectile_drift_m: f64,
    is_off_aim: bool,
    is_friendly_fire: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let burst = rng.chance(0.40);
    let n_shots = if burst {
        rng.range(40.0, 90.0)
    } else {
        rng.range(6.0, 24.0)
    };
    let q_shot = rng.range(8.0e3, 28.0e3); // J, not 200 kJ
    let wind = rng.range(2.0, 14.0);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(n_shots);
    proof.feed_f64(q_shot);

    let p_w = q_shot * (n_shots / 30.0); // mean power over 30 s
    let mut barrel = LumpedThermalNode::new(35.0, 8_000.0, 0.005);
    barrel.step(p_w, 20.0, 30.0);
    let d_t = (barrel.temperature_c - 20.0) * (0.010 * wind);
    let eps = thermal_expansion_strain(ALPHA, d_t);
    let ang = (eps * L) / D;
    let drift = RANGE * ang.abs().tan();
    let off = drift >= WARN_M;
    let ff = drift >= HARD_M;

    proof.feed_f64(drift);
    proof.feed_str(if ff {
        "FRIENDLY"
    } else if off {
        "DRIFT"
    } else {
        "ON"
    });

    Run {
        id,
        short_id,
        n_shots: n_shots.round(),
        max_projectile_drift_m: (drift * 100.0).round() / 100.0,
        is_off_aim: off,
        is_friendly_fire: ff,
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
                "{}/../../data/exports/sovereign/gun_barrel_warp.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    println!("====================================================================");
    println!("  G^G: BARREL WARP  (kJ/shot, αΔT)");
    println!("  n={n}");
    println!("====================================================================\n");
    let t0 = Instant::now();
    let mut rng = Rng::new(0x25B0_00E1);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("n_shots", DataType::Float64, false),
        Field::new("max_projectile_drift_m", DataType::Float64, false),
        Field::new("is_off_aim", DataType::Boolean, false),
        Field::new("is_friendly_fire", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.n_shots).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.max_projectile_drift_m).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_off_aim).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_friendly_fire).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G barrel warp dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let nf = n as f64;
    let a = rows.iter().filter(|r| r.is_off_aim).count();
    let b = rows.iter().filter(|r| r.is_friendly_fire).count();
    println!(
        "  drift {a} ({:.1}%)  friendly {b} ({:.1}%)",
        100.0 * a as f64 / nf,
        100.0 * b as f64 / nf
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
