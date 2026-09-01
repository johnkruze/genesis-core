use std::time::Instant;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output;
use genesis_core::rng::Rng;
use genesis_core::physics::dexterous::{
    evaluate_grasp_dynamics, C_TactileArray, Taxel, C_GraspState
};
use serde::Serialize;
use std::sync::Arc;
use arrow::array::{Float64Array, StringArray, BooleanArray};
use arrow::record_batch::RecordBatch;
use arrow::datatypes::{Schema, Field, DataType};
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::file::properties::WriterProperties;
use parquet::basic::Compression;

#[derive(Debug, Serialize)]
struct GraspRunResult {
    id: u32,
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
    t_first_micro_slip_ms: f64,
    t_reflex_active_ms: f64,
    t_slip_arrest_ms: f64,
    halt_ms: f64,
    halt_within_2ms: bool,
    loop_hz: f64,
    proof_hash: String,
}

fn run_single_grasp(
    id: u32,
    rng: &mut Rng,
    dt: f32,
    duration_s: f32,
) -> GraspRunResult {
    let short_id = output::short_id(rng);
    
    // Sweep sample payload mass (0.01 to 2.5 kg), static friction mu (0.1 to 0.8), initial force (5 to 30 N)
    let mass_kg = rng.range(0.01, 2.5) as f32;
    let mu_s = rng.range(0.15, 0.85) as f32;
    let initial_force = rng.range(5.0, 30.0) as f32;
    let disturbances = rng.range(0.0, 0.8) as f32;

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

    let total_steps = ((duration_s / dt).round() as usize).max(1);
    let loop_hz = 1.0f64 / dt as f64;

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
        // Build 4x4 tactile sensor array
        let normal_per_taxel = state.normal_force / 16.0;
        let shear_load = (mass_kg * 9.81 + disturbances * (step as f32 * 0.1)) / 16.0;

        let mut taxels = [Taxel { normal: normal_per_taxel, shear_x: 0.0, shear_y: 0.0 }; 16];
        for i in 0..16 {
            // Apply tangential shear & disturbance torque
            taxels[i].shear_x = shear_load * (1.0 + 0.1 * (i as f32 % 4.0));
            taxels[i].shear_y = shear_load * 0.15 * (i as f32 / 4.0);
        }

        let sensor = C_TactileArray { taxels };
        let res = evaluate_grasp_dynamics(&sensor, &mut state, dt);

        if res.micro_slip_detected { micro_detected = true; }
        if res.macro_slip_detected { macro_detected = true; }
        if res.rotational_slip_detected { rotational_detected = true; }
        final_margin = res.margin;

        if t_first_micro.is_nan()
            && (res.micro_slip_detected || res.macro_slip_detected || res.rotational_slip_detected)
        {
            t_first_micro = t_ms;
        }
        if t_reflex.is_nan() && state.reflex_active {
            t_reflex = t_ms;
        }
        // Arrest: after onset, slip no longer growing and macro is clear.
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

    let reflex_safe = final_margin > 0.15 && state.normal_force <= 45.0 && !macro_detected;
    let halt_ms = if t_first_micro.is_finite() && t_arrest.is_finite() {
        (t_arrest - t_first_micro).max(0.0)
    } else {
        -1.0
    };
    let halt_within_2ms = halt_ms >= 0.0 && halt_ms <= 2.0;

    proof.feed_f64(state.normal_force as f64);
    proof.feed_f64(state.slip_velocity as f64);
    proof.feed_f64(halt_ms);
    proof.feed_str(if reflex_safe { "TACTILE_REFLEX_SECURE" } else { "SAMPLE_SLIP_DROPPED" });
    proof.feed_str(if halt_within_2ms { "HALT_WITHIN_2MS" } else { "HALT_OUTSIDE_2MS" });

    GraspRunResult {
        id,
        short_id,
        object_mass_kg: mass_kg as f64,
        static_friction_mu: mu_s as f64,
        initial_normal_force_n: initial_force as f64,
        final_commanded_force_n: state.normal_force as f64,
        slip_velocity_m_s: state.slip_velocity as f64,
        tactile_friction_margin: final_margin as f64,
        micro_slip_detected: micro_detected,
        macro_slip_detected: macro_detected,
        rotational_slip_detected: rotational_detected,
        reflex_clamped_safe: reflex_safe,
        t_first_micro_slip_ms: if t_first_micro.is_finite() { t_first_micro } else { -1.0 },
        t_reflex_active_ms: if t_reflex.is_finite() { t_reflex } else { -1.0 },
        t_slip_arrest_ms: if t_arrest.is_finite() { t_arrest } else { -1.0 },
        halt_ms,
        halt_within_2ms,
        loop_hz,
        proof_hash: proof.seal(),
    }
}

fn write_parquet_dataset(path: &str, results: &[GraspRunResult], run_proof: &str) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::Utf8, false),
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
        Field::new("t_first_micro_slip_ms", DataType::Float64, false),
        Field::new("t_reflex_active_ms", DataType::Float64, false),
        Field::new("t_slip_arrest_ms", DataType::Float64, false),
        Field::new("halt_ms", DataType::Float64, false),
        Field::new("halt_within_2ms", DataType::Boolean, false),
        Field::new("loop_hz", DataType::Float64, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let ids: StringArray = results.iter().map(|r| Some(format!("grasp_{}", r.short_id))).collect();
    let masses: Float64Array = results.iter().map(|r| Some(r.object_mass_kg)).collect();
    let mus: Float64Array = results.iter().map(|r| Some(r.static_friction_mu)).collect();
    let f_inits: Float64Array = results.iter().map(|r| Some(r.initial_normal_force_n)).collect();
    let f_finals: Float64Array = results.iter().map(|r| Some(r.final_commanded_force_n)).collect();
    let v_slips: Float64Array = results.iter().map(|r| Some(r.slip_velocity_m_s)).collect();
    let margins: Float64Array = results.iter().map(|r| Some(r.tactile_friction_margin)).collect();
    let micros: BooleanArray = results.iter().map(|r| Some(r.micro_slip_detected)).collect();
    let macros: BooleanArray = results.iter().map(|r| Some(r.macro_slip_detected)).collect();
    let rots: BooleanArray = results.iter().map(|r| Some(r.rotational_slip_detected)).collect();
    let safes: BooleanArray = results.iter().map(|r| Some(r.reflex_clamped_safe)).collect();
    let t_micro: Float64Array = results.iter().map(|r| Some(r.t_first_micro_slip_ms)).collect();
    let t_ref: Float64Array = results.iter().map(|r| Some(r.t_reflex_active_ms)).collect();
    let t_arr: Float64Array = results.iter().map(|r| Some(r.t_slip_arrest_ms)).collect();
    let halts: Float64Array = results.iter().map(|r| Some(r.halt_ms)).collect();
    let halt2: BooleanArray = results.iter().map(|r| Some(r.halt_within_2ms)).collect();
    let hz: Float64Array = results.iter().map(|r| Some(r.loop_hz)).collect();
    let proofs: StringArray = results.iter().map(|r| Some(r.proof_hash.clone())).collect();

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(ids),
            Arc::new(masses),
            Arc::new(mus),
            Arc::new(f_inits),
            Arc::new(f_finals),
            Arc::new(v_slips),
            Arc::new(margins),
            Arc::new(micros),
            Arc::new(macros),
            Arc::new(rots),
            Arc::new(safes),
            Arc::new(t_micro),
            Arc::new(t_ref),
            Arc::new(t_arr),
            Arc::new(halts),
            Arc::new(halt2),
            Arc::new(hz),
            Arc::new(proofs),
        ],
    ).expect("Failed to create RecordBatch");

    let file = std::fs::File::create(path)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.to_string()),
            parquet::file::metadata::KeyValue::new("generator".to_string(), "G^G Tactile Grasp Reflex Dynamics v1.1 halt-column".to_string()),
        ]))
        .build();

    let mut writer = ArrowWriter::try_new(file, schema, Some(props))
        .expect("Failed to create Parquet ArrowWriter");
    writer.write(&batch).expect("Failed to write Parquet batch");
    writer.close().expect("Failed to close Parquet writer");

    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n_trajectories: u32 = args.get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(2_500);

    let hz: f64 = args.iter().position(|a| a == "--hz")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000.0);
    let dt = (1.0 / hz) as f32;
    let duration_s = 0.100f32; // 100 ms window — 100 steps at 1 kHz, 500 at 5 kHz

    let default_parquet = if (hz - 5000.0).abs() < 1.0 {
        "../../doe-genesis/topic-4-autonomous-laboratories/data/autolab_dexterous_grasp_tactile_5khz.parquet"
    } else {
        "../../doe-genesis/topic-4-autonomous-laboratories/data/autolab_dexterous_grasp_tactile.parquet"
    };
    let out_parquet = args.iter().position(|a| a == "--parquet")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| default_parquet.to_string());

    println!("====================================================================");
    println!("  G^G KERNEL: AUTONOMOUS LAB TACTILE SAMPLE GRASP REFLEX SWEEP");
    println!("  Target Trajectories: {}", n_trajectories);
    println!("  Loop: {:.0} Hz  dt={:.4} ms  window={:.0} ms", hz, dt * 1000.0, duration_s * 1000.0);
    println!("  Halt column: t_slip_arrest − t_first_micro  (2 ms gate)");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x4752_4153_505f4c41);
    let start = Instant::now();

    let mut results = Vec::with_capacity(n_trajectories as usize);
    for i in 0..n_trajectories {
        results.push(run_single_grasp(i, &mut rng, dt, duration_s));
    }

    let proof_hashes: Vec<_> = results.iter().map(|r| r.proof_hash.clone()).collect();
    let run_proof = proof::seal_run(&proof_hashes);

    write_parquet_dataset(&out_parquet, &results, &run_proof)
        .expect("Failed to write Parquet dataset");

    let safe_runs = results.iter().filter(|r| r.reflex_clamped_safe).count();
    let dropped_runs = n_trajectories as usize - safe_runs;
    let halt2 = results.iter().filter(|r| r.halt_within_2ms).count();
    let halt_events = results.iter().filter(|r| r.halt_ms >= 0.0).count();

    println!("====================================================================");
    println!("  TACTILE SAMPLE REFLEX SWEEP COMPLETE");
    println!("  Total Trajectories Simulated: {}", n_trajectories);
    println!("  Tactile Reflex Secured Runs:        {} ({:.1}%)", safe_runs, (safe_runs as f64 / n_trajectories as f64) * 100.0);
    println!("  Sample Slip / Drop Failures:        {} ({:.1}%)", dropped_runs, (dropped_runs as f64 / n_trajectories as f64) * 100.0);
    println!("  Halt events (onset+arrest):         {} ({:.1}%)", halt_events, (halt_events as f64 / n_trajectories as f64) * 100.0);
    println!("  Halt within 2 ms:                   {} ({:.1}%)", halt2, (halt2 as f64 / n_trajectories as f64) * 100.0);
    println!("  Master SHA-256 Run Proof:           {}", run_proof);
    println!("  Simulation Time:                    {:?}", start.elapsed());
    println!("  Parquet Dataset Written To:          {}", out_parquet);
    println!("====================================================================\n");
}
