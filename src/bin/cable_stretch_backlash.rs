//! Dyneema tendon elastic stretch. ΔL = F L / A E. Constitutive.
//! Gates: ΔL ≥ 2 mm tracking degrade vs ΔL ≥ 5 mm control limit.
//! Tension mix 70–1100 N so 5 mm binds. Organ: cable_elastic_mechanics.

use genesis_core::output;
use genesis_core::physics::tribology::cable_elastic_mechanics;
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
const L_M: f64 = 1.20;
const D_MM: f64 = 1.50;
const E_GPA: f64 = 120.0;
const DEGRADE_MM: f64 = 2.0;
const LIMIT_MM: f64 = 5.0;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    peak_tension_n: f64,
    elastic_elongation_mm: f64,
    tensile_stress_mpa: f64,
    is_backlash_degraded: bool,
    is_control_limit_exceeded: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let f = rng.range(70.0, 1100.0);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(f);

    let (dl, stress) = cable_elastic_mechanics(f, L_M, D_MM, E_GPA);
    let mm = dl * 1000.0;
    let deg = mm >= DEGRADE_MM;
    let lim = mm >= LIMIT_MM;

    proof.feed_f64(mm);
    proof.feed_f64(stress);
    proof.feed_str(if lim {
        "CONTROL_LIMIT"
    } else if deg {
        "DEGRADED"
    } else {
        "NOMINAL"
    });

    Run {
        id,
        short_id,
        peak_tension_n: (f * 10.0).round() / 10.0,
        elastic_elongation_mm: (mm * 100.0).round() / 100.0,
        tensile_stress_mpa: (stress * 10.0).round() / 10.0,
        is_backlash_degraded: deg,
        is_control_limit_exceeded: lim,
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
                "{}/../../data/exports/sovereign/cable_stretch_backlash.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: DYNEEMA ΔL = FL/AE");
    println!("  n={n}  degrade {DEGRADE_MM} mm  limit {LIMIT_MM} mm");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0x7185_0005);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("peak_tension_n", DataType::Float64, false),
        Field::new("elastic_elongation_mm", DataType::Float64, false),
        Field::new("tensile_stress_mpa", DataType::Float64, false),
        Field::new("is_backlash_degraded", DataType::Boolean, false),
        Field::new("is_control_limit_exceeded", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.peak_tension_n).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.elastic_elongation_mm).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.tensile_stress_mpa).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_backlash_degraded).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_control_limit_exceeded).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G Dyneema FL/AE dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let d = rows.iter().filter(|r| r.is_backlash_degraded).count();
    let lim = rows.iter().filter(|r| r.is_control_limit_exceeded).count();
    println!(
        "  degraded {d} ({:.1}%)  limit {lim} ({:.1}%)",
        100.0 * d as f64 / n_f,
        100.0 * lim as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
