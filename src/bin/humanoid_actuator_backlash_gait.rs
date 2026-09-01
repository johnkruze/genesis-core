//! Bipedal CoM as inverted pendulum with gearbox deadband on the ankle PD.
//! k_p > m g h or the upright is unstabilizable. Clock: 100 Hz, 8 s (10 strides).
//! Gates: |θ| ≥ 0.06 rad asymmetry vs |θ| ≥ 0.20 rad fall.
//! Organ: resonance::InvertedPendulum, backlash_deadband_torque_nm.
//! Not tribology — locomotion. TSV organ = resonance.

use genesis_core::output;
use genesis_core::physics::resonance::{
    backlash_deadband_torque_nm, pd_ankle_torque_nm, InvertedPendulum,
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
const HORIZON_S: f64 = 8.0;
const ASYM_RAD: f64 = 0.06;
const FALL_RAD: f64 = 0.20;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    gearbox_backlash_mrad: f64,
    max_tracking_error_rad: f64,
    final_com_pitch_error_rad: f64,
    is_gait_asymmetric: bool,
    is_dynamic_fall_failed: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let backlash_mrad = rng.range(0.3, 6.0);
    let mass = rng.range(58.0, 82.0);
    let h = rng.range(0.80, 0.95);
    let v_walk = rng.range(0.6, 1.6);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(backlash_mrad);
    proof.feed_f64(mass);
    proof.feed_f64(h);
    proof.feed_f64(v_walk);

    let mut plant = InvertedPendulum::new(0.02, h, mass);
    let kp = plant.mgh_nm_per_rad() * rng.range(1.15, 1.80);
    let kd = 2.0 * (kp * plant.inertia_kg_m2()).sqrt() * rng.range(0.4, 0.9);
    let tau_max = rng.range(80.0, 140.0);
    let k_series = rng.range(400.0, 1800.0);
    let tau_db = k_series * (backlash_mrad * 1e-3);

    let mut peak = plant.theta_rad.abs();
    let steps = (HORIZON_S / DT) as usize;
    for k in 0..steps {
        // Two torque reversals per 0.8 s stride: a small push at heel-strike.
        let t = k as f64 * DT;
        let stride_phase = (t / 0.80).fract();
        if stride_phase < DT / 0.80 {
            plant.omega_rad_s += 0.035 * v_walk * (1.0 + backlash_mrad / 8.0);
        }
        let cmd = pd_ankle_torque_nm(plant.theta_rad, plant.omega_rad_s, kp, kd, tau_max);
        let tau = backlash_deadband_torque_nm(cmd, tau_db);
        plant.step(tau, DT);
        let e = plant.theta_rad.abs();
        if e > peak {
            peak = e;
        }
        if peak >= FALL_RAD {
            break;
        }
        if k % 100 == 0 {
            proof.feed_f64(plant.theta_rad);
        }
    }

    let asym = peak >= ASYM_RAD;
    let fall = peak >= FALL_RAD;
    proof.feed_f64(peak);
    proof.feed_str(if fall {
        "FALL"
    } else if asym {
        "ASYMMETRIC"
    } else {
        "NOMINAL"
    });

    Run {
        id,
        short_id,
        gearbox_backlash_mrad: (backlash_mrad * 100.0).round() / 100.0,
        max_tracking_error_rad: (peak * 1000.0).round() / 1000.0,
        final_com_pitch_error_rad: (plant.theta_rad * 1000.0).round() / 1000.0,
        is_gait_asymmetric: asym,
        is_dynamic_fall_failed: fall,
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
                "{}/../../data/exports/sovereign/humanoid_actuator_backlash_gait.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: GAIT LIP + BACKLASH  (100 Hz, k_p > m g h)");
    println!("  n={n}  asym {ASYM_RAD} rad  fall {FALL_RAD} rad");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0x7187_0007);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("gearbox_backlash_mrad", DataType::Float64, false),
        Field::new("max_tracking_error_rad", DataType::Float64, false),
        Field::new("final_com_pitch_error_rad", DataType::Float64, false),
        Field::new("is_gait_asymmetric", DataType::Boolean, false),
        Field::new("is_dynamic_fall_failed", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.gearbox_backlash_mrad).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.max_tracking_error_rad).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.final_com_pitch_error_rad).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_gait_asymmetric).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_dynamic_fall_failed).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G gait LIP backlash dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let a = rows.iter().filter(|r| r.is_gait_asymmetric).count();
    let f = rows.iter().filter(|r| r.is_dynamic_fall_failed).count();
    println!(
        "  asymmetric {a} ({:.1}%)  fall {f} ({:.1}%)",
        100.0 * a as f64 / n_f,
        100.0 * f as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
