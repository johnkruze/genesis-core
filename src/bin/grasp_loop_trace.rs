//! Window A pinout: 100 steps at 1000 Hz for one inhabited grasp.
//! Same seed as autolab_dexterous_grasp_monte_carlo. Picks a held pulse with a halt.

use genesis_core::physics::dexterous::{
    evaluate_grasp_dynamics, C_GraspState, C_TactileArray, Taxel,
};
use genesis_core::proof::{self, ProofChain};
use genesis_core::rng::Rng;
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{BooleanArray, Float64Array, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;

struct Step {
    step: u32,
    t_ms: f64,
    force_n: f64,
    margin: f64,
    slip_m_s: f64,
    micro: bool,
    macro_slip: bool,
    reflex: bool,
    arrested: bool,
}

struct Trace {
    id: u32,
    mass_kg: f64,
    mu: f64,
    f0_n: f64,
    halt_ms: f64,
    held: bool,
    steps: Vec<Step>,
    proof: String,
}

fn integrate(id: u32, rng: &mut Rng, record: bool) -> Trace {
    let mass_kg = rng.range(0.01, 2.5) as f32;
    let mu_s = rng.range(0.15, 0.85) as f32;
    let initial_force = rng.range(5.0, 30.0) as f32;
    let disturbances = rng.range(0.0, 0.8) as f32;
    let _ = genesis_core::output::short_id(rng);

    let mut state = C_GraspState {
        normal_force: initial_force,
        slip_velocity: 0.0,
        slip_angular_velocity: 0.0,
        object_mass: mass_kg,
        static_friction_coeff: mu_s,
        dynamic_friction_coeff: mu_s * 0.8,
        reflex_active: false,
    };
    let dt = 0.001f32;
    let mut t_first = f64::NAN;
    let mut t_arrest = f64::NAN;
    let mut prev_slip = 0.0f32;
    let mut macro_hist = false;
    let mut final_margin = 0.0f32;
    let mut steps = Vec::new();
    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(mass_kg as f64);
    proof.feed_f64(mu_s as f64);

    for step in 0..100 {
        let t_ms = step as f64;
        let n = state.normal_force / 16.0;
        let shear = (mass_kg * 9.81 + disturbances * (step as f32 * 0.1)) / 16.0;
        let mut taxels = [Taxel { normal: n, shear_x: 0.0, shear_y: 0.0 }; 16];
        for i in 0..16 {
            taxels[i].shear_x = shear * (1.0 + 0.1 * (i as f32 % 4.0));
            taxels[i].shear_y = shear * 0.15 * (i as f32 / 4.0);
        }
        let res = evaluate_grasp_dynamics(&C_TactileArray { taxels }, &mut state, dt);
        if res.macro_slip_detected {
            macro_hist = true;
        }
        final_margin = res.margin;
        if t_first.is_nan() && (res.micro_slip_detected || res.macro_slip_detected || res.rotational_slip_detected)
        {
            t_first = t_ms;
        }
        if t_first.is_finite()
            && t_arrest.is_nan()
            && state.slip_velocity <= prev_slip + 1e-9
            && state.slip_velocity < 0.02
            && !res.macro_slip_detected
        {
            t_arrest = t_ms;
        }
        if record {
            steps.push(Step {
                step: step as u32,
                t_ms,
                force_n: state.normal_force as f64,
                margin: res.margin as f64,
                slip_m_s: state.slip_velocity as f64,
                micro: res.micro_slip_detected,
                macro_slip: res.macro_slip_detected,
                reflex: state.reflex_active,
                arrested: t_arrest.is_finite() && (t_ms - t_arrest).abs() < 0.5,
            });
        }
        prev_slip = state.slip_velocity;
        proof.feed_f64(state.normal_force as f64);
        proof.feed_f64(res.margin as f64);
    }
    let halt = if t_first.is_finite() && t_arrest.is_finite() {
        (t_arrest - t_first).max(0.0)
    } else {
        -1.0
    };
    let held = final_margin > 0.15 && state.normal_force <= 45.0 && !macro_hist;
    Trace {
        id,
        mass_kg: mass_kg as f64,
        mu: mu_s as f64,
        f0_n: initial_force as f64,
        halt_ms: halt,
        held,
        steps,
        proof: proof.seal(),
    }
}

fn main() {
    let out = "../../grokd/data/grasp_loop_1000hz.parquet";
    let mut rng = Rng::new(0x4752_4153_505f4c41);
    let t0 = Instant::now();
    let mut chosen: Option<u32> = None;
    let mut replay = Vec::new();
    for id in 0..2500u32 {
        let tr = integrate(id, &mut rng, false);
        replay.push((id, tr.held, tr.halt_ms, tr.mass_kg, tr.mu, tr.f0_n));
        if chosen.is_none() && tr.held && tr.halt_ms >= 2.0 && tr.halt_ms <= 16.0 {
            chosen = Some(id);
        }
    }
    let id = chosen.unwrap_or(0);
    // Replay from a fresh seed through `id` to record steps.
    let mut rng = Rng::new(0x4752_4153_505f4c41);
    let mut trace = None;
    for i in 0..=id {
        let rec = i == id;
        let tr = integrate(i, &mut rng, rec);
        if rec {
            trace = Some(tr);
        }
    }
    let tr = trace.expect("trace");
    println!("====================================================================");
    println!("  G^G WINDOW A  ·  1000 Hz LOOP TRACE");
    println!("  id={}  mass={:.3} kg  mu={:.3}  F0={:.2} N", tr.id, tr.mass_kg, tr.mu, tr.f0_n);
    println!("  held={}  halt_ms={:.1}  steps={}", tr.held, tr.halt_ms, tr.steps.len());
    println!("====================================================================");

    if let Some(p) = std::path::Path::new(out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("grasp_id", DataType::UInt32, false),
        Field::new("step", DataType::UInt32, false),
        Field::new("t_ms", DataType::Float64, false),
        Field::new("force_n", DataType::Float64, false),
        Field::new("margin", DataType::Float64, false),
        Field::new("slip_m_s", DataType::Float64, false),
        Field::new("micro_slip", DataType::Boolean, false),
        Field::new("macro_slip", DataType::Boolean, false),
        Field::new("reflex_active", DataType::Boolean, false),
        Field::new("mass_kg", DataType::Float64, false),
        Field::new("mu", DataType::Float64, false),
        Field::new("halt_ms", DataType::Float64, false),
        Field::new("held", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let n = tr.steps.len();
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new((0..n).map(|_| Some(tr.id)).collect::<UInt32Array>()),
            Arc::new(tr.steps.iter().map(|s| Some(s.step)).collect::<UInt32Array>()),
            Arc::new(tr.steps.iter().map(|s| Some(s.t_ms)).collect::<Float64Array>()),
            Arc::new(tr.steps.iter().map(|s| Some(s.force_n)).collect::<Float64Array>()),
            Arc::new(tr.steps.iter().map(|s| Some(s.margin)).collect::<Float64Array>()),
            Arc::new(tr.steps.iter().map(|s| Some(s.slip_m_s)).collect::<Float64Array>()),
            Arc::new(tr.steps.iter().map(|s| Some(s.micro)).collect::<BooleanArray>()),
            Arc::new(tr.steps.iter().map(|s| Some(s.macro_slip)).collect::<BooleanArray>()),
            Arc::new(tr.steps.iter().map(|s| Some(s.reflex)).collect::<BooleanArray>()),
            Arc::new((0..n).map(|_| Some(tr.mass_kg)).collect::<Float64Array>()),
            Arc::new((0..n).map(|_| Some(tr.mu)).collect::<Float64Array>()),
            Arc::new((0..n).map(|_| Some(tr.halt_ms)).collect::<Float64Array>()),
            Arc::new((0..n).map(|_| Some(tr.held)).collect::<BooleanArray>()),
            Arc::new((0..n).map(|_| Some(tr.proof.clone())).collect::<StringArray>()),
        ],
    )
    .expect("batch");
    let run_proof = proof::seal_run(&[tr.proof.clone()]);
    let file = std::fs::File::create(out).unwrap();
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.clone()),
            parquet::file::metadata::KeyValue::new("generator".to_string(), "G^G grasp 1000 Hz loop trace Window A".to_string()),
            parquet::file::metadata::KeyValue::new("grasp_id".to_string(), tr.id.to_string()),
        ]))
        .build();
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    println!("  parquet {out}");
    println!("  seal {run_proof}");
    println!("  {:?}", t0.elapsed());
    let _ = replay;
}
