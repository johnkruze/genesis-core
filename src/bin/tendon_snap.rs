//! UHMWPE micro-braid shock. σ = F/A via cable_elastic_mechanics.
//! Gates: σ ≥ 1800 MPa yield vs σ ≥ 2800 MPa UTS snap. Constitutive.

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
const D_MM: f64 = 1.20;
const L_M: f64 = 0.85;
const E_GPA: f64 = 140.0;
const YIELD_MPA: f64 = 1800.0;
const UTS_MPA: f64 = 2800.0;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    shock_tension_n: f64,
    peak_tensile_stress_mpa: f64,
    elastic_elongation_mm: f64,
    is_fiber_yield_warning: bool,
    is_tendon_snapped: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let f = rng.range(1000.0, 4000.0);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(f);

    let (dl, stress) = cable_elastic_mechanics(f, L_M, D_MM, E_GPA);
    let yld = stress >= YIELD_MPA;
    let snap = stress >= UTS_MPA;

    proof.feed_f64(stress);
    proof.feed_str(if snap {
        "SNAPPED"
    } else if yld {
        "YIELD"
    } else {
        "NOMINAL"
    });

    Run {
        id,
        short_id,
        shock_tension_n: (f * 10.0).round() / 10.0,
        peak_tensile_stress_mpa: (stress * 10.0).round() / 10.0,
        elastic_elongation_mm: (dl * 1e3 * 100.0).round() / 100.0,
        is_fiber_yield_warning: yld,
        is_tendon_snapped: snap,
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
                "{}/../../data/exports/sovereign/tendon_snap.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: TENDON UTS  (σ = F/A, yield {YIELD_MPA} / UTS {UTS_MPA} MPa)");
    println!("  n={n}");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0x7186_0006);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("shock_tension_n", DataType::Float64, false),
        Field::new("peak_tensile_stress_mpa", DataType::Float64, false),
        Field::new("elastic_elongation_mm", DataType::Float64, false),
        Field::new("is_fiber_yield_warning", DataType::Boolean, false),
        Field::new("is_tendon_snapped", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.shock_tension_n).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.peak_tensile_stress_mpa).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.elastic_elongation_mm).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_fiber_yield_warning).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_tendon_snapped).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G tendon UTS dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let y = rows.iter().filter(|r| r.is_fiber_yield_warning).count();
    let s = rows.iter().filter(|r| r.is_tendon_snapped).count();
    println!(
        "  yield {y} ({:.1}%)  snap {s} ({:.1}%)",
        100.0 * y as f64 / n_f,
        100.0 * s as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
