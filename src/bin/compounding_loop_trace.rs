//! Window A genre on the 68Q mill: 100 steps at 1000 Hz.
//! Same seed / ICs as compounding_pharmaceutical_monte_carlo. Reflex the 350 s
//! bank does not have: ramp the mill, freeze RPM when instantaneous Ostwald
//! stress crosses the named shear gate, decay under it. Halt is arrest − first
//! overstress. Dissolution is the slow clock — it barely moves in 100 ms.

use genesis_core::output;
use genesis_core::physics::compounding::{
    CompoundingState, DISSOLUTION_STALL_PCT, POTENCY_COLLAPSE_PCT,
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
    mill_shear_s1: f64,
    viscosity_pas: f64,
    tau_pa: f64,
    accumulated_pa: f64,
    potency_pct: f64,
    dissolution_pct: f64,
    overstress: bool,
    potency_collapsed: bool,
    dissolution_stalled: bool,
    reflex: bool,
    arrested: bool,
}

struct Trace {
    id: u32,
    state_type: u32,
    commanded_shear_s1: f64,
    critical_shear_pa: f64,
    k: f64,
    n: f64,
    halt_ms: f64,
    t_first_ms: f64,
    t_arrest_ms: f64,
    held: bool,
    steps: Vec<Step>,
    proof: String,
}

fn sample_ics(rng: &mut Rng) -> (f64, f64, f64, f64, f64, usize, f64, f64, f64, f64, f64, f64) {
    let _ = output::short_id(rng);
    let initial_mass = rng.range(0.01, 0.50);
    let k_index = rng.range(0.005, 0.08);
    let n_index = rng.range(0.45, 0.95);
    let shear = rng.range(10.0, 800.0);
    let ph = rng.range(1.5, 7.8);
    let state_type = rng.range(0.0, 3.0) as usize;
    let specific_area = rng.range(0.4, 5.5);
    let solubility = rng.range(25.0, 85.0);
    let diffusion = rng.range(1.5e-10, 1.8e-9);
    let boundary_h = rng.range(8.0e-6, 4.0e-5);
    let critical = match state_type {
        2 => rng.range(12.0, 40.0),
        1 => rng.range(120.0, 450.0),
        _ => rng.range(60.0, 280.0),
    };
    (
        initial_mass,
        k_index,
        n_index,
        shear,
        ph,
        state_type,
        specific_area,
        solubility,
        diffusion,
        boundary_h,
        critical,
        initial_mass,
    )
}

fn mill_tau(k: f64, n: f64, shear: f64) -> f64 {
    let g = shear.max(1e-3);
    let visc = (k * g.powf(n - 1.0)).clamp(0.0005, 5.0);
    visc * g
}

fn safe_shear(k: f64, n: f64, gate_pa: f64) -> f64 {
    let target = (gate_pa * 0.95).max(1e-6);
    let visc_lo = 0.0005;
    let visc_hi = 5.0;
    // τ = clamp(K γ^{n-1}, visc_lo, visc_hi) * γ. Invert in log space.
    let mut lo = 1e-3_f64;
    let mut hi = 1.0e6_f64;
    for _ in 0..48 {
        let mid = (lo * hi).sqrt();
        if mill_tau(k, n, mid) > target {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    let g = ((lo + hi) * 0.5).clamp(1e-3, 1.0e6);
    let _ = (visc_lo, visc_hi);
    g
}

fn integrate(id: u32, rng: &mut Rng, record: bool) -> Trace {
    let (initial_mass, k_index, n_index, commanded, ph, state_type, specific_area, solubility, diffusion, boundary_h, critical, _) =
        sample_ics(rng);

    let mut state = match state_type {
        0 => CompoundingState::new_stomach_state(),
        1 => CompoundingState::new_blood_state(),
        _ => CompoundingState::new_bioreactor_state(),
    };
    state.solid_mass_kg = initial_mass;
    state.solid_surface_area_m2 = (initial_mass * specific_area).clamp(0.02, 2.5);
    state.flow_consistency_index_k = k_index;
    state.flow_behavior_index_n = n_index;
    state.ph = ph;
    state.solubility_limit_cs = solubility;
    state.diffusion_coefficient = diffusion;
    state.boundary_layer_h = boundary_h;
    state.critical_shear_limit = critical;

    let mut mill_shear = commanded * 0.08;
    let mut reflex = false;
    let mut clamp_target = commanded;
    let mut t_first = f64::NAN;
    let mut t_arrest = f64::NAN;
    let mut potency_hist = false;
    let mut end_over = false;
    let mut end_tau = 0.0;
    let mut steps = Vec::new();
    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(initial_mass);
    proof.feed_f64(k_index);
    proof.feed_f64(n_index);
    proof.feed_f64(commanded);
    proof.feed_f64(ph);
    proof.feed_f64(critical);

    let dt = 0.001;
    for step in 0..100 {
        let t_ms = step as f64;
        if reflex {
            mill_shear += (clamp_target - mill_shear) * 0.12;
        } else {
            mill_shear += (commanded - mill_shear) * 0.10;
        }
        mill_shear = mill_shear.max(0.0);
        state.step(mill_shear, 0.0, dt);

        let tau = mill_tau(k_index, n_index, mill_shear);
        let potency_pct = (state.active_potency * 100.0).clamp(0.0, 100.0);
        let dissolution_pct =
            ((initial_mass - state.solid_mass_kg) / initial_mass * 100.0).clamp(0.0, 100.0);
        let over = tau > critical;
        let collapsed = potency_pct < POTENCY_COLLAPSE_PCT;
        let stalled = dissolution_pct < DISSOLUTION_STALL_PCT;
        if collapsed {
            potency_hist = true;
        }
        end_over = over;
        end_tau = tau;

        if t_first.is_nan() && over {
            t_first = t_ms;
            reflex = true;
            clamp_target = safe_shear(k_index, n_index, critical);
        }
        if t_first.is_finite() && t_arrest.is_nan() && !over && tau <= critical {
            t_arrest = t_ms;
        }

        if record {
            steps.push(Step {
                step: step as u32,
                t_ms,
                mill_shear_s1: mill_shear,
                viscosity_pas: state.viscosity,
                tau_pa: tau,
                accumulated_pa: state.accumulated_shear_stress,
                potency_pct,
                dissolution_pct,
                overstress: over,
                potency_collapsed: collapsed,
                dissolution_stalled: stalled,
                reflex,
                arrested: t_arrest.is_finite() && (t_ms - t_arrest).abs() < 0.5,
            });
        }
        if step % 25 == 0 {
            proof.feed_f64(mill_shear);
            proof.feed_f64(tau);
            proof.feed_f64(potency_pct);
        }
    }

    let halt = if t_first.is_finite() && t_arrest.is_finite() {
        (t_arrest - t_first).max(0.0)
    } else {
        -1.0
    };
    let held = !potency_hist && !end_over && end_tau <= critical;
    if held {
        proof.feed_str("SAMPLE_HELD");
    } else if potency_hist {
        proof.feed_str("POTENCY_COLLAPSED");
    } else {
        proof.feed_str("MILL_OVERSTRESS");
    }

    Trace {
        id,
        state_type: state_type as u32,
        commanded_shear_s1: commanded,
        critical_shear_pa: critical,
        k: k_index,
        n: n_index,
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
        "{}/../../grokd/data/compounding_loop_1000hz.parquet",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut rng = Rng::new(0x434F_4D50);
    let t0 = Instant::now();
    let mut chosen: Option<u32> = None;
    let mut replay = Vec::new();
    for id in 0..2500u32 {
        let tr = integrate(id, &mut rng, false);
        replay.push((id, tr.state_type, tr.held, tr.halt_ms));
        if chosen.is_none()
            && tr.state_type == 2
            && tr.held
            && tr.halt_ms >= 2.0
            && tr.halt_ms <= 16.0
        {
            chosen = Some(id);
        }
    }
    if chosen.is_none() {
        for (id, ty, held, halt) in &replay {
            if *held && *halt >= 2.0 && *halt <= 16.0 {
                let _ = ty;
                chosen = Some(*id);
                break;
            }
        }
    }
    let broth: Vec<_> = replay.iter().filter(|(_, ty, _, _)| *ty == 2).cloned().collect();
    let broth_held = broth.iter().filter(|(_, _, h, _)| *h).count();
    let broth_halted = broth
        .iter()
        .filter(|(_, _, h, halt)| *h && *halt >= 2.0 && *halt <= 16.0)
        .count();
    println!(
        "  scan n=2500  broth={}  broth-held={}  broth-held-with-halt-2-16ms={}",
        broth.len(),
        broth_held,
        broth_halted
    );
    let id = chosen.expect("mill pulse with halt 2–16 ms");
    let mut rng = Rng::new(0x434F_4D50);
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
    println!("  G^G  ·  COMPOUNDING 1000 Hz LOOP TRACE  ·  68Q mill");
    println!(
        "  id={}  broth={}  K={:.4}  n={:.3}  command={:.1} s^-1  gate={:.1} Pa",
        tr.id,
        tr.state_type == 2,
        tr.k,
        tr.n,
        tr.commanded_shear_s1,
        tr.critical_shear_pa
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
        Field::new("compounding_id", DataType::UInt32, false),
        Field::new("step", DataType::UInt32, false),
        Field::new("t_ms", DataType::Float64, false),
        Field::new("mill_shear_s1", DataType::Float64, false),
        Field::new("viscosity_pas", DataType::Float64, false),
        Field::new("tau_pa", DataType::Float64, false),
        Field::new("accumulated_shear_pa", DataType::Float64, false),
        Field::new("potency_pct", DataType::Float64, false),
        Field::new("dissolution_pct", DataType::Float64, false),
        Field::new("overstress", DataType::Boolean, false),
        Field::new("potency_collapsed", DataType::Boolean, false),
        Field::new("dissolution_stalled", DataType::Boolean, false),
        Field::new("reflex_active", DataType::Boolean, false),
        Field::new("arrested", DataType::Boolean, false),
        Field::new("commanded_shear_s1", DataType::Float64, false),
        Field::new("critical_shear_pa", DataType::Float64, false),
        Field::new("halt_ms", DataType::Float64, false),
        Field::new("held", DataType::Boolean, false),
        Field::new("state_type", DataType::UInt32, false),
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
                    .map(|s| Some(s.mill_shear_s1))
                    .collect::<Float64Array>(),
            ),
            Arc::new(
                tr.steps
                    .iter()
                    .map(|s| Some(s.viscosity_pas))
                    .collect::<Float64Array>(),
            ),
            Arc::new(tr.steps.iter().map(|s| Some(s.tau_pa)).collect::<Float64Array>()),
            Arc::new(
                tr.steps
                    .iter()
                    .map(|s| Some(s.accumulated_pa))
                    .collect::<Float64Array>(),
            ),
            Arc::new(
                tr.steps
                    .iter()
                    .map(|s| Some(s.potency_pct))
                    .collect::<Float64Array>(),
            ),
            Arc::new(
                tr.steps
                    .iter()
                    .map(|s| Some(s.dissolution_pct))
                    .collect::<Float64Array>(),
            ),
            Arc::new(
                tr.steps
                    .iter()
                    .map(|s| Some(s.overstress))
                    .collect::<BooleanArray>(),
            ),
            Arc::new(
                tr.steps
                    .iter()
                    .map(|s| Some(s.potency_collapsed))
                    .collect::<BooleanArray>(),
            ),
            Arc::new(
                tr.steps
                    .iter()
                    .map(|s| Some(s.dissolution_stalled))
                    .collect::<BooleanArray>(),
            ),
            Arc::new(tr.steps.iter().map(|s| Some(s.reflex)).collect::<BooleanArray>()),
            Arc::new(
                tr.steps
                    .iter()
                    .map(|s| Some(s.arrested))
                    .collect::<BooleanArray>(),
            ),
            Arc::new((0..n).map(|_| Some(tr.commanded_shear_s1)).collect::<Float64Array>()),
            Arc::new((0..n).map(|_| Some(tr.critical_shear_pa)).collect::<Float64Array>()),
            Arc::new((0..n).map(|_| Some(tr.halt_ms)).collect::<Float64Array>()),
            Arc::new((0..n).map(|_| Some(tr.held)).collect::<BooleanArray>()),
            Arc::new((0..n).map(|_| Some(tr.state_type)).collect::<UInt32Array>()),
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
                "G^G compounding 1000 Hz loop trace — 68Q mill halt".to_string(),
            ),
            parquet::file::metadata::KeyValue::new("compounding_id".to_string(), tr.id.to_string()),
        ]))
        .build();
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    println!("  parquet {out}");
    println!("  seal {run_proof}");
    println!("  {:?}", t0.elapsed());
    let any_halted = replay
        .iter()
        .filter(|(_, _, held, halt)| *held && *halt >= 2.0 && *halt <= 16.0)
        .count();
    println!("  held+halt 2–16 ms: {any_halted} / 2500 scanned");
}
