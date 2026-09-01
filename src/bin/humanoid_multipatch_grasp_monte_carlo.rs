//! G^G Humanoid Multi-Patch Fingertip Grasp Monte Carlo
//!
//! N=5 Contact Patches over Shared Friction Cone.
//! Organ: dexterous (evaluate_multi_patch_grasp, FingerPatch).
//! Sovereign Receipt n=2500 Dual-Regime Parquet.

use arrow::array::{BooleanArray, Float64Array, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use genesis_core::output;
use genesis_core::physics::dexterous::{
    evaluate_multi_patch_grasp, C_GraspState, C_TactileArray, FingerPatch, Taxel,
    GRASP_CLAMP_N, N_FINGER_PATCHES,
};
use genesis_core::proof::{self, ProofChain};
use genesis_core::rng::Rng;
use parquet::arrow::arrow_writer::ArrowWriter;
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;

const DEFAULT_N: usize = 2500;

#[derive(Debug, Serialize)]
struct Run {
    trajectory_id: u32,
    short_id: String,
    object_mass_kg: f64,
    static_friction_mu: f64,
    initial_normal_force_n: f64,
    final_commanded_force_n: f64,
    slip_velocity_m_s: f64,
    tactile_friction_margin: f64,
    micro_slip_detected: bool,
    macro_slip_detected: bool,
    rotational_slip_detected: bool,
    reflex_clamped_safe: bool,
    halt_ms: f64,
    halt_within_2ms: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let mass_kg = rng.range(0.02, 3.5) as f32;
    let mu_s = rng.range(0.10, 0.85) as f32;
    let initial_force = rng.range(4.0, 32.0) as f32;
    let disturbances = rng.range(0.0, 1.2) as f32;

    let mut state = C_GraspState {
        normal_force: initial_force,
        slip_velocity: 0.0,
        slip_angular_velocity: 0.0,
        object_mass: mass_kg,
        static_friction_coeff: mu_s,
        dynamic_friction_coeff: mu_s * 0.8,
        reflex_active: false,
    };

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(mass_kg as f64);
    proof.feed_f64(mu_s as f64);
    proof.feed_f64(initial_force as f64);

    let dt = 0.001f32;
    let duration_s = 0.100f32;
    let total_steps = ((duration_s / dt).round() as usize).max(1);

    let mut micro_detected = false;
    let mut macro_detected = false;
    let mut rotational_detected = false;
    let mut final_margin = 0.0f32;
    let mut t_first_micro = f64::NAN;
    let mut t_reflex = f64::NAN;
    let mut t_arrest = f64::NAN;
    let mut prev_slip = state.slip_velocity;

    for step in 0..total_steps {
        let t_ms = step as f64 * dt as f64 * 1000.0;
        let normal_per_taxel = state.normal_force / (16.0 * N_FINGER_PATCHES as f32);
        let shear_load = (mass_kg * 9.81 + disturbances * (step as f32 * 0.1)) / (16.0 * N_FINGER_PATCHES as f32);

        let mut patches = [FingerPatch {
            array: C_TactileArray {
                taxels: [Taxel {
                    normal: normal_per_taxel,
                    shear_x: 0.0,
                    shear_y: 0.0,
                }; 16],
            },
            com_offset_m: [0.0, 0.0, 0.0],
        }; N_FINGER_PATCHES];

        for p in 0..N_FINGER_PATCHES {
            let offset_x = (p as f32 - 2.0) * 0.02;
            patches[p].com_offset_m = [offset_x, 0.0, 0.0];
            for i in 0..16 {
                patches[p].array.taxels[i].shear_x = shear_load * (1.0 + 0.08 * (i as f32 % 4.0) + 0.05 * p as f32);
                patches[p].array.taxels[i].shear_y = shear_load * 0.12 * (i as f32 / 4.0);
            }
        }

        let res = evaluate_multi_patch_grasp(&patches, &mut state, dt);

        if res.micro_slip_detected {
            micro_detected = true;
        }
        if res.macro_slip_detected {
            macro_detected = true;
        }
        if res.rotational_slip_detected {
            rotational_detected = true;
        }
        final_margin = res.margin;

        if t_first_micro.is_nan()
            && (res.micro_slip_detected || res.macro_slip_detected || res.rotational_slip_detected)
        {
            t_first_micro = t_ms;
        }
        if t_reflex.is_nan() && state.reflex_active {
            t_reflex = t_ms;
        }
        if t_first_micro.is_finite()
            && t_arrest.is_nan()
            && state.slip_velocity <= prev_slip + 1e-9
            && state.slip_velocity < 0.02
            && !res.macro_slip_detected
        {
            t_arrest = t_ms;
        }
        prev_slip = state.slip_velocity;

        if step % 25 == 0 {
            proof.feed_f64(state.normal_force as f64);
            proof.feed_f64(res.margin as f64);
            proof.feed_f64(state.slip_velocity as f64);
        }
    }

    let reflex_safe = final_margin > 0.15 && state.normal_force <= GRASP_CLAMP_N && !macro_detected;
    let halt_ms = if t_first_micro.is_finite() && t_arrest.is_finite() {
        (t_arrest - t_first_micro).max(0.0)
    } else {
        -1.0
    };
    let halt_within_2ms = halt_ms >= 0.0 && halt_ms <= 2.0;

    proof.feed_f64(state.normal_force as f64);
    proof.feed_f64(state.slip_velocity as f64);
    proof.feed_f64(halt_ms);
    proof.feed_str(if reflex_safe {
        "REFLEX_SAFE"
    } else if halt_within_2ms {
        "HALT_2MS"
    } else {
        "SLIP_FAIL"
    });

    Run {
        trajectory_id: id,
        short_id,
        object_mass_kg: (mass_kg as f64 * 1000.0).round() / 1000.0,
        static_friction_mu: (mu_s as f64 * 1000.0).round() / 1000.0,
        initial_normal_force_n: (initial_force as f64 * 10.0).round() / 10.0,
        final_commanded_force_n: (state.normal_force as f64 * 10.0).round() / 10.0,
        slip_velocity_m_s: (state.slip_velocity as f64 * 1000.0).round() / 1000.0,
        tactile_friction_margin: (final_margin as f64 * 1000.0).round() / 1000.0,
        micro_slip_detected: micro_detected,
        macro_slip_detected: macro_detected,
        rotational_slip_detected: rotational_detected,
        reflex_clamped_safe: reflex_safe,
        halt_ms: (halt_ms * 100.0).round() / 100.0,
        halt_within_2ms,
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
                "{}/../../data/exports/sovereign/humanoid_multipatch_grasp.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: HUMANOID MULTI-PATCH GRASP (dexterous N_FINGER_PATCHES=5)");
    println!("  n={n}  out={out}");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0x4d75_6c74_6950_6174);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("object_mass_kg", DataType::Float64, false),
        Field::new("static_friction_mu", DataType::Float64, false),
        Field::new("initial_normal_force_n", DataType::Float64, false),
        Field::new("final_commanded_force_n", DataType::Float64, false),
        Field::new("slip_velocity_m_s", DataType::Float64, false),
        Field::new("tactile_friction_margin", DataType::Float64, false),
        Field::new("micro_slip_detected", DataType::Boolean, false),
        Field::new("macro_slip_detected", DataType::Boolean, false),
        Field::new("rotational_slip_detected", DataType::Boolean, false),
        Field::new("reflex_clamped_safe", DataType::Boolean, false),
        Field::new("halt_ms", DataType::Float64, false),
        Field::new("halt_within_2ms", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.trajectory_id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.object_mass_kg).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.static_friction_mu).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.initial_normal_force_n).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.final_commanded_force_n).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.slip_velocity_m_s).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.tactile_friction_margin).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.micro_slip_detected).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.macro_slip_detected).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.rotational_slip_detected).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.reflex_clamped_safe).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.halt_ms).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.halt_within_2ms).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");

    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G humanoid multipatch grasp dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let safe = rows.iter().filter(|r| r.reflex_clamped_safe).count();
    let halt2 = rows.iter().filter(|r| r.halt_within_2ms).count();
    println!(
        "  reflex_clamped_safe {safe} ({:.1}%)  halt_within_2ms {halt2} ({:.1}%)",
        100.0 * safe as f64 / n_f,
        100.0 * halt2 as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
