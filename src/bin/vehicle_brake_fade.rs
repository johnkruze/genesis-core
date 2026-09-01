//! Grade descent. Brake power m g sinθ v into lumped rotor; fluid lags.
//! Mix 4–14° not a 30° cliff. Clock: 2 Hz, 90 s. Gates: rotor ≥ 380 °C vs fluid ≥ 180 °C (wet DOT-4).

use genesis_core::output;
use genesis_core::physics::thermal::LumpedThermalNode;
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
const DT: f64 = 0.5;
const HORIZON_S: f64 = 90.0;
const MASS: f64 = 10_500.0;
const ROTOR_C: f64 = 380.0;
const FLUID_C: f64 = 180.0;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    decline_angle_deg: f64,
    max_fluid_temp_c: f64,
    is_rotor_hot: bool,
    is_freefall: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let steep = rng.chance(0.40);
    let ang = if steep {
        rng.range(8.0, 14.0)
    } else {
        rng.range(3.5, 7.5)
    };
    let v = rng.range(6.0, 12.0);
    let derate = rng.range(0.55, 1.0);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(ang);
    proof.feed_f64(v);

    let p_w = MASS * 9.81 * ang.to_radians().sin() * v * derate;
    let mut rotor = LumpedThermalNode::new(40.0, 16_000.0, 0.0036);
    let mut fluid = LumpedThermalNode::new(40.0, 2_200.0, 0.14);
    let amb = rng.range(18.0, 38.0);
    let steps = (HORIZON_S / DT) as usize;
    for k in 0..steps {
        rotor.step(p_w, amb, DT);
        let q_to_fluid = (rotor.temperature_c - fluid.temperature_c) / 0.14;
        fluid.step(q_to_fluid.max(0.0), rotor.temperature_c, DT);
        if k % 20 == 0 {
            proof.feed_f64(fluid.temperature_c);
        }
    }
    let hot = rotor.temperature_c >= ROTOR_C;
    let boil = fluid.temperature_c >= FLUID_C;

    proof.feed_f64(fluid.temperature_c);
    proof.feed_str(if boil {
        "BOIL"
    } else if hot {
        "ROTOR"
    } else {
        "HOLD"
    });

    Run {
        id,
        short_id,
        decline_angle_deg: (ang * 10.0).round() / 10.0,
        max_fluid_temp_c: (fluid.temperature_c * 10.0).round() / 10.0,
        is_rotor_hot: hot,
        is_freefall: boil,
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
                "{}/../../data/exports/sovereign/vehicle_brake_fade.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    println!("====================================================================");
    println!("  G^G: BRAKE FADE  (lumped rotor/fluid, 2 Hz)");
    println!("  n={n}");
    println!("====================================================================\n");
    let t0 = Instant::now();
    let mut rng = Rng::new(0xB0A1_00D4);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("decline_angle_deg", DataType::Float64, false),
        Field::new("max_fluid_temp_c", DataType::Float64, false),
        Field::new("is_rotor_hot", DataType::Boolean, false),
        Field::new("is_freefall", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.decline_angle_deg).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.max_fluid_temp_c).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_rotor_hot).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_freefall).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G brake fade dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let nf = n as f64;
    let a = rows.iter().filter(|r| r.is_rotor_hot).count();
    let b = rows.iter().filter(|r| r.is_freefall).count();
    println!(
        "  rotor {a} ({:.1}%)  boil {b} ({:.1}%)",
        100.0 * a as f64 / nf,
        100.0 * b as f64 / nf
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
