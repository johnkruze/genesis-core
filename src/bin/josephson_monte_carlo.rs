use genesis_core::physics::josephson::JosephsonState;
use genesis_core::proof::seal_run;
use rayon::prelude::*;
use std::fs::File;
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{BooleanArray, Float64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;

#[derive(Clone)]
struct RunConfig {
    temp_mk: f64,
    pulse_amplitude_na: f64,
    control_frequency_ghz: f64,
    pulse_duration_ns: f64,
    initial_phase: f64,
    trajectory_id: usize,
}

struct RunResult {
    quenched: bool,
    final_coherence: f64,
    max_residual: f64,
    is_thermal_quench: bool,
    is_drive_residual_failed: bool,
    is_coherent: bool,
    proof_hash: String,
}

fn simulate(cfg: &RunConfig) -> RunResult {
    let mut qubit = JosephsonState::new();
    qubit.temp_mk = cfg.temp_mk;
    qubit.phase = cfg.initial_phase;

    let dt: f64 = 0.002; // 2 ps timestep (500 GHz solver rate)
    let total_time_ns: f64 = 50.0; // Run for 50 ns (25,000 steps)

    let steps = (total_time_ns / dt).round() as usize;

    let mut max_residual = 0.0;
    let omega_drive = 2.0 * std::f64::consts::PI * cfg.control_frequency_ghz; // rad/ns

    for step in 0..steps {
        if qubit.quenched { break; }

        let t = step as f64 * dt;
        
        // Define control pulse: Sinusoidal envelope that shuts off after pulse_duration
        let control_input = if t < cfg.pulse_duration_ns {
            cfg.pulse_amplitude_na * (omega_drive * t).sin()
        } else {
            0.0
        };

        // Seed RNG dynamically based on trajectory ID and step count
        let seed = (cfg.trajectory_id as u64) ^ (step as u64).wrapping_mul(0x9e3779b97f4a7c15);

        qubit.step(dt, control_input, seed);
        
        if qubit.residual > max_residual {
            max_residual = qubit.residual;
        }
    }

    let thermal = qubit.quenched && cfg.temp_mk >= 50.0;
    let drive = max_residual > 1e-5 || (qubit.quenched && cfg.temp_mk < 50.0);
    RunResult {
        quenched: qubit.quenched,
        final_coherence: qubit.coherence,
        max_residual,
        is_thermal_quench: thermal,
        is_drive_residual_failed: drive,
        is_coherent: !qubit.quenched && qubit.coherence >= 0.005,
        proof_hash: qubit.get_sealed_hash(),
    }
}

fn main() {
    println!("=== G^G KERNEL: QUANTUM JOSEPHSON PHASE COHERENCE SWEEP ===");
    let args: Vec<String> = std::env::args().collect();
    
    let limit: Option<usize> = args.get(1)
        .and_then(|s| s.parse().ok());

    let out_path = args.iter().position(|a| a == "--out").or_else(|| args.iter().position(|a| a == "--parquet"))
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| {
            if limit.is_some() {
                "../../grokd/data/josephson_coherence_envelope.parquet".to_string()
            } else {
                concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/products/josephson_quantum_coherence.parquet").to_string()
            }
        });

    // Create target directory if it doesn't exist
    if let Some(parent) = std::path::Path::new(&out_path).parent() {
        std::fs::create_dir_all(parent).unwrap();
    }

    let start = Instant::now();

    // Full 5-D grid is 100k. Campaign n samples the box with LCG (truncate would bias cold temps).
    let mut configs = Vec::new();
    if let Some(l) = limit {
        use genesis_core::rng::Rng;
        let mut rng = Rng::new(0x4a4f_5345_50485f51);
        for trajectory_id in 0..l {
            configs.push(RunConfig {
                temp_mk: rng.range(10.0, 100.0),
                pulse_amplitude_na: rng.range(0.0, 45.0),
                control_frequency_ghz: rng.range(3.0, 4.98),
                pulse_duration_ns: rng.range(5.0, 23.0),
                initial_phase: rng.range(0.0, 1.35),
                trajectory_id,
            });
        }
        println!("LCG sample of RCSJ box: {} trajectories.", l);
    } else {
        let mut trajectory_id = 0;
        for t_idx in 0..10 {
            let temp_mk = 10.0 + (t_idx as f64 * 10.0);
            for a_idx in 0..10 {
                let pulse_amplitude_na = a_idx as f64 * 5.0;
                for f_idx in 0..10 {
                    let control_frequency_ghz = 3.0 + (f_idx as f64 * 0.22);
                    for d_idx in 0..10 {
                        let pulse_duration_ns = 5.0 + (d_idx as f64 * 2.0);
                        for p_idx in 0..10 {
                            configs.push(RunConfig {
                                temp_mk,
                                pulse_amplitude_na,
                                control_frequency_ghz,
                                pulse_duration_ns,
                                initial_phase: p_idx as f64 * 0.15,
                                trajectory_id,
                            });
                            trajectory_id += 1;
                        }
                    }
                }
            }
        }
    }

    let total_runs = configs.len();
    println!("Total trajectories to simulate: {}", total_runs);

    // 2. Setup Arrow / Parquet Writer
    let schema = Arc::new(Schema::new(vec![
        Field::new("temp_mk", DataType::Float64, false),
        Field::new("pulse_amplitude_na", DataType::Float64, false),
        Field::new("control_frequency_ghz", DataType::Float64, false),
        Field::new("pulse_duration_ns", DataType::Float64, false),
        Field::new("initial_phase", DataType::Float64, false),
        Field::new("quenched", DataType::Boolean, false),
        Field::new("final_coherence", DataType::Float64, false),
        Field::new("max_residual", DataType::Float64, false),
        Field::new("is_thermal_quench", DataType::Boolean, false),
        Field::new("is_drive_residual_failed", DataType::Boolean, false),
        Field::new("is_coherent", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    println!("  Integrating RCSJ box...");
    let paired: Vec<(RunConfig, RunResult)> = configs
        .into_par_iter()
        .map(|cfg| {
            let res = simulate(&cfg);
            (cfg, res)
        })
        .collect();

    let mut all_proof_hashes = Vec::with_capacity(total_runs);
    let mut quench_count = 0;
    let mut residual_violations = 0;
    let mut thermal_n = 0;
    let mut drive_n = 0;
    let mut coherent_n = 0;
    for (_, res) in &paired {
        all_proof_hashes.push(res.proof_hash.clone());
        if res.quenched {
            quench_count += 1;
        }
        if res.max_residual > 1e-5 {
            residual_violations += 1;
        }
        if res.is_thermal_quench {
            thermal_n += 1;
        }
        if res.is_drive_residual_failed {
            drive_n += 1;
        }
        if res.is_coherent {
            coherent_n += 1;
        }
    }
    let master_hash = seal_run(&all_proof_hashes);

    let file = File::create(&out_path).expect("Failed to create output Parquet file");
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), master_hash.clone()),
            parquet::file::metadata::KeyValue::new("generator".to_string(), "G^G Josephson RCSJ coherence envelope v1.1".to_string()),
        ]))
        .build();
    let mut writer = ArrowWriter::try_new(file, schema.clone(), Some(props))
        .expect("Failed to create ArrowWriter");

    let mut temp_mk = Vec::with_capacity(total_runs);
    let mut pulse_amplitude_na = Vec::with_capacity(total_runs);
    let mut control_frequency_ghz = Vec::with_capacity(total_runs);
    let mut pulse_duration_ns = Vec::with_capacity(total_runs);
    let mut initial_phase = Vec::with_capacity(total_runs);
    let mut quenched = Vec::with_capacity(total_runs);
    let mut final_coherence = Vec::with_capacity(total_runs);
    let mut max_residual = Vec::with_capacity(total_runs);
    let mut thermal = Vec::with_capacity(total_runs);
    let mut drive = Vec::with_capacity(total_runs);
    let mut coherent = Vec::with_capacity(total_runs);
    let mut proof_hash_vec = Vec::with_capacity(total_runs);
    for (cfg, res) in &paired {
        temp_mk.push(cfg.temp_mk);
        pulse_amplitude_na.push(cfg.pulse_amplitude_na);
        control_frequency_ghz.push(cfg.control_frequency_ghz);
        pulse_duration_ns.push(cfg.pulse_duration_ns);
        initial_phase.push(cfg.initial_phase);
        quenched.push(res.quenched);
        final_coherence.push(res.final_coherence);
        max_residual.push(res.max_residual);
        thermal.push(res.is_thermal_quench);
        drive.push(res.is_drive_residual_failed);
        coherent.push(res.is_coherent);
        proof_hash_vec.push(res.proof_hash.clone());
    }
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(Float64Array::from(temp_mk)),
            Arc::new(Float64Array::from(pulse_amplitude_na)),
            Arc::new(Float64Array::from(control_frequency_ghz)),
            Arc::new(Float64Array::from(pulse_duration_ns)),
            Arc::new(Float64Array::from(initial_phase)),
            Arc::new(BooleanArray::from(quenched)),
            Arc::new(Float64Array::from(final_coherence)),
            Arc::new(Float64Array::from(max_residual)),
            Arc::new(BooleanArray::from(thermal)),
            Arc::new(BooleanArray::from(drive)),
            Arc::new(BooleanArray::from(coherent)),
            Arc::new(StringArray::from(proof_hash_vec)),
        ],
    )
    .expect("Failed to create RecordBatch");
    writer.write(&batch).expect("Failed to write RecordBatch");
    writer.close().expect("Failed to close ArrowWriter");

    println!("\n=========================================================");
    println!("JOSEPHSON RCSJ COHERENCE ENVELOPE COMPLETE.");
    println!("TOTAL TRAJECTORIES: {:?}", total_runs);
    println!("COHERENT (50 ns):              {} ({:.2}%)", coherent_n, (coherent_n as f64 / total_runs as f64) * 100.0);
    println!("THERMAL QUENCH:                {} ({:.2}%)", thermal_n, (thermal_n as f64 / total_runs as f64) * 100.0);
    println!("DRIVE / RESIDUAL FAIL:         {} ({:.2}%)", drive_n, (drive_n as f64 / total_runs as f64) * 100.0);
    println!("QUBIT DECOHERENCE/QUENCH:      {} ({:.2}%)", quench_count, (quench_count as f64 / total_runs as f64) * 100.0);
    println!("ZTP RESIDUAL ANOMALIES:        {}", residual_violations);
    println!("MASTER RUN PROOF HASH: {}", master_hash);
    println!("SWEEP TIME: {:?}", start.elapsed());
    println!("DATA WRITTEN TO: {}", out_path);
    println!("=========================================================\n");
}
