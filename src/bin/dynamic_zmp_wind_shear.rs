//! Humanoid in a gust. LIP with k_p > m g h. Wind torque ½ρv² Cd A h.
//! Not a static moment-arm clone of ankle_inversion (obstacle). Clock: 100 Hz, 1.5 s.
//! Gates: lean |θ|≥0.08 (ZMP 30 mm is the whole bank at 3–18 m/s) vs fall |θ|≥0.18.

use genesis_core::output;
use genesis_core::physics::aero::dynamic_pressure_pa;
use genesis_core::physics::resonance::{
    pd_ankle_torque_nm, zmp_from_ankle_torque_m, InvertedPendulum,
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
const DT: f64 = 0.01;
const HORIZON_S: f64 = 1.5;
const AREA: f64 = 0.65;
const CD: f64 = 1.1;
const RHO: f64 = 1.225;
const FALL: f64 = 0.18;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    wind_ms: f64,
    peak_zmp_mm: f64,
    is_zmp_thin: bool,
    is_catastrophic_fall: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let wind = rng.range(3.0, 18.0);
    let mass = rng.range(60.0, 85.0);
    let h = rng.range(0.90, 1.10);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(wind);
    proof.feed_f64(mass);

    let mut plant = InvertedPendulum::new(0.02, h, mass);
    let kp = plant.mgh_nm_per_rad() * rng.range(1.20, 1.80);
    let kd = 2.0 * (kp * plant.inertia_kg_m2()).sqrt() * rng.range(0.35, 0.80);
    let q = dynamic_pressure_pa(RHO, wind);
    let f_wind = q * CD * AREA;
    let tau_wind = f_wind * h;
    let fnorm = mass * 9.81;
    let mut peak_zmp: f64 = 0.0;
    let mut peak_th: f64 = 0.02;
    let steps = (HORIZON_S / DT) as usize;
    for k in 0..steps {
        let tau_pd = pd_ankle_torque_nm(plant.theta_rad, plant.omega_rad_s, kp, kd, 140.0);
        plant.step(tau_pd - tau_wind, DT);
        let zmp = zmp_from_ankle_torque_m(tau_pd.abs(), fnorm).abs();
        peak_zmp = peak_zmp.max(zmp);
        peak_th = peak_th.max(plant.theta_rad.abs());
        if peak_th >= FALL {
            break;
        }
        if k % 15 == 0 {
            proof.feed_f64(plant.theta_rad);
        }
    }
    let thin = peak_th >= 0.08;
    let fall = peak_th >= FALL;
    proof.feed_f64(peak_zmp);
    proof.feed_str(if fall {
        "FALL"
    } else if thin {
        "THIN"
    } else {
        "HELD"
    });

    Run {
        id,
        short_id,
        wind_ms: (wind * 10.0).round() / 10.0,
        peak_zmp_mm: (peak_zmp * 1e3 * 10.0).round() / 10.0,
        is_zmp_thin: thin,
        is_catastrophic_fall: fall,
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
                "{}/../../data/exports/sovereign/dynamic_zmp_wind_shear.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    println!("====================================================================");
    println!("  G^G: WIND ZMP  (LIP k_p>mgh, q Cd A, 100 Hz)");
    println!("  n={n}");
    println!("====================================================================\n");
    let t0 = Instant::now();
    let mut rng = Rng::new(0x2170_00D4);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("wind_ms", DataType::Float64, false),
        Field::new("peak_zmp_mm", DataType::Float64, false),
        Field::new("is_zmp_thin", DataType::Boolean, false),
        Field::new("is_catastrophic_fall", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.wind_ms).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.peak_zmp_mm).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_zmp_thin).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_catastrophic_fall).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G wind ZMP LIP dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let nf = n as f64;
    let a = rows.iter().filter(|r| r.is_zmp_thin).count();
    let b = rows.iter().filter(|r| r.is_catastrophic_fall).count();
    println!(
        "  zmp_thin {a} ({:.1}%)  fall {b} ({:.1}%)",
        100.0 * a as f64 / nf,
        100.0 * b as f64 / nf
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
