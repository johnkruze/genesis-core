//! Humanoid Bipedal Gait — Full 3D Euler Rigid Body Dynamics
//!
//! Newton-Euler: \tau = I \dot{\omega} + \omega \times (I \omega), \delta_{ZMP} = \tau_{gyro} / (M g)
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
pub const M_BODY: f64 = 80.0;          // kg — body mass (no load)
pub const M_LEG: f64 = 12.0;           // kg — single leg mass
pub const L_LEG: f64 = 0.9;            // m — leg length
pub const R_LEG: f64 = 0.06;           // m — leg radius of gyration
pub const H_COM: f64 = 1.0;            // m — nominal CoM height
pub const STRIDE_LENGTH: f64 = 0.70;   // m — stride length
pub const SUPPORT_X_HALF: f64 = 0.10;  // m — support polygon half-length (fore-aft)
pub const SUPPORT_Y_HALF: f64 = 0.12;  // m — support polygon half-width (lateral)
pub const G: f64 = 9.81;               // m/s²

const I_SWING_XX: f64 = M_LEG * L_LEG * L_LEG / 12.0;
const I_SWING_YY: f64 = M_LEG * L_LEG * L_LEG / 12.0;
const I_SWING_ZZ: f64 = M_LEG * R_LEG * R_LEG / 2.0;

#[inline]
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[derive(Debug, Serialize)]
struct Run {
    trajectory_id: u32,
    short_id: String,
    speed_ms: f64,
    load_kg: f64,
    load_height_m: f64,
    terrain_slope_deg: f64,
    min_stability_margin_m: f64,
    simplified_margin_m: f64,
    gyro_torque_norm_nm: f64,
    is_simplified_zmp_optimistic: bool,
    is_gait_failed: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let speed = rng.range(0.48, 1.00);
    let load = rng.range(0.0, 16.0);
    let load_h = rng.range(0.0, 0.35);
    let slope_deg = rng.range(0.0, 6.0);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(speed);
    proof.feed_f64(load);
    proof.feed_f64(load_h);
    proof.feed_f64(slope_deg);

    let m_total = M_BODY + load;
    let slope_rad = slope_deg.to_radians();
    let f_step = speed / (2.0 * STRIDE_LENGTH);
    let omega_swing = std::f64::consts::PI * f_step * L_LEG / (STRIDE_LENGTH / 2.0);

    let omega_swing_vec = [
        0.05 * omega_swing * rng.gaussian(1.0, 0.1),
        omega_swing,
        0.02 * omega_swing * rng.gaussian(1.0, 0.1),
    ];

    let l_swing = [
        I_SWING_XX * omega_swing_vec[0],
        I_SWING_YY * omega_swing_vec[1],
        I_SWING_ZZ * omega_swing_vec[2],
    ];

    let omega_body_y = speed / H_COM;
    let h_com_eff = (M_BODY * H_COM + load * (H_COM + load_h)) / m_total;
    let lateral_sway_amp = 0.03 * (1.0 + load * load_h / (m_total * H_COM));
    let omega_body_x = lateral_sway_amp * omega_swing + rng.gaussian(0.0, 0.01);
    let omega_body_z = 0.01 * omega_swing + rng.gaussian(0.0, 0.005);
    let omega_body = [omega_body_x, omega_body_y, omega_body_z];

    let tau_gyro = cross(omega_body, l_swing);
    let tau_norm = (tau_gyro[0] * tau_gyro[0] + tau_gyro[1] * tau_gyro[1] + tau_gyro[2] * tau_gyro[2]).sqrt();

    // Organ coupling: zmp_from_ankle_torque_m
    let delta_zmp_x = zmp_from_ankle_torque_m(tau_gyro[1], m_total * G);
    let delta_zmp_y = zmp_from_ankle_torque_m(-tau_gyro[0], m_total * G);

    let delta_zmp_simplified = h_com_eff * speed * speed / (G * L_LEG);
    let zmp_slope = slope_rad * h_com_eff;

    let dt = 0.005f64;
    let n_steps = (2.0 / dt) as usize;
    let mut min_margin = SUPPORT_Y_HALF;
    let mut failure = false;
    let mut consecutive_violations = 0i32;

    for step in 0..n_steps {
        let t = step as f64 * dt;
        let phase = (t * f_step * std::f64::consts::TAU).sin();
        let phase2 = (t * f_step * std::f64::consts::TAU * 2.0).sin();

        let zmp_nom_x = 0.05 * phase - zmp_slope;
        let zmp_nom_y = 0.06 * phase2;

        let gyro_phase = ((phase + 1.0) / 2.0).max(0.0);
        let zmp_x = zmp_nom_x + delta_zmp_x * gyro_phase * (1.0 + rng.gaussian(0.0, 0.05));
        let zmp_y = zmp_nom_y + delta_zmp_y * gyro_phase * (1.0 + rng.gaussian(0.0, 0.05));

        let margin_x = SUPPORT_X_HALF - zmp_x.abs();
        let margin_y = SUPPORT_Y_HALF - zmp_y.abs();
        let margin = margin_x.min(margin_y);

        if margin < min_margin {
            min_margin = margin;
        }

        if margin < 0.0 {
            consecutive_violations += 1;
            if consecutive_violations > 140 {
                failure = true;
                break;
            }
        } else {
            consecutive_violations = 0;
        }
    }

    let margin_simplified = SUPPORT_Y_HALF - (delta_zmp_simplified + zmp_slope).abs();

    let is_failed = failure;
    let is_optimistic = !is_failed && margin_simplified > min_margin;

    proof.feed_f64(min_margin);
    proof.feed_f64(margin_simplified);
    proof.feed_str(if is_failed {
        "GAIT_FAILED"
    } else if is_optimistic {
        "OPTIMISTIC_ZMP"
    } else {
        "STABLE"
    });

    Run {
        trajectory_id: id,
        short_id,
        speed_ms: (speed * 1000.0).round() / 1000.0,
        load_kg: (load * 100.0).round() / 100.0,
        load_height_m: (load_h * 1000.0).round() / 1000.0,
        terrain_slope_deg: (slope_deg * 100.0).round() / 100.0,
        min_stability_margin_m: (min_margin * 1000.0).round() / 1000.0,
        simplified_margin_m: (margin_simplified * 1000.0).round() / 1000.0,
        gyro_torque_norm_nm: (tau_norm * 100.0).round() / 100.0,
        is_simplified_zmp_optimistic: is_optimistic,
        is_gait_failed: is_failed,
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
                "{}/../../data/exports/sovereign/humanoid_gait_euler.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: GAIT 3D EULER DYNAMICS (zmp_from_ankle_torque_m)");
    println!("  n={n}  out={out}");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0x3d01_e71e);
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
        Field::new("terrain_slope_deg", DataType::Float64, false),
        Field::new("min_stability_margin_m", DataType::Float64, false),
        Field::new("simplified_margin_m", DataType::Float64, false),
        Field::new("gyro_torque_norm_nm", DataType::Float64, false),
        Field::new("is_simplified_zmp_optimistic", DataType::Boolean, false),
        Field::new("is_gait_failed", DataType::Boolean, false),
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
            Arc::new(Float64Array::from(rows.iter().map(|r| r.terrain_slope_deg).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.min_stability_margin_m).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.simplified_margin_m).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.gyro_torque_norm_nm).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_simplified_zmp_optimistic).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_gait_failed).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");

    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G humanoid gait euler dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let opt = rows.iter().filter(|r| r.is_simplified_zmp_optimistic).count();
    let fail = rows.iter().filter(|r| r.is_gait_failed).count();
    let neither = rows.iter().filter(|r| !r.is_simplified_zmp_optimistic && !r.is_gait_failed).count();
    println!(
        "  optimistic {opt} ({:.1}%)  gait_failed {fail} ({:.1}%)  survive {neither} ({:.1}%)",
        100.0 * opt as f64 / n_f,
        100.0 * fail as f64 / n_f,
        100.0 * neither as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
