//! Swing equation. dt=1 ms, 10 s horizon. Weather drop then delayed AI interconnect shock.
//! Exclusive two-way: cascaded · held.
//! Remainder of “not cascaded” is held — named, re-sealed. No FFI. Cousin ai_grid_blackout stays a demo.

use genesis_core::output;
use genesis_core::physics::swing::SynchronousMachine;
use genesis_core::proof;
use genesis_core::rng::Rng;
use rayon::prelude::*;
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{BooleanArray, Float64Array, StringArray, UInt32Array, UInt64Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::ArrowWriter;

const DEFAULT_N: usize = 2500;
const DT: f64 = 0.001; // 1 ms
const MAX_TIME_MS: u64 = 10_000;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    inertia_h: f64,
    weather_drop_pct: f64,
    t_ai_ms: u64,
    damping: f64,
    t_to_cascade_ms: u64,
    final_frequency_drift_hz: f64,
    inverter_tripped: bool,
    is_cascaded: bool,
    is_held: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let inertia_h = rng.range(1.0, 6.0);
    let weather_drop_pct = rng.range(0.0, 50.0);
    let t_ai_ms = rng.range(1.0, 101.0) as u64;
    let damping = rng.range(0.01, 0.10);

    let mut machine = SynchronousMachine::new();
    machine.h_constant = inertia_h;
    machine.damping = damping;
    machine.p_mech = 1.0;
    machine.p_max = 1.05;
    machine.delta = (machine.p_mech / machine.p_max).asin();
    machine.proof.seed(&id.to_le_bytes());
    machine.proof.feed_f64(inertia_h);
    machine.proof.feed_f64(weather_drop_pct);
    machine.proof.feed_f64(t_ai_ms as f64);

    machine.simulate_weather_loss(weather_drop_pct + 10.0);

    for _ in 0..t_ai_ms {
        machine.step(DT);
    }
    machine.ai_apply_load_mismatch(15.0);
    while !machine.cascaded && machine.time_ms < MAX_TIME_MS {
        machine.step(DT);
    }

    let is_cascaded = machine.cascaded;
    let is_held = !is_cascaded;
    let class = if is_cascaded { "CASCADED" } else { "HELD" };
    machine.proof.feed_str(class);

    let t_to_cascade_ms = if is_cascaded { machine.time_ms } else { 0 };
    let final_frequency_drift_hz = machine.delta_omega / (2.0 * std::f64::consts::PI);
    let inverter_tripped = machine.inverter_tripped;
    let proof_hash = machine.get_sealed_hash();

    Run {
        id,
        short_id,
        inertia_h,
        weather_drop_pct,
        t_ai_ms,
        damping,
        t_to_cascade_ms,
        final_frequency_drift_hz,
        inverter_tripped,
        is_cascaded,
        is_held,
        proof_hash,
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
                "{}/../../data/exports/sovereign/swing_synchronism.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: SWING SYNCHRONISM  (dt=1 ms · 10 s)");
    println!("  n={n}  exclusive CASCADED / HELD");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let rows: Vec<Run> = (0..n as u32)
        .into_par_iter()
        .map(|i| {
            let mut rng = Rng::new(0x5A1E_0003u64.wrapping_add(i as u64).wrapping_mul(0x9E3779B97F4A7C15));
            run_one(i, &mut rng)
        })
        .collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let unique: std::collections::HashSet<&str> = proofs.iter().map(|s| s.as_str()).collect();
    assert_eq!(unique.len(), n, "proof_hash must be unique");
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("inertia_h", DataType::Float64, false),
        Field::new("weather_drop_pct", DataType::Float64, false),
        Field::new("t_ai_ms", DataType::UInt64, false),
        Field::new("damping", DataType::Float64, false),
        Field::new("t_to_cascade_ms", DataType::UInt64, false),
        Field::new("final_frequency_drift_hz", DataType::Float64, false),
        Field::new("inverter_tripped", DataType::Boolean, false),
        Field::new("is_cascaded", DataType::Boolean, false),
        Field::new("is_held", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.inertia_h).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.weather_drop_pct).collect::<Vec<_>>())),
            Arc::new(UInt64Array::from(rows.iter().map(|r| r.t_ai_ms).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.damping).collect::<Vec<_>>())),
            Arc::new(UInt64Array::from(rows.iter().map(|r| r.t_to_cascade_ms).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.final_frequency_drift_hz).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.inverter_tripped).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_cascaded).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_held).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G swing synchronism dual-regime v1.1");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let nf = n as f64;
    let casc = rows.iter().filter(|r| r.is_cascaded).count();
    let held = rows.iter().filter(|r| r.is_held).count();
    let both = rows.iter().filter(|r| r.is_cascaded == r.is_held).count();
    let pll = rows.iter().filter(|r| r.inverter_tripped).count();
    println!(
        "  exclusive: cascaded {casc} ({:.1}%)  held {held} ({:.1}%)  sum {}  overlap {both}",
        100.0 * casc as f64 / nf,
        100.0 * held as f64 / nf,
        casc + held
    );
    println!(
        "  inverter PLL trip (nested, not a fourth class) {pll} ({:.1}%)",
        100.0 * pll as f64 / nf
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
