//! G^G Humanoid Coupled Walk + Grasp Trajectory Monte Carlo
//!
//! Shared 1000Hz clock coupling locomotion heel-strike chatter (resonance)
//! with end-effector tactile slip dynamics (dexterous).
//! Sovereign Receipt n=2500 Dual-Regime Parquet.

use arrow::array::{BooleanArray, Float64Array, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use genesis_core::output;
use genesis_core::physics::dexterous::{
    evaluate_grasp_dynamics, C_GraspState, C_TactileArray, Taxel,
};
use genesis_core::physics::resonance::{zmp_from_ankle_torque_m, DynamicOscillator};
use genesis_core::proof::{self, ProofChain};
use genesis_core::rng::Rng;
use parquet::arrow::arrow_writer::ArrowWriter;
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;

const DEFAULT_N: usize = 2500;
pub const ROBOT_MASS_KG: f64 = 75.0;
pub const GRAVITY: f64 = 9.81;
pub const SUPPORT_Y_HALF: f64 = 0.12; // m

#[derive(Debug, Serialize)]
struct Run {
    trajectory_id: u32,
    short_id: String,
    speed_ms: f64,
    object_mass_kg: f64,
    static_friction_mu: f64,
    peak_chatter_disp_mm: f64,
    min_zmp_margin_m: f64,
    peak_grasp_slip_vel_ms: f64,
    is_chatter_or_thin_zmp: bool,
    is_grasp_macro_slip: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let speed = rng.range(0.6, 2.8);
    let payload_mass = rng.range(0.2, 3.2) as f32;
    let mu_s = rng.range(0.18, 0.80) as f32;
    let initial_grip_n = rng.range(8.0, 30.0) as f32;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(speed);
    proof.feed_f64(payload_mass as f64);
    proof.feed_f64(mu_s as f64);

    let dt = 0.001f64;
    let dt_f32 = dt as f32;
    let steps = 1000;

    let mut chatter_osc = DynamicOscillator::new(14.0, 0.04);
    let mut min_zmp_margin = SUPPORT_Y_HALF;
    let mut peak_chatter_m = 0.0f64;

    let mut grasp_state = C_GraspState {
        normal_force: initial_grip_n,
        slip_velocity: 0.0,
        slip_angular_velocity: 0.0,
        object_mass: payload_mass,
        static_friction_coeff: mu_s,
        dynamic_friction_coeff: mu_s * 0.8,
        reflex_active: false,
    };

    let mut macro_slip_occurred = false;
    let mut peak_slip_vel = 0.0f32;

    for step in 0..steps {
        let t = step as f64 * dt;
        
        let heel_strike = if (t - 0.40).abs() < dt / 2.0 || (t - 0.90).abs() < dt / 2.0 {
            25.0 * speed
        } else {
            0.0
        };

        let (disp, acc) = chatter_osc.step(heel_strike, dt);
        if disp.abs() > peak_chatter_m {
            peak_chatter_m = disp.abs();
        }

        let ankle_torque = 45.0 * (t * 2.0 * std::f64::consts::PI).sin() + acc * 0.5;
        let zmp_x = zmp_from_ankle_torque_m(ankle_torque, ROBOT_MASS_KG * GRAVITY);
        let margin = SUPPORT_Y_HALF - zmp_x.abs();
        if margin < min_zmp_margin {
            min_zmp_margin = margin;
        }

        let shock_shear = (heel_strike * payload_mass as f64 * 0.22) as f32;
        let base_shear = (payload_mass * 9.81 / 16.0) as f32;
        let taxel_shear = base_shear + shock_shear / 16.0;

        let taxels = [Taxel {
            normal: grasp_state.normal_force / 16.0,
            shear_x: taxel_shear,
            shear_y: taxel_shear * 0.2,
        }; 16];
        let sensor = C_TactileArray { taxels };

        let grasp_res = evaluate_grasp_dynamics(&sensor, &mut grasp_state, dt_f32);
        if grasp_res.macro_slip_detected {
            macro_slip_occurred = true;
        }
        if grasp_state.slip_velocity > peak_slip_vel {
            peak_slip_vel = grasp_state.slip_velocity;
        }

        if step % 25 == 0 {
            proof.feed_f64(disp);
            proof.feed_f64(margin);
            proof.feed_f64(grasp_state.slip_velocity as f64);
        }
    }

    let is_chatter_thin = peak_chatter_m > 0.00045 || min_zmp_margin < 0.040;
    let is_macro_slip = macro_slip_occurred;

    proof.feed_f64(min_zmp_margin);
    proof.feed_f64(peak_slip_vel as f64);
    proof.feed_str(if is_chatter_thin && is_macro_slip {
        "CHATTER_AND_MACRO_SLIP"
    } else if is_macro_slip {
        "GRASP_MACRO_SLIP"
    } else if is_chatter_thin {
        "CHATTER_THIN_ZMP"
    } else {
        "STABLE_COUPLED"
    });

    Run {
        trajectory_id: id,
        short_id,
        speed_ms: (speed * 1000.0).round() / 1000.0,
        object_mass_kg: (payload_mass as f64 * 1000.0).round() / 1000.0,
        static_friction_mu: (mu_s as f64 * 1000.0).round() / 1000.0,
        peak_chatter_disp_mm: (peak_chatter_m * 1000.0 * 10.0).round() / 10.0,
        min_zmp_margin_m: (min_zmp_margin * 1000.0).round() / 1000.0,
        peak_grasp_slip_vel_ms: (peak_slip_vel as f64 * 1000.0).round() / 1000.0,
        is_chatter_or_thin_zmp: is_chatter_thin,
        is_grasp_macro_slip: is_macro_slip,
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
                "{}/../../data/exports/sovereign/humanoid_walk_grasp_coupled.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: HUMANOID COUPLED WALK + GRASP (resonance + dexterous)");
    println!("  n={n}  out={out}");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0x7761_6c6b_6772_6173);
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
        Field::new("object_mass_kg", DataType::Float64, false),
        Field::new("static_friction_mu", DataType::Float64, false),
        Field::new("peak_chatter_disp_mm", DataType::Float64, false),
        Field::new("min_zmp_margin_m", DataType::Float64, false),
        Field::new("peak_grasp_slip_vel_ms", DataType::Float64, false),
        Field::new("is_chatter_or_thin_zmp", DataType::Boolean, false),
        Field::new("is_grasp_macro_slip", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.trajectory_id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.speed_ms).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.object_mass_kg).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.static_friction_mu).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.peak_chatter_disp_mm).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.min_zmp_margin_m).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.peak_grasp_slip_vel_ms).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_chatter_or_thin_zmp).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_grasp_macro_slip).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");

    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G humanoid walk grasp coupled dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let chatter = rows.iter().filter(|r| r.is_chatter_or_thin_zmp).count();
    let macro_slip = rows.iter().filter(|r| r.is_grasp_macro_slip).count();
    println!(
        "  chatter_or_thin_zmp {chatter} ({:.1}%)  grasp_macro_slip {macro_slip} ({:.1}%)",
        100.0 * chatter as f64 / n_f,
        100.0 * macro_slip as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
