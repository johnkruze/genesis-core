//! G^G Bipedal Whole-Body Lift Collapse Monte Carlo
//!
//! Inverted Pendulum Slip vs Fluctuating Warehouse Friction.
//! Organ: resonance (InvertedPendulum).
//! Sovereign Receipt n=2500 Dual-Regime Parquet.

use arrow::array::{BooleanArray, Float64Array, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use genesis_core::output;
use genesis_core::physics::resonance::{pd_ankle_torque_nm, InvertedPendulum};
use genesis_core::proof::{self, ProofChain};
use genesis_core::rng::Rng;
use parquet::arrow::arrow_writer::ArrowWriter;
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;

const DEFAULT_N: usize = 2500;
pub const GRAVITY: f64 = 9.81;

// Named constants (OEM custom-run parameters)
pub const HUMANOID_MASS_KG: f64 = 50.0;
pub const PACKAGE_MASS_KG: f64 = 20.0;
pub const TOTAL_MASS: f64 = HUMANOID_MASS_KG + PACKAGE_MASS_KG;
pub const CENTER_OF_GRAVITY_HEIGHT: f64 = 0.85; // m
pub const FEET_FORWARD_OFFSET: f64 = 0.3; // m
pub const HEAVE_ACCEL_MS2: f64 = 4.2; // OEM lever — continuous 4.5 glued dust to collapse
pub const SLIP_WARN_M: f64 = 0.02;
pub const COLLAPSE_THETA_RAD: f64 = 0.45;
pub const K_DIST: f64 = 1.85;

#[derive(Debug, Clone, Copy, PartialEq)]
enum FailureMode {
    VirtualCleanSim,       // Idealized lab concrete (Mu = 0.8)
    NominalWarehouse,      // Standard clean warehouse floor (Mu = 0.6)
    MicroDustAccumulation, // Cardboard dust drops friction (Mu = 0.35)
}

#[derive(Debug, Serialize)]
struct Run {
    trajectory_id: u32,
    short_id: String,
    true_static_mu: f64,
    true_kinetic_mu: f64,
    foot_slip_distance_m: f64,
    final_pitch_deg: f64,
    is_kinetic_slip: bool,
    is_pendulum_collapse: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);

    let failure = if rng.chance(0.15) {
        FailureMode::VirtualCleanSim
    } else if rng.chance(0.35) {
        FailureMode::NominalWarehouse
    } else {
        FailureMode::MicroDustAccumulation
    };

    let true_static_mu = match failure {
        FailureMode::VirtualCleanSim => rng.range(0.75, 0.85),
        FailureMode::NominalWarehouse => rng.range(0.55, 0.65),
        FailureMode::MicroDustAccumulation => rng.range(0.38, 0.52),
    };

    let true_kinetic_mu = true_static_mu * 0.6;
    let heave_pulse_s = rng.range(0.30, 0.90);
    let dt = 0.001;
    let max_time_s = 4.0;
    let max_steps = (max_time_s / dt) as usize;
    let lift_initiation_time = 1.0;

    let mut is_slipping = false;
    let mut foot_slip_velocity = 0.0;
    let mut foot_slip_distance = 0.0;

    // Organ integration: InvertedPendulum
    let mut pendulum = InvertedPendulum::new(0.0, CENTER_OF_GRAVITY_HEIGHT, TOTAL_MASS);
    let tau_max = pendulum.mgh_nm_per_rad() * 1.8;
    let kp = pendulum.mgh_nm_per_rad() * 1.85;
    let kd = 2.0 * (kp * pendulum.inertia_kg_m2()).sqrt() * 0.55;
    let mut outcome = "TIMEOUT_ERROR";

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(TOTAL_MASS);
    proof.feed_f64(true_static_mu);

    for step in 0..max_steps {
        let t = step as f64 * dt;
        let total_normal_force = TOTAL_MASS * GRAVITY;
        let mut commanded_shear_force = 0.0;

        if t >= lift_initiation_time && t < lift_initiation_time + heave_pulse_s {
            commanded_shear_force = TOTAL_MASS * HEAVE_ACCEL_MS2;
        }

        if !is_slipping && commanded_shear_force > 0.0 {
            let structural_grip = total_normal_force * true_static_mu;
            if commanded_shear_force > structural_grip {
                is_slipping = true;
            }
        }

        if is_slipping {
            let kinetic_grip = total_normal_force * true_kinetic_mu;
            let net_sliding_force = commanded_shear_force - kinetic_grip;
            if net_sliding_force > 0.0 {
                let foot_accel = net_sliding_force / TOTAL_MASS;
                foot_slip_velocity += foot_accel * dt;
                foot_slip_distance += foot_slip_velocity * dt;
            }
        }

        let restore = pd_ankle_torque_nm(pendulum.theta_rad, pendulum.omega_rad_s, kp, kd, tau_max);
        if is_slipping && foot_slip_distance > SLIP_WARN_M {
            let disturbance = TOTAL_MASS * GRAVITY * foot_slip_distance * K_DIST;
            pendulum.step(restore - disturbance, dt);
        } else {
            pendulum.step(restore, dt);
        }

        if pendulum.theta_rad.abs() >= COLLAPSE_THETA_RAD {
            outcome = "PENDULUM_COLLAPSE";
            break;
        }

        if t > 3.0 {
            outcome = "LIFT_HELD";
            break;
        }

        if step % 200 == 0 {
            proof.feed_f64(foot_slip_distance);
            proof.feed_f64(pendulum.theta_rad);
        }
    }

    let is_collapse = outcome == "PENDULUM_COLLAPSE";
    let slipped = foot_slip_distance >= SLIP_WARN_M;

    proof.feed_f64(foot_slip_distance);
    proof.feed_f64(pendulum.theta_rad);
    proof.feed_str(outcome);

    Run {
        trajectory_id: id,
        short_id,
        true_static_mu: (true_static_mu * 1000.0).round() / 1000.0,
        true_kinetic_mu: (true_kinetic_mu * 1000.0).round() / 1000.0,
        foot_slip_distance_m: (foot_slip_distance * 1000.0).round() / 1000.0,
        final_pitch_deg: (pendulum.theta_rad.to_degrees() * 10.0).round() / 10.0,
        is_kinetic_slip: slipped,
        is_pendulum_collapse: is_collapse,
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
                "{}/../../data/exports/sovereign/humanoid_bipedal_lift.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: BIPEDAL WHOLE-BODY LIFT COLLAPSE (resonance InvertedPendulum)");
    println!("  n={n}  out={out}");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0xdead_beef_c0fe_1337);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("true_static_mu", DataType::Float64, false),
        Field::new("true_kinetic_mu", DataType::Float64, false),
        Field::new("foot_slip_distance_m", DataType::Float64, false),
        Field::new("final_pitch_deg", DataType::Float64, false),
        Field::new("is_kinetic_slip", DataType::Boolean, false),
        Field::new("is_pendulum_collapse", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.trajectory_id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.true_static_mu).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.true_kinetic_mu).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.foot_slip_distance_m).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.final_pitch_deg).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_kinetic_slip).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_pendulum_collapse).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");

    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G humanoid bipedal lift dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let slip = rows.iter().filter(|r| r.is_kinetic_slip).count();
    let collapse = rows.iter().filter(|r| r.is_pendulum_collapse).count();
    let only_slip = rows.iter().filter(|r| r.is_kinetic_slip && !r.is_pendulum_collapse).count();
    let only_fall = rows.iter().filter(|r| !r.is_kinetic_slip && r.is_pendulum_collapse).count();
    let both = rows.iter().filter(|r| r.is_kinetic_slip && r.is_pendulum_collapse).count();
    let neither = rows.iter().filter(|r| !r.is_kinetic_slip && !r.is_pendulum_collapse).count();
    println!(
        "  kinetic_slip {slip} ({:.1}%)  pendulum_collapse {collapse} ({:.1}%)",
        100.0 * slip as f64 / n_f,
        100.0 * collapse as f64 / n_f
    );
    println!("  four-cell only_slip={only_slip} only_collapse={only_fall} both={both} neither={neither}");
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
