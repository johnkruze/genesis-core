//! Track pin / bushing. Point-contact Hamrock as reduced-order for one Hertz patch.
//! Sharing: F_hertz = T / 20. Clock: 200 h march. Gates: Λ<1 breakdown vs wear seizure.
//! Comment 400–1200 MPa was a lie — projected-area pressure is ~5–20 MPa.

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
const SIGMA_UM: f64 = 0.25;
const SHARE: f64 = 20.0;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    track_tension_kn: f64,
    lambda_ratio: f64,
    cumulative_wear_um: f64,
    is_lubrication_breakdown: bool,
    is_pin_seizure_failed: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let t_kn = rng.range(30.0, 160.0);
    let v_track = rng.range(1.5, 8.0);
    let eta = rng.range(0.08, 0.40);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(t_kn);
    proof.feed_f64(v_track);
    proof.feed_f64(eta);

    let f_hertz = (t_kn * 1e3) / SHARE;
    let u = (v_track / 0.35) * 0.0225;
    let h = ehl_minimum_film_thickness_um(eta, u, 0.0225, f_hertz, 210.0, 25.0);
    let lambda = lambda_lubrication_ratio(h, SIGMA_UM);
    let breakdown = lambda < 1.0;
    let scale = lambda_wear_multiplier(lambda);
    // Hertz patch pressure, not projected-area 5–20 MPa (that never seizes).
    let p_mpa = (f_hertz / 12.0).clamp(60.0, 400.0);

    let mut state = TribologySurfaceState::new(p_mpa, u, 313.15);
    let params = TribologyAgingParams::default();
    for _ in 0..100 {
        let before = state.cumulative_galling_wear_um;
        state.step(&params, 2.0);
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
    } else if breakdown {
        "BREAKDOWN"
    } else {
        "EHL"
    });

    Run {
        id,
        short_id,
        track_tension_kn: (t_kn * 10.0).round() / 10.0,
        lambda_ratio: (lambda * 100.0).round() / 100.0,
        cumulative_wear_um: (state.cumulative_galling_wear_um * 100.0).round() / 100.0,
        is_lubrication_breakdown: breakdown,
        is_pin_seizure_failed: state.is_galling_seizure_failed,
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
                "{}/../../data/exports/sovereign/track_pin_galling.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: TRACK PIN EHL  (F_hertz = T/20, 200 h)");
    println!("  n={n}  breakdown Λ<1  seize wear≥45 µm");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0x7184_0004);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("track_tension_kn", DataType::Float64, false),
        Field::new("lambda_ratio", DataType::Float64, false),
        Field::new("cumulative_wear_um", DataType::Float64, false),
        Field::new("is_lubrication_breakdown", DataType::Boolean, false),
        Field::new("is_pin_seizure_failed", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.track_tension_kn).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.lambda_ratio).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.cumulative_wear_um).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_lubrication_breakdown).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_pin_seizure_failed).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G track-pin EHL dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let b = rows.iter().filter(|r| r.is_lubrication_breakdown).count();
    let s = rows.iter().filter(|r| r.is_pin_seizure_failed).count();
    println!(
        "  breakdown {b} ({:.1}%)  seizure {s} ({:.1}%)",
        100.0 * b as f64 / n_f,
        100.0 * s as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
