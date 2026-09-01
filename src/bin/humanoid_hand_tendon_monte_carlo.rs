//! G^G Humanoid Hand Tendon Monte Carlo
//!
//! Serial phalanges + tendon place FingerPatches into the shared friction cone.
//! Dual-regime: tendon overstretch (warn) vs pad slip (hard).
//! Organ: dexterous. Sovereign receipt n=2500.

use arrow::array::{BooleanArray, Float64Array, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use genesis_core::output;
use genesis_core::physics::dexterous::{
    evaluate_hand_tendon_dynamics, C_HandTendonState, N_HAND_FINGERS, THUMB_OPPOSITION_RAD,
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
    object_span_m: f64,
    opposition_rad: f64,
    commanded_close_rad: f64,
    tendon_tension_n: f64,
    tendon_strain: f64,
    pad_normal_n: f64,
    tactile_friction_margin: f64,
    slip_velocity_m_s: f64,
    tendon_overstretch: bool,
    pad_slip: bool,
    halt_ms: f64,
    proof_hash: String,
}

fn blank_hand(
    mass_kg: f32,
    mu_s: f32,
    span_m: f32,
    opposition: f32,
    close_rad: f32,
) -> C_HandTendonState {
    C_HandTendonState {
        q_mcp: [0.06; N_HAND_FINGERS],
        q_pip: [0.04; N_HAND_FINGERS],
        q_dip: [0.03; N_HAND_FINGERS],
        qdot_mcp: [0.0; N_HAND_FINGERS],
        qdot_pip: [0.0; N_HAND_FINGERS],
        qdot_dip: [0.0; N_HAND_FINGERS],
        tendon_stretch_m: 0.0,
        tendon_tension_n: 0.0,
        opposition_rad: opposition,
        object_span_m: span_m,
        commanded_close_rad: close_rad,
        pad_normal_n: 0.0,
        normal_force: 4.0,
        slip_velocity: 0.0,
        slip_angular_velocity: 0.0,
        object_mass: mass_kg,
        static_friction_coeff: mu_s,
        dynamic_friction_coeff: mu_s * 0.8,
        reflex_active: false,
    }
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let mass_kg = rng.range(0.05, 2.6) as f32;
    let mu_s = rng.range(0.10, 0.82) as f32;
    let span_m = rng.range(0.018, 0.078) as f32;
    let opposition = rng.range(0.35, 1.35) as f32;
    let close0 = rng.range(0.32, 1.55) as f32;
    let disturb = rng.range(0.0, 1.15) as f32;

    let mut state = blank_hand(mass_kg, mu_s, span_m, opposition, close0);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(mass_kg as f64);
    proof.feed_f64(mu_s as f64);
    proof.feed_f64(span_m as f64);
    proof.feed_f64(opposition as f64);
    proof.feed_f64(close0 as f64);

    let dt = 0.001f32;
    let total_steps = 100usize;

    let mut overstretch = false;
    let mut pad_slip = false;
    let mut final_margin = 0.0f32;
    let mut final_strain = 0.0f32;
    let mut t_first_slip = f64::NAN;
    let mut t_arrest = f64::NAN;
    let mut prev_slip = state.slip_velocity;

    for step in 0..total_steps {
        let t_ms = step as f64 * dt as f64 * 1000.0;
        state.commanded_close_rad = close0 + disturb * (step as f32) * 0.004;
        let res = evaluate_hand_tendon_dynamics(&mut state, dt);
        if res.tendon_overstretch {
            overstretch = true;
        }
        if res.pad_slip {
            pad_slip = true;
        }
        final_margin = res.margin;
        final_strain = res.strain;

        if t_first_slip.is_nan() && res.pad_slip {
            t_first_slip = t_ms;
        }
        if t_first_slip.is_finite()
            && t_arrest.is_nan()
            && state.slip_velocity <= prev_slip + 1e-9
            && state.slip_velocity < 0.02
            && !res.pad_slip
        {
            t_arrest = t_ms;
        }
        prev_slip = state.slip_velocity;

        if step % 25 == 0 {
            proof.feed_f64(state.tendon_tension_n as f64);
            proof.feed_f64(res.margin as f64);
            proof.feed_f64(state.slip_velocity as f64);
            proof.feed_f64(res.strain as f64);
        }
    }

    let halt_ms = if t_first_slip.is_finite() && t_arrest.is_finite() {
        (t_arrest - t_first_slip).max(0.0)
    } else {
        -1.0
    };

    proof.feed_f64(state.tendon_tension_n as f64);
    proof.feed_f64(state.normal_force as f64);
    proof.feed_f64(halt_ms);
    proof.feed_str(if overstretch && pad_slip {
        "BOTH"
    } else if overstretch {
        "OVERSTRETCH"
    } else if pad_slip {
        "PAD_SLIP"
    } else {
        "HOLD"
    });

    Run {
        trajectory_id: id,
        short_id,
        object_mass_kg: (mass_kg as f64 * 1000.0).round() / 1000.0,
        static_friction_mu: (mu_s as f64 * 1000.0).round() / 1000.0,
        object_span_m: (span_m as f64 * 1000.0).round() / 1000.0,
        opposition_rad: (opposition as f64 * 1000.0).round() / 1000.0,
        commanded_close_rad: (close0 as f64 * 1000.0).round() / 1000.0,
        tendon_tension_n: (state.tendon_tension_n as f64 * 10.0).round() / 10.0,
        tendon_strain: (final_strain as f64 * 1000.0).round() / 1000.0,
        pad_normal_n: (state.pad_normal_n as f64 * 10.0).round() / 10.0,
        tactile_friction_margin: (final_margin as f64 * 1000.0).round() / 1000.0,
        slip_velocity_m_s: (state.slip_velocity as f64 * 1000.0).round() / 1000.0,
        tendon_overstretch: overstretch,
        pad_slip,
        halt_ms: (halt_ms * 100.0).round() / 100.0,
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
                "{}/../../data/exports/sovereign/humanoid_hand_tendon.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: HUMANOID HAND TENDON (dexterous serial + pad cone)");
    println!("  opposition default {THUMB_OPPOSITION_RAD} rad  n={n}  out={out}");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0x4841_4e44_5445_4e44);
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
        Field::new("object_span_m", DataType::Float64, false),
        Field::new("opposition_rad", DataType::Float64, false),
        Field::new("commanded_close_rad", DataType::Float64, false),
        Field::new("tendon_tension_n", DataType::Float64, false),
        Field::new("tendon_strain", DataType::Float64, false),
        Field::new("pad_normal_n", DataType::Float64, false),
        Field::new("tactile_friction_margin", DataType::Float64, false),
        Field::new("slip_velocity_m_s", DataType::Float64, false),
        Field::new("tendon_overstretch", DataType::Boolean, false),
        Field::new("pad_slip", DataType::Boolean, false),
        Field::new("halt_ms", DataType::Float64, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(
                rows.iter().map(|r| r.trajectory_id).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.object_mass_kg).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.static_friction_mu).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.object_span_m).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.opposition_rad).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter()
                    .map(|r| r.commanded_close_rad)
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.tendon_tension_n).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.tendon_strain).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.pad_normal_n).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter()
                    .map(|r| r.tactile_friction_margin)
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.slip_velocity_m_s).collect::<Vec<_>>(),
            )),
            Arc::new(BooleanArray::from(
                rows.iter().map(|r| r.tendon_overstretch).collect::<Vec<_>>(),
            )),
            Arc::new(BooleanArray::from(
                rows.iter().map(|r| r.pad_slip).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.halt_ms).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>(),
            )),
        ],
    )
    .expect("batch");

    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G humanoid hand tendon dual-regime v1.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let over = rows.iter().filter(|r| r.tendon_overstretch).count();
    let slip = rows.iter().filter(|r| r.pad_slip).count();
    let both = rows
        .iter()
        .filter(|r| r.tendon_overstretch && r.pad_slip)
        .count();
    let neither = rows
        .iter()
        .filter(|r| !r.tendon_overstretch && !r.pad_slip)
        .count();
    let unique: std::collections::BTreeSet<&str> =
        rows.iter().map(|r| r.proof_hash.as_str()).collect();
    println!(
        "  tendon_overstretch {over} ({:.1}%)  pad_slip {slip} ({:.1}%)",
        100.0 * over as f64 / n_f,
        100.0 * slip as f64 / n_f
    );
    println!("  four-cell both {both}  neither {neither}  unique proofs {}", unique.len());
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
