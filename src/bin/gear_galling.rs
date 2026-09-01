//! Gear-tooth EHL under pitch-line sliding. Hamrock–Dowson + Λ, Archard × Λ-scale.
//! Seizure is wear/μ after cycles, not μ F at t=0. Clock: 200 h equivalent in 2 h steps.
//! Gates: Λ<1 film depleted vs wear seizure.

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
const SIGMA_UM: f64 = 0.12;
const R_M: f64 = 0.012;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    pitch_velocity_m_s: f64,
    ehl_film_thickness_um: f64,
    lambda_ratio: f64,
    cumulative_wear_um: f64,
    is_film_depleted: bool,
    is_galling_seizure_failed: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let f_n = rng.range(200.0, 1400.0);
    let u = rng.range(0.35, 2.5);
    let eta = rng.range(0.025, 0.080);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(f_n);
    proof.feed_f64(u);
    proof.feed_f64(eta);

    let h = ehl_minimum_film_thickness_um(eta, u, R_M, f_n, 220.0, 20.0);
    let lambda = lambda_lubrication_ratio(h, SIGMA_UM);
    let depleted = lambda < 1.0;
    let scale = lambda_wear_multiplier(lambda);
    let p_mpa = (f_n / (0.010 * 0.0008 * 1e6)).clamp(80.0, 900.0);

    let mut state = TribologySurfaceState::new(p_mpa, u, 330.0);
    let params = TribologyAgingParams::default();
    for _ in 0..80 {
        let before = state.cumulative_galling_wear_um;
        state.step(&params, 1.0);
        let dw = state.cumulative_galling_wear_um - before;
        state.cumulative_galling_wear_um = before + dw * scale;
        if state.cumulative_galling_wear_um > 45.0 {
            state.is_galling_seizure_failed = true;
            break;
        }
        state.is_galling_seizure_failed = false;
    }

    proof.feed_f64(lambda);
    proof.feed_f64(state.cumulative_galling_wear_um);
    proof.feed_str(if state.is_galling_seizure_failed {
        "SEIZURE"
    } else if depleted {
        "FILM_DEPLETED"
    } else {
        "EHL"
    });

    Run {
        id,
        short_id,
        pitch_velocity_m_s: (u * 100.0).round() / 100.0,
        ehl_film_thickness_um: (h * 1000.0).round() / 1000.0,
        lambda_ratio: (lambda * 100.0).round() / 100.0,
        cumulative_wear_um: (state.cumulative_galling_wear_um * 100.0).round() / 100.0,
        is_film_depleted: depleted,
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
                "{}/../../data/exports/sovereign/gear_galling.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: GEAR EHL  (pitch-line sliding, Λ-scaled Archard)");
    println!("  n={n}  film Λ<1  seize wear≥45 µm");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0x7183_0003);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("pitch_velocity_m_s", DataType::Float64, false),
        Field::new("ehl_film_thickness_um", DataType::Float64, false),
        Field::new("lambda_ratio", DataType::Float64, false),
        Field::new("cumulative_wear_um", DataType::Float64, false),
        Field::new("is_film_depleted", DataType::Boolean, false),
        Field::new("is_galling_seizure_failed", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.pitch_velocity_m_s).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.ehl_film_thickness_um).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.lambda_ratio).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.cumulative_wear_um).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_film_depleted).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_galling_seizure_failed).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G gear EHL dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let d = rows.iter().filter(|r| r.is_film_depleted).count();
    let s = rows.iter().filter(|r| r.is_galling_seizure_failed).count();
    println!(
        "  film_depleted {d} ({:.1}%)  seizure {s} ({:.1}%)",
        100.0 * d as f64 / n_f,
        100.0 * s as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
