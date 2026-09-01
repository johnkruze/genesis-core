//! Lateral ankle as inverted pendulum. k_p > m g h (Gemini's k_p=350 was unstabilizable).
//! Clock: 1 kHz, 400 ms single-support. ZMP = τ / F_n.
//! Gates: peak |x_zmp| ≥ 30 mm margin-thin vs |θ| ≥ 0.20 rad inversion fall.
//! Organ: resonance::InvertedPendulum, pd_ankle_torque_nm, zmp_from_ankle_torque_m.

use genesis_core::output;
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
const DT: f64 = 0.001;
const STANCE_S: f64 = 0.40;
const FOOT_HALF_M: f64 = 0.045;
const ZMP_THIN_M: f64 = 0.030;
const FALL_THETA: f64 = 0.20;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    lateral_obstacle_height_mm: f64,
    peak_lateral_zmp_offset_mm: f64,
    ankle_roll_angle_deg: f64,
    is_support_margin_degraded: bool,
    is_ankle_inversion_fall: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let h_obs_mm = rng.range(2.0, 16.0);
    let mass = rng.range(58.0, 85.0);
    let com_h = rng.range(0.80, 0.92);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(h_obs_mm);
    proof.feed_f64(mass);
    proof.feed_f64(com_h);

    let theta0 = (h_obs_mm * 1e-3 / (2.0 * FOOT_HALF_M)).asin();
    let mut plant = InvertedPendulum::new(theta0, com_h, mass);
    let kp = plant.mgh_nm_per_rad() * rng.range(1.20, 1.90);
    let kd = 2.0 * (kp * plant.inertia_kg_m2()).sqrt() * rng.range(0.35, 0.80);
    let tau_max = rng.range(90.0, 140.0);
    let fnorm = mass * 9.81;

    let mut peak_zmp = 0.0;
    let mut peak_theta = theta0.abs();
    let steps = (STANCE_S / DT) as usize;
    for k in 0..steps {
        let tau = pd_ankle_torque_nm(plant.theta_rad, plant.omega_rad_s, kp, kd, tau_max);
        let zmp = zmp_from_ankle_torque_m(tau.abs(), fnorm).abs();
        if zmp > peak_zmp {
            peak_zmp = zmp;
        }
        plant.step(tau, DT);
        let th = plant.theta_rad.abs();
        if th > peak_theta {
            peak_theta = th;
        }
        if th >= FALL_THETA {
            break;
        }
        if k % 50 == 0 {
            proof.feed_f64(plant.theta_rad);
        }
    }

    let thin = peak_theta >= 0.10;
    let fall = peak_theta >= FALL_THETA;
    proof.feed_f64(peak_zmp);
    proof.feed_f64(plant.theta_rad);
    proof.feed_str(if fall {
        "INVERSION_FALL"
    } else if thin {
        "THIN_MARGIN"
    } else {
        "NOMINAL"
    });

    Run {
        id,
        short_id,
        lateral_obstacle_height_mm: (h_obs_mm * 10.0).round() / 10.0,
        peak_lateral_zmp_offset_mm: (peak_zmp * 1e3 * 10.0).round() / 10.0,
        ankle_roll_angle_deg: (plant.theta_rad.to_degrees() * 10.0).round() / 10.0,
        is_support_margin_degraded: thin,
        is_ankle_inversion_fall: fall,
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
                "{}/../../data/exports/sovereign/ankle_inversion_trip.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: ANKLE LIP  (1 kHz stance, k_p > m g h)");
    println!("  n={n}  ZMP thin {ZMP_THIN_M} m  fall |θ|≥{FALL_THETA} rad");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0x7188_0008);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("lateral_obstacle_height_mm", DataType::Float64, false),
        Field::new("peak_lateral_zmp_offset_mm", DataType::Float64, false),
        Field::new("ankle_roll_angle_deg", DataType::Float64, false),
        Field::new("is_support_margin_degraded", DataType::Boolean, false),
        Field::new("is_ankle_inversion_fall", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.lateral_obstacle_height_mm).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.peak_lateral_zmp_offset_mm).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.ankle_roll_angle_deg).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_support_margin_degraded).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_ankle_inversion_fall).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G ankle LIP dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let d = rows.iter().filter(|r| r.is_support_margin_degraded).count();
    let f = rows.iter().filter(|r| r.is_ankle_inversion_fall).count();
    println!(
        "  zmp_thin {d} ({:.1}%)  fall {f} ({:.1}%)",
        100.0 * d as f64 / n_f,
        100.0 * f as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
