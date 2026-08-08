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
    proof_hash: String,
}

fn run_single_grasp(
    id: u32,
    rng: &mut Rng,
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

    let dt = 0.001f32; // 1000Hz loop (1ms step)
    let total_steps = 100;

    let mut micro_detected = false;
    let mut macro_detected = false;
    let mut rotational_detected = false;
    let mut final_margin = 0.0f32;

    for step in 0..total_steps {
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

        if step % 25 == 0 {
            proof.feed_f64(state.normal_force as f64);
            proof.feed_f64(res.margin as f64);
            proof.feed_f64(state.slip_velocity as f64);
        }
    }

    let reflex_safe = final_margin > 0.15 && state.normal_force <= 45.0 && !macro_detected;

    proof.feed_f64(state.normal_force as f64);
    proof.feed_f64(state.slip_velocity as f64);
    proof.feed_str(if reflex_safe { "TACTILE_REFLEX_SECURE" } else { "SAMPLE_SLIP_DROPPED" });

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
            Arc::new(proofs),
        ],
    ).expect("Failed to create RecordBatch");

    let file = std::fs::File::create(path)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.to_string()),
            parquet::file::metadata::KeyValue::new("generator".to_string(), "G^G Tactile Grasp Reflex Dynamics v1.0".to_string()),
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
        .unwrap_or(1_000);

    let out_parquet = args.iter().position(|a| a == "--parquet")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "../../doe-genesis/topic-4-autonomous-laboratories/data/autolab_dexterous_grasp_tactile.parquet".to_string());

    println!("====================================================================");
    println!("  G^G KERNEL: AUTONOMOUS LAB TACTILE SAMPLE GRASP REFLEX SWEEP");
    println!("  Target Trajectories: {}", n_trajectories);
    println!("  Evaluating 1000Hz Micro-Slip, Friction Margin & 45N Reflex Clamp...");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x4752_4153_505f4c41);
    let start = Instant::now();

    let mut results = Vec::with_capacity(n_trajectories as usize);
    for i in 0..n_trajectories {
        results.push(run_single_grasp(i, &mut rng));
    }

    let proof_hashes: Vec<_> = results.iter().map(|r| r.proof_hash.clone()).collect();
    let run_proof = proof::seal_run(&proof_hashes);

    write_parquet_dataset(&out_parquet, &results, &run_proof)
        .expect("Failed to write Parquet dataset");

    let safe_runs = results.iter().filter(|r| r.reflex_clamped_safe).count();
    let dropped_runs = n_trajectories as usize - safe_runs;

    println!("====================================================================");
    println!("  TACTILE SAMPLE REFLEX SWEEP COMPLETE");
    println!("  Total Trajectories Simulated: {}", n_trajectories);
    println!("  Tactile Reflex Secured Runs:        {} ({:.1}%)", safe_runs, (safe_runs as f64 / n_trajectories as f64) * 100.0);
    println!("  Sample Slip / Drop Failures:        {} ({:.1}%)", dropped_runs, (dropped_runs as f64 / n_trajectories as f64) * 100.0);
    println!("  Master SHA-256 Run Proof:           {}", run_proof);
    println!("  Simulation Time:                    {:?}", start.elapsed());
    println!("  Parquet Dataset Written To:          {}", out_parquet);
    println!("====================================================================\n");
}
