//! Thin-lens optothermal defocus vs geometric depth of focus.
//! Δf = f [α − (dn/dT)/(n−1)] ΔT. Blur when |Δf| > δ = 2λN². Lock lost when |Δf| > 4δ.
//! Constitutive (no 1000 Hz loop). Organ: lens_optothermal_defocus_m, geometric_depth_of_focus_m.

use genesis_core::output;
use genesis_core::physics::thermal::{
    geometric_depth_of_focus_m, lens_optothermal_defocus_m, VISIBLE_WAVELENGTH_M,
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
const T_CAL_C: f64 = 20.0;
const N_BK7: f64 = 1.5168;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    lens_temp_c: f64,
    focal_length_mm: f64,
    defocus_um: f64,
    depth_of_focus_um: f64,
    is_focal_blur_detected: bool,
    is_optical_lock_lost: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let t_c = rng.range(30.0, 95.0);
    let f_m = rng.range(0.025, 0.180);
    let n_number = rng.range(1.4, 4.0);
    let alpha = rng.range(6.8e-6, 8.5e-6);
    let dn_dt = rng.range(2.5e-6, 4.5e-6);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(t_c);
    proof.feed_f64(f_m);
    proof.feed_f64(n_number);

    let df = lens_optothermal_defocus_m(f_m, alpha, dn_dt, N_BK7, t_c - T_CAL_C).abs();
    let dof = geometric_depth_of_focus_m(VISIBLE_WAVELENGTH_M, n_number);
    let blur = df > dof;
    let lost = df > 4.0 * dof;

    proof.feed_f64(df);
    proof.feed_f64(dof);
    proof.feed_str(if lost {
        "LOCK_LOST"
    } else if blur {
        "BLURRED"
    } else {
        "NOMINAL"
    });

    Run {
        id,
        short_id,
        lens_temp_c: (t_c * 10.0).round() / 10.0,
        focal_length_mm: (f_m * 1e3 * 10.0).round() / 10.0,
        defocus_um: (df * 1e6 * 100.0).round() / 100.0,
        depth_of_focus_um: (dof * 1e6 * 100.0).round() / 100.0,
        is_focal_blur_detected: blur,
        is_optical_lock_lost: lost,
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
                "{}/../../data/exports/sovereign/thermal_lens_warp.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: THIN-LENS OPTOTHERMAL  (Δf vs 2λN²)");
    println!("  n={n}  BK7  λ=550 nm");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0x7381_0005);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("lens_temp_c", DataType::Float64, false),
        Field::new("focal_length_mm", DataType::Float64, false),
        Field::new("defocus_um", DataType::Float64, false),
        Field::new("depth_of_focus_um", DataType::Float64, false),
        Field::new("is_focal_blur_detected", DataType::Boolean, false),
        Field::new("is_optical_lock_lost", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.lens_temp_c).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.focal_length_mm).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.defocus_um).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.depth_of_focus_um).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_focal_blur_detected).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_optical_lock_lost).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G thin-lens Δf vs DoF dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let blur = rows.iter().filter(|r| r.is_focal_blur_detected).count();
    let lost = rows.iter().filter(|r| r.is_optical_lock_lost).count();
    println!(
        "  blur {blur} ({:.1}%)  lock_lost {lost} ({:.1}%)",
        100.0 * blur as f64 / n_f,
        100.0 * lost as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
