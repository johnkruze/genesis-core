//! Roller-screw EHL. Hamrock–Dowson film, Λ = h/σ, Archard wear scaled by Λ.
//! Mix of speed and load so boundary, mixed, and EHL all exist.
//! Clock: 50 h duty in 0.5 h steps. Gates: Λ<1 boundary vs wear≥45 µm seizure.

use genesis_core::output;
use genesis_core::physics::tribology::{
    ehl_minimum_film_thickness_um, lambda_lubrication_ratio, lambda_wear_multiplier,
    TribologyAgingParams, TribologySurfaceState,
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
const SIGMA_UM: f64 = 0.08;
const E_GPA: f64 = 210.0;
const ALPHA: f64 = 22.0;
const R_M: f64 = 0.005;
const WEAR_SEIZE_UM: f64 = 45.0;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    sliding_velocity_m_s: f64,
    ehl_film_thickness_um: f64,
    lambda_ratio: f64,
    cumulative_wear_um: f64,
    is_boundary_lubrication: bool,
    is_galling_seizure_failed: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let p_mpa = rng.range(150.0, 1000.0);
    let u = rng.range(0.20, 3.0);
    let eta = rng.range(0.03, 0.10);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(p_mpa);
    proof.feed_f64(u);
    proof.feed_f64(eta);

    let f_n = p_mpa * 1e6 * std::f64::consts::PI * (0.0004f64).powi(2);
    let h = ehl_minimum_film_thickness_um(eta, u, R_M, f_n, E_GPA, ALPHA);
    let lambda = lambda_lubrication_ratio(h, SIGMA_UM);
    let boundary = lambda < 1.0;
    let scale = lambda_wear_multiplier(lambda);

    let mut state = TribologySurfaceState::new(p_mpa, u, 310.0);
    let params = TribologyAgingParams::default();
    for _ in 0..40 {
        let before = state.cumulative_galling_wear_um;
        state.step(&params, 0.5);
        let dw = state.cumulative_galling_wear_um - before;
        state.cumulative_galling_wear_um = before + dw * scale;
        if state.cumulative_galling_wear_um > WEAR_SEIZE_UM {
            state.is_galling_seizure_failed = true;
            break;
        }
        state.is_galling_seizure_failed = false;
    }

    proof.feed_f64(lambda);
    proof.feed_f64(state.cumulative_galling_wear_um);
    proof.feed_str(if state.is_galling_seizure_failed {
        "SEIZURE"
    } else if boundary {
        "BOUNDARY"
    } else {
        "EHL"
    });

    Run {
        id,
        short_id,
        sliding_velocity_m_s: (u * 100.0).round() / 100.0,
        ehl_film_thickness_um: (h * 1000.0).round() / 1000.0,
        lambda_ratio: (lambda * 100.0).round() / 100.0,
        cumulative_wear_um: (state.cumulative_galling_wear_um * 100.0).round() / 100.0,
        is_boundary_lubrication: boundary,
        is_galling_seizure_failed: state.is_galling_seizure_failed,
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
                "{}/../../data/exports/sovereign/actuator_tribology.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: ROLLER-SCREW EHL  (Hamrock–Dowson Λ, Archard × Λ-scale)");
    println!("  n={n}  20 h  boundary Λ<1  seize wear≥{WEAR_SEIZE_UM} µm");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0x7182_0002);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("sliding_velocity_m_s", DataType::Float64, false),
        Field::new("ehl_film_thickness_um", DataType::Float64, false),
        Field::new("lambda_ratio", DataType::Float64, false),
        Field::new("cumulative_wear_um", DataType::Float64, false),
        Field::new("is_boundary_lubrication", DataType::Boolean, false),
        Field::new("is_galling_seizure_failed", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.sliding_velocity_m_s).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.ehl_film_thickness_um).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.lambda_ratio).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.cumulative_wear_um).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_boundary_lubrication).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_galling_seizure_failed).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G roller-screw EHL dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let b = rows.iter().filter(|r| r.is_boundary_lubrication).count();
    let s = rows.iter().filter(|r| r.is_galling_seizure_failed).count();
    println!(
        "  boundary {b} ({:.1}%)  seizure {s} ({:.1}%)",
        100.0 * b as f64 / n_f,
        100.0 * s as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
