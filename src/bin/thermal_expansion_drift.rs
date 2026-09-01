//! Aluminum stereo baseline expansion. ΔZ = Z · α · ΔT (linear in range, not quadratic).
//! Clock: constitutive (no time loop). Gates: ΔZ ≥ 2 mm degraded vs ΔZ ≥ 5 mm grasp-invalid.
//! Workspace 1.2–6.5 m so both gates can bind. Organ: thermal_expansion_strain.

use genesis_core::output;
use genesis_core::physics::thermal::thermal_expansion_strain;
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
const B0_M: f64 = 0.150;
const ALPHA: f64 = 23.1e-6; // 6061-T6
const T_CAL_C: f64 = 20.0;
const Z_DEGRADE_MM: f64 = 2.0;
const Z_INVALID_MM: f64 = 5.0;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    operating_temp_c: f64,
    target_distance_m: f64,
    depth_error_mm: f64,
    is_precision_degraded: bool,
    is_grasp_pose_invalidated: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let t_c = rng.range(25.0, 75.0);
    let z_m = rng.range(1.2, 6.5);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(t_c);
    proof.feed_f64(z_m);

    let strain = thermal_expansion_strain(ALPHA, t_c - T_CAL_C);
    let dz_mm = z_m * strain * 1000.0;
    let degraded = dz_mm >= Z_DEGRADE_MM;
    let invalid = dz_mm >= Z_INVALID_MM;

    proof.feed_f64(dz_mm);
    proof.feed_str(if invalid {
        "INVALIDATED"
    } else if degraded {
        "DEGRADED"
    } else {
        "NOMINAL"
    });

    Run {
        id,
        short_id,
        operating_temp_c: (t_c * 10.0).round() / 10.0,
        target_distance_m: (z_m * 100.0).round() / 100.0,
        depth_error_mm: (dz_mm * 100.0).round() / 100.0,
        is_precision_degraded: degraded,
        is_grasp_pose_invalidated: invalid,
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
                "{}/../../data/exports/sovereign/thermal_expansion_drift.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: STEREO BASELINE EXPANSION  (ΔZ = Z α ΔT)");
    println!("  n={n}  b0={B0_M} m  degrade {Z_DEGRADE_MM} mm  invalid {Z_INVALID_MM} mm");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0x3344_0004);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("operating_temp_c", DataType::Float64, false),
        Field::new("target_distance_m", DataType::Float64, false),
        Field::new("depth_error_mm", DataType::Float64, false),
        Field::new("is_precision_degraded", DataType::Boolean, false),
        Field::new("is_grasp_pose_invalidated", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.operating_temp_c).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.target_distance_m).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.depth_error_mm).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_precision_degraded).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_grasp_pose_invalidated).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G stereo αΔT dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let d = rows.iter().filter(|r| r.is_precision_degraded).count();
    let inv = rows.iter().filter(|r| r.is_grasp_pose_invalidated).count();
    println!(
        "  degraded {d} ({:.1}%)  invalid {inv} ({:.1}%)",
        100.0 * d as f64 / n_f,
        100.0 * inv as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
