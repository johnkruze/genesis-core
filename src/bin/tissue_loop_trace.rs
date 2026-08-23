//! Window A genre on the other clamp: 100 steps at 1000 Hz.
//! Same seed / ICs as surgical_tissue_monte_carlo. Reflex the detector bank does not have:
//! freeze the jaw (dx=0) and decay force to the tissue limit. Halt is arrest − first overstress.

use genesis_core::output;
use genesis_core::physics::dexterous::{
    evaluate_surgical_grasp_dynamics, C_SurgicalTissueAuditor,
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
    disp_m: f64,
    clamped_n: f64,
    overstress: bool,
    rupture: bool,
    cable: bool,
    reflex: bool,
    arrested: bool,
}

struct Trace {
    id: u32,
    tissue_type_id: u32,
    tissue_limit_n: f64,
    policy_command_n: f64,
    f0_n: f64,
    halt_ms: f64,
    t_first_ms: f64,
    t_arrest_ms: f64,
    held: bool,
    steps: Vec<Step>,
    proof: String,
}

fn tissue_limit(id: u32) -> f32 {
    match id {
        0 => 1.2,
        1 => 2.5,
        2 => 40.0,
        _ => 1.0,
    }
}

fn integrate(id: u32, rng: &mut Rng, record: bool) -> Trace {
    let _ = output::short_id(rng);
    let tissue_type_id = rng.index(3) as u32;
    let limit = tissue_limit(tissue_type_id);
    let max_tearing = limit * rng.range(0.8, 1.35) as f32;
    let policy_command = limit * rng.range(0.35, 2.15) as f32;
    let cable_failed = rng.range(0.0, 1.0) < 0.12;
    let yield_force = limit * rng.range(0.80, 1.45) as f32;
    let disp_rate = rng.range(0.04, 0.18) as f32;

    let mut disp = 0.0f32;
    let mut force = rng.range(0.05, 0.35) as f32 * limit;
    let f0 = force;
    let mut last_disp;
    let mut last_force;
    let mut reflex = false;
    let mut clamp_target = limit;
    let mut t_first = f64::NAN;
    let mut t_arrest = f64::NAN;
    let mut rupture_hist = false;
    let mut cable_hist = false;
    let mut end_over = false;
    let mut end_clamped = limit;
    let mut steps = Vec::new();
    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(limit as f64);
    proof.feed_f64(policy_command as f64);

    let dt = 0.001f32;
    for step in 0..100 {
        let t_ms = step as f64;
        last_disp = disp;
        last_force = force;

        if cable_failed {
            force *= 0.92;
            if disp > 0.012 {
                force = rng.range(0.0, 0.04) as f32;
            }
            disp += disp_rate * dt;
        } else if reflex {
            // Freeze jaw. Decay is the halt. dx=0 so the rupture detector
            // does not fire on the back-off (that flag is a stiffness drop
            // while still closing).
            // 0.12 / step → ~2 ms from a 0.01 N overshoot (tissue.c ramp).
            force += (clamp_target - force) * 0.12;
        } else {
            disp += disp_rate * dt;
            force += (policy_command - force) * 0.10;
            if force > yield_force && disp > 0.0015 {
                force *= 0.42;
            }
        }

        let auditor = C_SurgicalTissueAuditor {
            tissue_type_id,
            max_tearing_force_n: max_tearing,
            measured_displacement_m: disp,
            measured_force_n: force,
            relaxation_tau: 0.05,
            last_displacement_m: last_disp,
            last_force_n: last_force,
            accumulated_energy_j: (force * disp).max(0.0),
        };
        let res = evaluate_surgical_grasp_dynamics(&auditor, dt);
        if res.viscoelastic_rupture_detected {
            rupture_hist = true;
        }
        if res.cable_slip_fault {
            cable_hist = true;
        }
        end_over = res.tissue_overstress_detected;
        end_clamped = res.clamped_force;

        if t_first.is_nan() && res.tissue_overstress_detected {
            t_first = t_ms;
            reflex = true;
            // Hold clearance under the tear so overstress (`F > clamp`) goes quiet.
            clamp_target = res.clamped_force * 0.95;
        }
        if t_first.is_finite()
            && t_arrest.is_nan()
            && !res.tissue_overstress_detected
            && force <= res.clamped_force + 1e-6
        {
            t_arrest = t_ms;
        }

        if record {
            steps.push(Step {
                step: step as u32,
                t_ms,
                force_n: force as f64,
                disp_m: disp as f64,
                clamped_n: res.clamped_force as f64,
                overstress: res.tissue_overstress_detected,
                rupture: res.viscoelastic_rupture_detected,
                cable: res.cable_slip_fault,
                reflex,
                arrested: t_arrest.is_finite() && (t_ms - t_arrest).abs() < 0.5,
            });
        }
        if step % 25 == 0 {
            proof.feed_f64(force as f64);
            proof.feed_f64(disp as f64);
        }
    }

    let halt = if t_first.is_finite() && t_arrest.is_finite() {
        (t_arrest - t_first).max(0.0)
    } else {
        -1.0
    };
    let held = !rupture_hist && !cable_hist && !end_over && force <= end_clamped + 1e-6;
    if held {
        proof.feed_str("SAMPLE_HELD");
    } else if cable_hist {
        proof.feed_str("CABLE_SLIP");
    } else if rupture_hist {
        proof.feed_str("VISCOELASTIC_RUPTURE");
    } else {
        proof.feed_str("TISSUE_OVERSTRESS");
    }

    Trace {
        id,
        tissue_type_id,
        tissue_limit_n: limit as f64,
        policy_command_n: policy_command as f64,
        f0_n: f0 as f64,
        halt_ms: halt,
        t_first_ms: t_first,
        t_arrest_ms: t_arrest,
        held,
        steps,
        proof: proof.seal(),
    }
}

fn main() {
    let out = format!(
        "{}/../../grokd/data/surgical_tissue_loop_1000hz.parquet",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut rng = Rng::new(0x5355_5247_5f544953);
    let t0 = Instant::now();
    let mut chosen: Option<u32> = None;
    let mut replay = Vec::new();
    for id in 0..2500u32 {
        let tr = integrate(id, &mut rng, false);
        replay.push((id, tr.tissue_type_id, tr.held, tr.halt_ms));
        if chosen.is_none()
            && tr.tissue_type_id == 0
            && tr.held
            && tr.halt_ms >= 2.0
            && tr.halt_ms <= 16.0
        {
            chosen = Some(id);
        }
    }
    let liver_any: Vec<_> = replay
        .iter()
        .filter(|(_, ty, _, _)| *ty == 0)
        .cloned()
        .collect();
    let liver_held = liver_any.iter().filter(|(_, _, h, _)| *h).count();
    let liver_halted = liver_any
        .iter()
        .filter(|(_, _, h, halt)| *h && *halt >= 0.0)
        .count();
    println!(
        "  scan n=2500  liver={}  liver-held={}  liver-held-with-halt={}",
        liver_any.len(),
        liver_held,
        liver_halted
    );
    let id = chosen.expect("liver pulse with halt 2–16 ms");
    let mut rng = Rng::new(0x5355_5247_5f544953);
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
    println!("  G^G  ·  TISSUE 1000 Hz LOOP TRACE  ·  other clamp");
    println!(
        "  id={}  liver={}  limit={:.2} N  policy={:.2} N  F0={:.2} N",
        tr.id,
        tr.tissue_type_id == 0,
        tr.tissue_limit_n,
        tr.policy_command_n,
        tr.f0_n
    );
    println!(
        "  held={}  halt_ms={:.1}  first={:.0}  arrest={:.0}  steps={}",
        tr.held,
        tr.halt_ms,
        tr.t_first_ms,
        tr.t_arrest_ms,
        tr.steps.len()
    );
    println!("====================================================================");

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("tissue_id", DataType::UInt32, false),
        Field::new("step", DataType::UInt32, false),
        Field::new("t_ms", DataType::Float64, false),
        Field::new("force_n", DataType::Float64, false),
        Field::new("disp_m", DataType::Float64, false),
        Field::new("clamped_n", DataType::Float64, false),
        Field::new("overstress", DataType::Boolean, false),
        Field::new("rupture", DataType::Boolean, false),
        Field::new("cable", DataType::Boolean, false),
        Field::new("reflex_active", DataType::Boolean, false),
        Field::new("arrested", DataType::Boolean, false),
        Field::new("tissue_limit_n", DataType::Float64, false),
        Field::new("policy_command_n", DataType::Float64, false),
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
            Arc::new(tr.steps.iter().map(|s| Some(s.disp_m)).collect::<Float64Array>()),
            Arc::new(
                tr.steps
                    .iter()
                    .map(|s| Some(s.clamped_n))
                    .collect::<Float64Array>(),
            ),
            Arc::new(
                tr.steps
                    .iter()
                    .map(|s| Some(s.overstress))
                    .collect::<BooleanArray>(),
            ),
            Arc::new(tr.steps.iter().map(|s| Some(s.rupture)).collect::<BooleanArray>()),
            Arc::new(tr.steps.iter().map(|s| Some(s.cable)).collect::<BooleanArray>()),
            Arc::new(tr.steps.iter().map(|s| Some(s.reflex)).collect::<BooleanArray>()),
            Arc::new(
                tr.steps
                    .iter()
                    .map(|s| Some(s.arrested))
                    .collect::<BooleanArray>(),
            ),
            Arc::new((0..n).map(|_| Some(tr.tissue_limit_n)).collect::<Float64Array>()),
            Arc::new((0..n).map(|_| Some(tr.policy_command_n)).collect::<Float64Array>()),
            Arc::new((0..n).map(|_| Some(tr.halt_ms)).collect::<Float64Array>()),
            Arc::new((0..n).map(|_| Some(tr.held)).collect::<BooleanArray>()),
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
                "G^G tissue 1000 Hz loop trace — other clamp halt".to_string(),
            ),
            parquet::file::metadata::KeyValue::new("tissue_id".to_string(), tr.id.to_string()),
        ]))
        .build();
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    println!("  parquet {out}");
    println!("  seal {run_proof}");
    println!("  {:?}", t0.elapsed());
    let liver_halted = replay
        .iter()
        .filter(|(_, ty, held, halt)| *ty == 0 && *held && *halt >= 2.0 && *halt <= 16.0)
        .count();
    println!("  liver held+halt 2–16 ms: {liver_halted} / 2500 scanned");
}
