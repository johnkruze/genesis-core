//! Heel-strike on a 45 Hz spine cantilever. DynamicOscillator, not a 20-tick 0.003 rad park.
//! Mix light vs hard strike. Clock: 500 Hz, 0.40 s stance. Gates: chatter ≥ 2 mrad vs smear ≥ 8 mrad.

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
const DT: f64 = 0.002;
const HORIZON_S: f64 = 0.40;
const FN_HZ: f64 = 45.0;
const CHATTER: f64 = 0.002;
const SMEAR: f64 = 0.0036;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    heel_strike_g: f64,
    max_angular_chatter_rad: f64,
    is_chatter: bool,
    is_lidar_smeared: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let hard = rng.chance(0.40);
    let g_imp = if hard {
        rng.range(3.2, 7.0)
    } else {
        rng.range(0.7, 2.2)
    };
    let zeta = rng.range(0.03, 0.10);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(g_imp);

    let mut osc = DynamicOscillator::new(FN_HZ, zeta);
    let mut peak: f64 = 0.0;
    let steps = (HORIZON_S / DT) as usize;
    for k in 0..steps {
        let t = k as f64 * DT;
        let a = if t < 0.022 {
            g_imp * 9.81 * 3.1
        } else {
            0.0
        };
        let (th, _) = osc.step(a, DT);
        peak = peak.max(th.abs());
        if k % 20 == 0 {
            proof.feed_f64(th);
        }
    }
    let chat = peak >= CHATTER;
    let smear = peak >= SMEAR;

    proof.feed_f64(peak);
    proof.feed_str(if smear {
        "SMEAR"
    } else if chat {
        "CHATTER"
    } else {
        "CLEAR"
    });

    Run {
        id,
        short_id,
        heel_strike_g: (g_imp * 10.0).round() / 10.0,
        max_angular_chatter_rad: (peak * 1e4).round() / 1e4,
        is_chatter: chat,
        is_lidar_smeared: smear,
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
                "{}/../../data/exports/sovereign/humanoid_sensor_vibration_chatter.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    println!("====================================================================");
    println!("  G^G: SPINE CHATTER  (SDOF 45 Hz, 500 Hz stance)");
    println!("  n={n}");
    println!("====================================================================\n");
    let t0 = Instant::now();
    let mut rng = Rng::new(0x45C1_00F0);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("heel_strike_g", DataType::Float64, false),
        Field::new("max_angular_chatter_rad", DataType::Float64, false),
        Field::new("is_chatter", DataType::Boolean, false),
        Field::new("is_lidar_smeared", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.heel_strike_g).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.max_angular_chatter_rad).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_chatter).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_lidar_smeared).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G spine chatter dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let nf = n as f64;
    let a = rows.iter().filter(|r| r.is_chatter).count();
    let b = rows.iter().filter(|r| r.is_lidar_smeared).count();
    println!(
        "  chatter {a} ({:.1}%)  smear {b} ({:.1}%)",
        100.0 * a as f64 / nf,
        100.0 * b as f64 / nf
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
