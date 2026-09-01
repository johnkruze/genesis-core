//! Humanoid Bipedal Gait — Yaw Angular Momentum Accumulation
//!
//! \tau_z = \Omega_x \times L_y, N_{crit} = \tau_{friction\_max} / \tau_z
//! Organ: resonance (zmp_from_ankle_torque_m).
//! Sovereign Receipt n=2500 Dual-Regime Parquet.

use arrow::array::{BooleanArray, Float64Array, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use genesis_core::output;
use genesis_core::physics::resonance::zmp_from_ankle_torque_m;
use genesis_core::proof::{self, ProofChain};
use genesis_core::rng::Rng;
use parquet::arrow::arrow_writer::ArrowWriter;
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;

const DEFAULT_N: usize = 2500;

// Named constants (OEM custom-run geometry)
pub const M_BODY: f64 = 80.0;     // kg — body mass
pub const M_LEG: f64 = 12.0;      // kg — single leg mass
pub const L_LEG: f64 = 0.9;       // m — leg length
pub const STRIDE_L: f64 = 0.70;   // m — stride length
pub const R_ANKLE: f64 = 0.08;    // m — ankle moment arm for friction torque
pub const G: f64 = 9.81;          // m/s²
pub const I_BODY_Z: f64 = 18.0;   // kg·m² — body yaw inertia
pub const ZMP_BLIND_THRESHOLD_M: f64 = 0.00085; // m — yaw ZMP the sagittal controller ignores

const I_SWING_YY: f64 = M_LEG * L_LEG * L_LEG / 12.0;

#[derive(Debug, Serialize)]
struct Run {
    trajectory_id: u32,
    short_id: String,
    speed_ms: f64,
    load_kg: f64,
    load_height_m: f64,
    friction_coeff: f64,
    tau_z_nm: f64,
    tau_friction_max_nm: f64,
    critical_step_count: f64,
    gait_distance_to_fall_m: f64,
    is_zmp_blind: bool,
    is_short_walk_fall: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let speed = rng.range(0.6, 2.0);
    let load = rng.range(0.0, 40.0);
    let load_h = rng.range(0.0, 0.50);
    let friction = rng.range(0.25, 0.85);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(speed);
    proof.feed_f64(load);
    proof.feed_f64(load_h);
    proof.feed_f64(friction);

    let m_total = M_BODY + load;
    let f_step = speed / (2.0 * STRIDE_L);

    let omega_swing_y = std::f64::consts::PI * f_step * L_LEG / (STRIDE_L / 2.0);
    let h_com_eff = (M_BODY * 1.0 + load * (1.0 + load_h)) / m_total;
    let omega_body_x = 0.08 * speed / L_LEG * (1.0 + load * load_h / (m_total * h_com_eff))
        + rng.gaussian(0.0, 0.01);

    let l_y = I_SWING_YY * omega_swing_y;
    let tau_z = (omega_body_x * l_y).abs();
    let tau_friction = friction * m_total * G * R_ANKLE;

    let n_crit = if tau_z > 0.001 {
        (tau_friction / tau_z).min(10000.0)
    } else {
        10000.0
    };

    // Organ coupling: zmp_from_ankle_torque_m
    let zmp_from_yaw = zmp_from_ankle_torque_m(tau_z, m_total * G);
    let is_zmp_blind = zmp_from_yaw.abs() < ZMP_BLIND_THRESHOLD_M;

    let gait_dist = (n_crit * STRIDE_L).min(10000.0);
    // Hard: fall inside a short walk. Do not OR with a 105 m distance cap (that ate survive).
    let is_short_walk = n_crit < 40.0;

    proof.feed_f64(tau_z);
    proof.feed_f64(n_crit);
    proof.feed_str(if is_short_walk && is_zmp_blind {
        "BLIND_SHORT_FALL"
    } else if is_short_walk {
        "SHORT_FALL"
    } else if is_zmp_blind {
        "ZMP_BLIND"
    } else {
        "NOMINAL"
    });

    Run {
        trajectory_id: id,
        short_id,
        speed_ms: (speed * 1000.0).round() / 1000.0,
        load_kg: (load * 100.0).round() / 100.0,
        load_height_m: (load_h * 1000.0).round() / 1000.0,
        friction_coeff: (friction * 1000.0).round() / 1000.0,
        tau_z_nm: (tau_z * 1000.0).round() / 1000.0,
        tau_friction_max_nm: (tau_friction * 100.0).round() / 100.0,
        critical_step_count: (n_crit * 10.0).round() / 10.0,
        gait_distance_to_fall_m: (gait_dist * 10.0).round() / 10.0,
        is_zmp_blind,
        is_short_walk_fall: is_short_walk,
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
                "{}/../../data/exports/sovereign/humanoid_yaw_accumulation.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: HUMANOID YAW ACCUMULATION (zmp_from_ankle_torque_m)");
    println!("  n={n}  out={out}");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0x7a77_a001);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("speed_ms", DataType::Float64, false),
        Field::new("load_kg", DataType::Float64, false),
        Field::new("load_height_m", DataType::Float64, false),
        Field::new("friction_coeff", DataType::Float64, false),
        Field::new("tau_z_nm", DataType::Float64, false),
        Field::new("tau_friction_max_nm", DataType::Float64, false),
        Field::new("critical_step_count", DataType::Float64, false),
        Field::new("gait_distance_to_fall_m", DataType::Float64, false),
        Field::new("is_zmp_blind", DataType::Boolean, false),
        Field::new("is_short_walk_fall", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.trajectory_id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.speed_ms).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.load_kg).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.load_height_m).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.friction_coeff).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.tau_z_nm).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.tau_friction_max_nm).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.critical_step_count).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.gait_distance_to_fall_m).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_zmp_blind).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_short_walk_fall).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");

    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G humanoid yaw accumulation dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let blind = rows.iter().filter(|r| r.is_zmp_blind).count();
    let short_fall = rows.iter().filter(|r| r.is_short_walk_fall).count();
    println!(
        "  zmp_blind {blind} ({:.1}%)  short_walk_fall {short_fall} ({:.1}%)",
        100.0 * blind as f64 / n_f,
        100.0 * short_fall as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
