//! Hydraulic column. Beta_eff with entrained air, δL = L ΔP/B. Not a 1000 Hz constant loop.
//! Mix dry vs aerated. Clock: constitutive. Gates: overshoot ≥ 0.9 mm vs ≥ 3.0 mm.

use genesis_core::output;
use genesis_core::physics::hydraulics::{
    entrained_air_effective_bulk_modulus_pa, hydraulic_column_compression_m, NOMINAL_OIL_BULK_MODULUS_PA,
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
const L: f64 = 0.50;
const WARN_M: f64 = 0.0018;
const HARD_M: f64 = 0.0030;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    entrained_air_pct: f64,
    max_overshoot_mm: f64,
    is_soft: bool,
    is_task_failed: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let aerated = rng.chance(0.40);
    let x_air = if aerated {
        rng.range(0.010, 0.028)
    } else {
        rng.range(0.0002, 0.003)
    };
    let p_pa = rng.range(0.8e6, 3.8e6);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(x_air);
    proof.feed_f64(p_pa);

    let b = entrained_air_effective_bulk_modulus_pa(NOMINAL_OIL_BULK_MODULUS_PA, x_air, p_pa);
    let d = hydraulic_column_compression_m(L, p_pa, b);
    let soft = d >= WARN_M;
    let fail = d >= HARD_M;

    proof.feed_f64(d);
    proof.feed_str(if fail {
        "OVERSHOOT"
    } else if soft {
        "SOFT"
    } else {
        "STIFF"
    });

    Run {
        id,
        short_id,
        entrained_air_pct: (x_air * 1000.0).round() / 10.0,
        max_overshoot_mm: (d * 1e3 * 100.0).round() / 100.0,
        is_soft: soft,
        is_task_failed: fail,
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
                "{}/../../data/exports/sovereign/hydraulic_fluid_compressibility.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    println!("====================================================================");
    println!("  G^G: HYDRAULIC B  (entrained air, δ=L ΔP/B)");
    println!("  n={n}");
    println!("====================================================================\n");
    let t0 = Instant::now();
    let mut rng = Rng::new(0x7A11_00C2);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("entrained_air_pct", DataType::Float64, false),
        Field::new("max_overshoot_mm", DataType::Float64, false),
        Field::new("is_soft", DataType::Boolean, false),
        Field::new("is_task_failed", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.entrained_air_pct).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.max_overshoot_mm).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_soft).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_task_failed).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G hydraulic Beta dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let nf = n as f64;
    let a = rows.iter().filter(|r| r.is_soft).count();
    let b = rows.iter().filter(|r| r.is_task_failed).count();
    println!(
        "  soft {a} ({:.1}%)  overshoot {b} ({:.1}%)",
        100.0 * a as f64 / nf,
        100.0 * b as f64 / nf
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
