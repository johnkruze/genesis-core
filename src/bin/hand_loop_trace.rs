//! Window A genre on the hand: 100 steps at 1000 Hz.
//! Sibling of grasp_loop_trace / tissue_loop_trace. Same seed family as the bank.

use genesis_core::last_state::{self, LastStateFrame64, BODY_HAND};
use genesis_core::output;
use genesis_core::physics::dexterous::{
    evaluate_hand_tendon_dynamics, C_HandTendonState, N_HAND_FINGERS,
};
use genesis_core::proof::{self, ProofChain};
use genesis_core::rng::Rng;
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{BooleanArray, Float64Array, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::ArrowWriter;

struct Step {
    step: u32,
    t_ms: f64,
    tension_n: f64,
    pad_normal_n: f64,
    strain: f64,
    margin: f64,
    slip_m_s: f64,
    overstretch: bool,
    pad_slip: bool,
}

struct Trace {
    id: u32,
    mass_kg: f64,
    mu: f64,
    span_m: f64,
    close0: f64,
    halt_ms: f64,
    overstretch: bool,
    pad_slip: bool,
    steps: Vec<Step>,
    proof: String,
}

fn integrate(id: u32, rng: &mut Rng, record: bool) -> Trace {
    let _ = output::short_id(rng);
    let mass_kg = rng.range(0.05, 2.6) as f32;
    let mu_s = rng.range(0.10, 0.82) as f32;
    let span_m = rng.range(0.018, 0.078) as f32;
    let opposition = rng.range(0.35, 1.35) as f32;
    let close0 = rng.range(0.32, 1.55) as f32;
    let disturb = rng.range(0.0, 1.15) as f32;

    let mut state = C_HandTendonState {
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
        commanded_close_rad: close0,
        pad_normal_n: 0.0,
        normal_force: 4.0,
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
    let mut over_hist = false;
    let mut slip_hist = false;
    let mut steps = Vec::new();
    let mut frames: Vec<[u8; 64]> = Vec::new();
    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(mass_kg as f64);
    proof.feed_f64(mu_s as f64);

    for step in 0..100 {
        let t_ms = step as f64;
        state.commanded_close_rad = close0 + disturb * (step as f32) * 0.004;
        let res = evaluate_hand_tendon_dynamics(&mut state, dt);
        if res.tendon_overstretch {
            over_hist = true;
        }
        if res.pad_slip {
            slip_hist = true;
        }
        if t_first.is_nan() && res.pad_slip {
            t_first = t_ms;
        }
        if t_first.is_finite()
            && t_arrest.is_nan()
            && state.slip_velocity <= prev_slip + 1e-9
            && state.slip_velocity < 0.02
            && !res.pad_slip
        {
            t_arrest = t_ms;
        }
        if record {
            steps.push(Step {
                step: step as u32,
                t_ms,
                tension_n: state.tendon_tension_n as f64,
                pad_normal_n: state.pad_normal_n as f64,
                strain: res.strain as f64,
                margin: res.margin as f64,
                slip_m_s: state.slip_velocity as f64,
                overstretch: res.tendon_overstretch,
                pad_slip: res.pad_slip,
            });
            frames.push(
                LastStateFrame64::pack_hand(
                    step as u32,
                    state.tendon_tension_n,
                    state.pad_normal_n,
                    state.tendon_stretch_m,
                    state.opposition_rad,
                    state.q_mcp[0],
                    state.slip_velocity,
                    res.margin,
                    state.object_span_m,
                    res.tendon_overstretch,
                    res.pad_slip,
                )
                .to_bytes(),
            );
        }
        prev_slip = state.slip_velocity;
        proof.feed_f64(state.tendon_tension_n as f64);
        proof.feed_f64(res.margin as f64);
    }
    let halt = if t_first.is_finite() && t_arrest.is_finite() {
        (t_arrest - t_first).max(0.0)
    } else {
        -1.0
    };
    if record && !frames.is_empty() {
        let bin = last_state::write_soma_file(BODY_HAND, *b"HAND0001", &frames);
        let soma = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../grokd/public/soma/hand_terminal.soma.bin"
        );
        if let Some(p) = std::path::Path::new(soma).parent() {
            std::fs::create_dir_all(p).ok();
        }
        std::fs::write(soma, bin).ok();
    }
    Trace {
        id,
        mass_kg: mass_kg as f64,
        mu: mu_s as f64,
        span_m: span_m as f64,
        close0: close0 as f64,
        halt_ms: halt,
        overstretch: over_hist,
        pad_slip: slip_hist,
        steps,
        proof: proof.seal(),
    }
}

fn main() {
    let out = "../../grokd/data/hand_loop_1000hz.parquet";
    let mut rng = Rng::new(0x4841_4e44_5445_4e44);
    let t0 = Instant::now();
    let mut chosen: Option<u32> = None;
    for id in 0..2500u32 {
        let tr = integrate(id, &mut rng, false);
        if chosen.is_none() && tr.overstretch && tr.pad_slip {
            chosen = Some(id);
        }
    }
    let id = chosen.unwrap_or(0);
    let mut rng = Rng::new(0x4841_4e44_5445_4e44);
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
    println!("  G^G WINDOW A  ·  HAND 1000 Hz LOOP TRACE");
    println!(
        "  id={}  mass={:.3} kg  mu={:.3}  span={:.3} m  close0={:.3} rad",
        tr.id, tr.mass_kg, tr.mu, tr.span_m, tr.close0
    );
    println!(
        "  overstretch={}  pad_slip={}  halt_ms={:.1}  steps={}",
        tr.overstretch,
        tr.pad_slip,
        tr.halt_ms,
        tr.steps.len()
    );
    println!("====================================================================");

    if let Some(p) = std::path::Path::new(out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("hand_id", DataType::UInt32, false),
        Field::new("step", DataType::UInt32, false),
        Field::new("t_ms", DataType::Float64, false),
        Field::new("tension_n", DataType::Float64, false),
        Field::new("pad_normal_n", DataType::Float64, false),
        Field::new("strain", DataType::Float64, false),
        Field::new("margin", DataType::Float64, false),
        Field::new("slip_m_s", DataType::Float64, false),
        Field::new("tendon_overstretch", DataType::Boolean, false),
        Field::new("pad_slip", DataType::Boolean, false),
        Field::new("mass_kg", DataType::Float64, false),
        Field::new("mu", DataType::Float64, false),
        Field::new("halt_ms", DataType::Float64, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let n = tr.steps.len();
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new((0..n).map(|_| Some(tr.id)).collect::<UInt32Array>()),
            Arc::new(tr.steps.iter().map(|s| Some(s.step)).collect::<UInt32Array>()),
            Arc::new(tr.steps.iter().map(|s| Some(s.t_ms)).collect::<Float64Array>()),
            Arc::new(
                tr.steps
                    .iter()
                    .map(|s| Some(s.tension_n))
                    .collect::<Float64Array>(),
            ),
            Arc::new(
                tr.steps
                    .iter()
                    .map(|s| Some(s.pad_normal_n))
                    .collect::<Float64Array>(),
            ),
            Arc::new(tr.steps.iter().map(|s| Some(s.strain)).collect::<Float64Array>()),
            Arc::new(tr.steps.iter().map(|s| Some(s.margin)).collect::<Float64Array>()),
            Arc::new(
                tr.steps
                    .iter()
                    .map(|s| Some(s.slip_m_s))
                    .collect::<Float64Array>(),
            ),
            Arc::new(
                tr.steps
                    .iter()
                    .map(|s| Some(s.overstretch))
                    .collect::<BooleanArray>(),
            ),
            Arc::new(
                tr.steps
                    .iter()
                    .map(|s| Some(s.pad_slip))
                    .collect::<BooleanArray>(),
            ),
            Arc::new((0..n).map(|_| Some(tr.mass_kg)).collect::<Float64Array>()),
            Arc::new((0..n).map(|_| Some(tr.mu)).collect::<Float64Array>()),
            Arc::new((0..n).map(|_| Some(tr.halt_ms)).collect::<Float64Array>()),
            Arc::new((0..n).map(|_| Some(tr.proof.clone())).collect::<StringArray>()),
        ],
    )
    .expect("batch");
    let run_proof = proof::seal_run(&[tr.proof.clone()]);
    let file = std::fs::File::create(out).unwrap();
    let props = output::parquet_receipt_properties(&run_proof, "G^G hand 1000 Hz loop trace Window A");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    println!("  parquet {out}");
    println!("  seal {run_proof}");
    println!("  {:?}", t0.elapsed());
}
