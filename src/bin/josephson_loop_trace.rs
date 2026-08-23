//! Window A genre on the RCSJ pendulum: 100 samples of the 50 ns envelope.
//! Same seed / ICs as josephson_monte_carlo campaign n=2500. Solver stays 2 ps.
//! 1000 Hz on a 50 ns junction is costume — diesel law. This pulse is 0.5 ns / row.
//! No panic halt on first phase slip. The gate is coherence < 0.005 or T > 100 mK.

use genesis_core::physics::josephson::JosephsonState;
use genesis_core::proof::{self, ProofChain};
use genesis_core::rng::Rng;
use rayon::prelude::*;
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
    t_ns: f64,
    phase: f64,
    phase_velocity: f64,
    drive_na: f64,
    thermal_na: f64,
    coherence: f64,
    residual: f64,
    quenched: bool,
    pulse_on: bool,
}

struct Trace {
    id: u32,
    temp_mk: f64,
    pulse_amplitude_na: f64,
    control_frequency_ghz: f64,
    pulse_duration_ns: f64,
    initial_phase: f64,
    coherent: bool,
    quenched: bool,
    thermal: bool,
    drive: bool,
    final_coherence: f64,
    max_residual: f64,
    t_quench_ns: f64,
    steps: Vec<Step>,
    proof: String,
}

struct Ics {
    temp_mk: f64,
    pulse_amplitude_na: f64,
    control_frequency_ghz: f64,
    pulse_duration_ns: f64,
    initial_phase: f64,
}

fn sample_ics(rng: &mut Rng) -> Ics {
    Ics {
        temp_mk: rng.range(10.0, 100.0),
        pulse_amplitude_na: rng.range(0.0, 45.0),
        control_frequency_ghz: rng.range(3.0, 4.98),
        pulse_duration_ns: rng.range(5.0, 23.0),
        initial_phase: rng.range(0.0, 1.35),
    }
}

fn integrate(id: u32, ics: &Ics, record: bool) -> Trace {
    let mut qubit = JosephsonState::new();
    qubit.temp_mk = ics.temp_mk;
    qubit.phase = ics.initial_phase;

    let dt: f64 = 0.002; // 2 ps
    let total_time_ns: f64 = 50.0;
    let steps = (total_time_ns / dt).round() as usize; // dt, total_time_ns are f64
    let record_every = (steps / 100).max(1);
    let omega_drive = 2.0 * std::f64::consts::PI * ics.control_frequency_ghz;

    let mut max_residual = 0.0;
    let mut t_quench = f64::NAN;
    let mut samples = Vec::new();
    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(ics.temp_mk);
    proof.feed_f64(ics.pulse_amplitude_na);
    proof.feed_f64(ics.control_frequency_ghz);
    proof.feed_f64(ics.pulse_duration_ns);
    proof.feed_f64(ics.initial_phase);

    for step in 0..steps {
        let t = step as f64 * dt;
        let control_input = if t < ics.pulse_duration_ns {
            ics.pulse_amplitude_na * (omega_drive * t).sin()
        } else {
            0.0
        };
        let seed = (id as u64) ^ (step as u64).wrapping_mul(0x9e3779b97f4a7c15);
        qubit.step(dt, control_input, seed);
        if qubit.residual > max_residual {
            max_residual = qubit.residual;
        }
        if t_quench.is_nan() && qubit.quenched {
            t_quench = t;
        }
        if record && step % record_every == 0 && samples.len() < 100 {
            samples.push(Step {
                step: samples.len() as u32,
                t_ns: t,
                phase: qubit.phase,
                phase_velocity: qubit.phase_velocity,
                drive_na: control_input,
                thermal_na: qubit.thermal_current,
                coherence: qubit.coherence,
                residual: qubit.residual,
                quenched: qubit.quenched,
                pulse_on: t < ics.pulse_duration_ns,
            });
        }
        if step % 2500 == 0 {
            proof.feed_f64(qubit.phase);
            proof.feed_f64(qubit.coherence);
        }
    }

    let thermal = qubit.quenched && ics.temp_mk >= 50.0;
    let drive = max_residual > 1e-5 || (qubit.quenched && ics.temp_mk < 50.0);
    let coherent = !qubit.quenched && qubit.coherence >= 0.005;
    if coherent {
        proof.feed_str("COHERENT");
    } else if thermal {
        proof.feed_str("THERMAL_QUENCH");
    } else {
        proof.feed_str("DRIVE_RESIDUAL");
    }
    proof.feed_f64(qubit.coherence);
    proof.feed_f64(max_residual);

    Trace {
        id,
        temp_mk: ics.temp_mk,
        pulse_amplitude_na: ics.pulse_amplitude_na,
        control_frequency_ghz: ics.control_frequency_ghz,
        pulse_duration_ns: ics.pulse_duration_ns,
        initial_phase: ics.initial_phase,
        coherent,
        quenched: qubit.quenched,
        thermal,
        drive,
        final_coherence: qubit.coherence,
        max_residual,
        t_quench_ns: t_quench,
        steps: samples,
        proof: proof.seal(),
    }
}

fn main() {
    let out = format!(
        "{}/../../grokd/data/josephson_loop_1000hz.parquet",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut rng = Rng::new(0x4a4f_5345_50485f51);
    let t0 = Instant::now();
    let ics: Vec<(u32, Ics)> = (0..2500u32).map(|id| (id, sample_ics(&mut rng))).collect();
    let replay: Vec<(u32, bool, bool, bool, f64, f64)> = ics
        .par_iter()
        .map(|(id, ics)| {
            let tr = integrate(*id, ics, false);
            (
                *id,
                tr.coherent,
                tr.quenched,
                tr.thermal,
                tr.pulse_duration_ns,
                tr.temp_mk,
            )
        })
        .collect();
    let mut chosen: Option<u32> = None;
    for (id, coherent, _, _, dur, temp) in &replay {
        if *coherent && *dur >= 8.0 && *dur <= 20.0 && *temp < 30.0 {
            chosen = Some(*id);
            break;
        }
    }
    if chosen.is_none() {
        for (id, coherent, _, _, dur, _) in &replay {
            if *coherent && *dur >= 8.0 && *dur <= 20.0 {
                chosen = Some(*id);
                break;
            }
        }
    }
    let coherent_n = replay.iter().filter(|(_, c, _, _, _, _)| *c).count();
    let cold_pulse = replay
        .iter()
        .filter(|(_, c, _, _, dur, t)| *c && *dur >= 8.0 && *dur <= 20.0 && *t < 30.0)
        .count();
    println!(
        "  scan n=2500  coherent={}  coherent-cold-pulse-8-20ns={}",
        coherent_n, cold_pulse
    );
    let id = chosen.expect("coherent RCSJ pulse with drive in-window");
    let mut rng = Rng::new(0x4a4f_5345_50485f51);
    let mut trace = None;
    for i in 0..=id {
        let ics = sample_ics(&mut rng);
        let rec = i == id;
        let tr = integrate(i, &ics, rec);
        if rec {
            trace = Some(tr);
        }
    }
    let tr = trace.expect("trace");
    println!("====================================================================");
    println!("  G^G  ·  JOSEPHSON RCSJ LOOP TRACE  ·  50 ns / 100 samples");
    println!(
        "  id={}  T={:.1} mK  A={:.2} nA  f={:.3} GHz  pulse={:.2} ns  φ0={:.3}",
        tr.id,
        tr.temp_mk,
        tr.pulse_amplitude_na,
        tr.control_frequency_ghz,
        tr.pulse_duration_ns,
        tr.initial_phase
    );
    println!(
        "  coherent={}  quenched={}  thermal={}  drive={}  Cend={:.4}  steps={}",
        tr.coherent,
        tr.quenched,
        tr.thermal,
        tr.drive,
        tr.final_coherence,
        tr.steps.len()
    );
    println!("====================================================================");

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("josephson_id", DataType::UInt32, false),
        Field::new("step", DataType::UInt32, false),
        Field::new("t_ns", DataType::Float64, false),
        Field::new("phase", DataType::Float64, false),
        Field::new("phase_velocity", DataType::Float64, false),
        Field::new("drive_na", DataType::Float64, false),
        Field::new("thermal_na", DataType::Float64, false),
        Field::new("coherence", DataType::Float64, false),
        Field::new("residual", DataType::Float64, false),
        Field::new("quenched", DataType::Boolean, false),
        Field::new("pulse_on", DataType::Boolean, false),
        Field::new("temp_mk", DataType::Float64, false),
        Field::new("pulse_amplitude_na", DataType::Float64, false),
        Field::new("control_frequency_ghz", DataType::Float64, false),
        Field::new("pulse_duration_ns", DataType::Float64, false),
        Field::new("initial_phase", DataType::Float64, false),
        Field::new("final_coherence", DataType::Float64, false),
        Field::new("max_residual", DataType::Float64, false),
        Field::new("t_quench_ns", DataType::Float64, false),
        Field::new("is_coherent", DataType::Boolean, false),
        Field::new("is_thermal_quench", DataType::Boolean, false),
        Field::new("is_drive_residual_failed", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let n = tr.steps.len();
    let t_quench_col = if tr.t_quench_ns.is_finite() {
        tr.t_quench_ns
    } else {
        -1.0
    };
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new((0..n).map(|_| Some(tr.id)).collect::<UInt32Array>()),
            Arc::new(tr.steps.iter().map(|s| Some(s.step)).collect::<UInt32Array>()),
            Arc::new(tr.steps.iter().map(|s| Some(s.t_ns)).collect::<Float64Array>()),
            Arc::new(tr.steps.iter().map(|s| Some(s.phase)).collect::<Float64Array>()),
            Arc::new(
                tr.steps
                    .iter()
                    .map(|s| Some(s.phase_velocity))
                    .collect::<Float64Array>(),
            ),
            Arc::new(
                tr.steps
                    .iter()
                    .map(|s| Some(s.drive_na))
                    .collect::<Float64Array>(),
            ),
            Arc::new(
                tr.steps
                    .iter()
                    .map(|s| Some(s.thermal_na))
                    .collect::<Float64Array>(),
            ),
            Arc::new(
                tr.steps
                    .iter()
                    .map(|s| Some(s.coherence))
                    .collect::<Float64Array>(),
            ),
            Arc::new(
                tr.steps
                    .iter()
                    .map(|s| Some(s.residual))
                    .collect::<Float64Array>(),
            ),
            Arc::new(
                tr.steps
                    .iter()
                    .map(|s| Some(s.quenched))
                    .collect::<BooleanArray>(),
            ),
            Arc::new(
                tr.steps
                    .iter()
                    .map(|s| Some(s.pulse_on))
                    .collect::<BooleanArray>(),
            ),
            Arc::new((0..n).map(|_| Some(tr.temp_mk)).collect::<Float64Array>()),
            Arc::new(
                (0..n)
                    .map(|_| Some(tr.pulse_amplitude_na))
                    .collect::<Float64Array>(),
            ),
            Arc::new(
                (0..n)
                    .map(|_| Some(tr.control_frequency_ghz))
                    .collect::<Float64Array>(),
            ),
            Arc::new(
                (0..n)
                    .map(|_| Some(tr.pulse_duration_ns))
                    .collect::<Float64Array>(),
            ),
            Arc::new(
                (0..n)
                    .map(|_| Some(tr.initial_phase))
                    .collect::<Float64Array>(),
            ),
            Arc::new(
                (0..n)
                    .map(|_| Some(tr.final_coherence))
                    .collect::<Float64Array>(),
            ),
            Arc::new((0..n).map(|_| Some(tr.max_residual)).collect::<Float64Array>()),
            Arc::new((0..n).map(|_| Some(t_quench_col)).collect::<Float64Array>()),
            Arc::new((0..n).map(|_| Some(tr.coherent)).collect::<BooleanArray>()),
            Arc::new((0..n).map(|_| Some(tr.thermal)).collect::<BooleanArray>()),
            Arc::new((0..n).map(|_| Some(tr.drive)).collect::<BooleanArray>()),
            Arc::new((0..n).map(|_| Some(tr.proof.clone())).collect::<StringArray>()),
        ],
    )
    .expect("batch");
    let run_proof = proof::seal_run(&[tr.proof.clone()]);
    let file = std::fs::File::create(&out).unwrap();
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.clone()),
            parquet::file::metadata::KeyValue::new(
                "generator".to_string(),
                "G^G Josephson RCSJ loop trace — 50 ns / 100 samples".to_string(),
            ),
            parquet::file::metadata::KeyValue::new("josephson_id".to_string(), tr.id.to_string()),
        ]))
        .build();
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    println!("  parquet {out}");
    println!("  seal {run_proof}");
    println!("  {:?}", t0.elapsed());
    println!("  coherent through 50 ns: {coherent_n} / 2500 scanned");
}
