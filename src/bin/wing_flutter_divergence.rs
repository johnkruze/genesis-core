//! Wing SDOF at q = ½ρV². Aero forcing is sin(ωt), not sin(ω).
//! Delay-line aileron (not a PID sign error). Clock: 200 Hz, 3 s.
//! Gates: |y|≥20 mm divergent vs |k y|≥80 kN spar shear.
//! Organ: aero::DelayAeroservoelastic, dynamic_pressure_pa.

use genesis_core::output;
use genesis_core::physics::aero::{dynamic_pressure_pa, tas_from_mach, DelayAeroservoelastic, RHO_SL};
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
const DT: f64 = 0.005;
const HORIZON_S: f64 = 3.0;
const Y_DIV_M: f64 = 0.020;
const SHEAR_N: f64 = 80_000.0;
const K_SPAR: f64 = 2.0e6;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    airspeed_mach: f64,
    max_deflection_m: f64,
    max_shear_force_n: f64,
    is_divergent: bool,
    is_wing_detached: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let mach = rng.range(0.50, 1.20);
    let f_hz = rng.range(11.0, 18.0);
    let latency_s = rng.range(0.010, 0.045);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(mach);
    proof.feed_f64(f_hz);
    proof.feed_f64(latency_s);

    let q = dynamic_pressure_pa(RHO_SL, tas_from_mach(mach));
    let q_ref = dynamic_pressure_pa(RHO_SL, tas_from_mach(0.85));
    let amp = 15.0 * (q / q_ref);
    let gain = -30.0 * (q / q_ref) * (latency_s / 0.022);

    let mut wing = DelayAeroservoelastic::new(f_hz, 0.04, latency_s, DT);
    let mut peak_y: f64 = 0.0;
    let mut peak_shear: f64 = 0.0;
    let steps = (HORIZON_S / DT) as usize;
    for k in 0..steps {
        let t = k as f64 * DT;
        let y = wing.step(amp * (wing.omega_rad_s * t).sin(), gain, DT);
        peak_y = peak_y.max(y.abs());
        peak_shear = peak_shear.max((K_SPAR * y).abs());
        if peak_shear >= SHEAR_N {
            break;
        }
    }

    let div = peak_y >= Y_DIV_M;
    let det = peak_shear >= SHEAR_N;
    proof.feed_f64(peak_y);
    proof.feed_f64(peak_shear);
    proof.feed_str(if det {
        "DETACHED"
    } else if div {
        "DIVERGENT"
    } else {
        "NOMINAL"
    });

    Run {
        id,
        short_id,
        airspeed_mach: (mach * 100.0).round() / 100.0,
        max_deflection_m: (peak_y * 1e4).round() / 1e4,
        max_shear_force_n: peak_shear.round(),
        is_divergent: div,
        is_wing_detached: det,
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
                "{}/../../data/exports/sovereign/wing_flutter_divergence.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: SPAR SDOF  (sin(ωt), delay-line aileron, 200 Hz)");
    println!("  n={n}  div {Y_DIV_M} m  shear {SHEAR_N} N");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0xD1FE_0077);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("airspeed_mach", DataType::Float64, false),
        Field::new("max_deflection_m", DataType::Float64, false),
        Field::new("max_shear_force_n", DataType::Float64, false),
        Field::new("is_divergent", DataType::Boolean, false),
        Field::new("is_wing_detached", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.airspeed_mach).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.max_deflection_m).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.max_shear_force_n).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_divergent).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_wing_detached).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G spar SDOF dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let n_f = n as f64;
    let d = rows.iter().filter(|r| r.is_divergent).count();
    let det = rows.iter().filter(|r| r.is_wing_detached).count();
    println!(
        "  divergent {d} ({:.1}%)  detached {det} ({:.1}%)",
        100.0 * d as f64 / n_f,
        100.0 * det as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
