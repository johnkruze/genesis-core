// G^G Maven Monte Carlo — THE ORBITAL TRUTH
// Exo-atmospheric interceptor, deep space burn, high delta-V tracking.

use std::sync::Arc;
use std::time::Instant;
use genesis_core::physics::orbital::{
    self, OrbitalPhysics, SatelliteState,
    ReactionWheel, ThrusterSet, RateGyro,
};
use genesis_core::rng::Rng;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output::{self, DatasetMetadata, TrajectoryRecord, Dataset};
use arrow::array::{BooleanArray, Float64Array, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::ArrowWriter;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Search,
    TrackingLock,
    MainEngineBurn,
    Intercepted,
    TargetLost,
}

impl Phase { fn as_str(&self) -> &'static str { match self { Phase::Search => "SEARCH", Phase::TrackingLock => "TRACKING_LOCK", Phase::MainEngineBurn => "MAIN_ENGINE_BURN", Phase::Intercepted => "INTERCEPTED", Phase::TargetLost => "TARGET_LOST" } } }

#[derive(Debug)]
#[allow(dead_code)]
struct TrajectoryResult {
    id: u32,
    short_id: String,
    persona: &'static str,
    target_offset_deg: f64,
    fuel_used: f64,
    burn_duration: f64,
    mission_success: bool,
    outcome: &'static str,
    phase: Phase,
    steps: usize,
    proof_hash: String,
    intercept_range: &'static str,
    is_held: bool,
    is_long_range: bool,
    is_target_lost: bool,
    telemetry: Vec<serde_json::Value>,
}

fn run_single_trajectory(id: u32, rng: &mut Rng, record_telemetry: bool) -> TrajectoryResult {
    let short_id = output::short_id(rng);
    let persona = "MAVEN_Interceptor";
    let (intercept_range, mut target_offset_deg) = match rng.index(3) {
        0 => ("Close_Range", rng.range(10.0, 45.0)),
        1 => ("Medium_Range", rng.range(45.0, 90.0)),
        _ => ("Long_Range", rng.range(90.0, 180.0)),
    };
    let mass = rng.range(2500.0, 4000.0);
    let ix = mass * 2.0; let iy = mass * 2.5; let iz = mass * 1.5;

    let mut state = SatelliteState {
        position: [40000.0, 0.0, 0.0], // Deep space GEO / beyond
        velocity: [0.0, 3.0, 0.0],
        quaternion_attitude: [1.0, 0.0, 0.0, 0.0],
        angular_velocity: [0.0, 0.0, 0.0],
        inertia_tensor: [[ix, 0.0, 0.0], [0.0, iy, 0.0], [0.0, 0.0, iz]],
    };

    let mut wheels = [ReactionWheel::new(2.0, 100.0), ReactionWheel::new(2.0, 100.0), ReactionWheel::new(2.0, 100.0)];
    let initial_fuel = 800.0;
    // High thrust main engine setup for intercept
    let mut thrusters = ThrusterSet::new(500.0, 310.0, initial_fuel);
    let mut gyro = RateGyro::new(0.0001, 0.00001); // High precision

    let dt = 0.05;
    let max_steps = 4_000;
    let mut phase = Phase::Search;
    let mut step = 0;
    
    let target_lost_chance = rng.chance(0.03);
    let mut burn_timer = 0.0_f64;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(mass);

    let mut telemetry = Vec::new();

    while step < max_steps {
        let t = step as f64 * dt;
        let _omega = gyro.read(&state.angular_velocity, rng, dt);
        let mut total_torque = [0.0; 3];

        if target_lost_chance && step > 1000 && phase != Phase::Intercepted { phase = Phase::TargetLost; break; }

        match phase {
            Phase::Search => { // Slew to target
                target_offset_deg -= 5.0 * dt; // Slewing at 5 deg/s
                if target_offset_deg <= 0.5 { target_offset_deg = 0.0; phase = Phase::TrackingLock; }
                for i in 0..3 { total_torque[i] = wheels[i].command(if i==0 {0.5} else {0.0}, dt); } // Panning torque
            }
            Phase::TrackingLock => {
                if step > 1500 { phase = Phase::MainEngineBurn; } // Vetting complete, fire
            }
            Phase::MainEngineBurn => {
                let thr_tq = thrusters.fire([0.0, 0.0, 50.0], dt); // Massive Z-axis burn (or whatever main engine)
                for i in 0..3 { total_torque[i] += thr_tq[i]; }
                burn_timer += dt;
                
                // Burn complete logic
                if burn_timer > rng.range(20.0, 45.0) { phase = Phase::Intercepted; break; }
            }
            _ => break,
        }

        OrbitalPhysics::step_attitude(&mut state, &total_torque, dt);

        if step % 50 == 0 {
            proof.feed_f64(target_offset_deg);
            if record_telemetry {
                telemetry.push(serde_json::json!({
                    "t": t, "phase": phase.as_str(), "target_offset_deg": (target_offset_deg * 10.0).round() / 10.0,
                    "fuel_kg": (thrusters.fuel_kg * 10.0).round() / 10.0, "omega_mag": orbital::omega_magnitude(&state.angular_velocity),
                }));
            }
        }
        step += 1;
    }

    let mission_success = phase == Phase::Intercepted;
    let is_target_lost = phase == Phase::TargetLost;
    let is_long_range = !is_target_lost && intercept_range == "Long_Range";
    let is_held = !is_target_lost && !is_long_range;
    let class = if is_target_lost {
        "TARGET_LOST"
    } else if is_long_range {
        "LONG_RANGE"
    } else {
        "HELD"
    };
    let outcome = if mission_success { "KINETIC_INTERCEPT" } else { class };

    proof.feed_f64(target_offset_deg);
    proof.feed_str(class);

    TrajectoryResult {
        id, short_id, persona, target_offset_deg, fuel_used: initial_fuel - thrusters.fuel_kg,
        burn_duration: burn_timer, mission_success, outcome, phase, steps: step,
        proof_hash: proof.seal(), intercept_range,
        is_held, is_long_range, is_target_lost, telemetry,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n_trajectories: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(2_500);
    let json_output = args.iter().any(|a| a == "--json");
    let out_dir = args.iter().position(|a| a == "--out-dir").and_then(|i| args.get(i + 1)).cloned();
    let parquet_path = args.iter().position(|a| a == "--parquet")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| {
            format!(
                "{}/../../data/exports/sovereign/maven_intercept.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    let parquet_path = args.iter().position(|a| a == "--parquet")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| {
            format!(
                "{}/../../data/exports/sovereign/maven_intercept.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    let mut rng = Rng::new(0x1A7E_C3E0_7777_7777);
    let start = Instant::now();
    let record_telemetry = json_output || out_dir.is_some();
    let mut results = Vec::with_capacity(n_trajectories as usize);

    for i in 0..n_trajectories { results.push(run_single_trajectory(i, &mut rng, record_telemetry)); }

    if !json_output {
        let success = results.iter().filter(|r| r.mission_success).count();
        println!("  G^G MAVEN INTERCEPTOR MONTE CARLO | {} runs | {:.2}s", n_trajectories, start.elapsed().as_secs_f64());
        println!("  | KINETIC INTERCEPT:{:>6} ({:>5.1}%)  |", success, success as f64 / n_trajectories as f64 * 100.0);
    }

    if let Some(base_dir) = out_dir {
        let date_str = &output::now_iso()[0..10];
        let mut grouped: std::collections::HashMap<String, Vec<&TrajectoryResult>> = std::collections::HashMap::new();

        for r in &results {
            grouped.entry(r.intercept_range.to_string()).or_default().push(r);
        }

        let mut hash_list = Vec::new();

        for (range_name, range_results) in grouped {
            let dir_path = format!("{}/{}/{}", base_dir, date_str, range_name);
            std::fs::create_dir_all(&dir_path).expect("Failed to create deeply nested data folders");

            let records: Vec<_> = range_results.iter().map(|r| {
                TrajectoryRecord {
                    id: format!("maven_{}_{}", r.short_id, range_name), traj_type: "exo_intercept".to_string(),
                    scenario: format!("{}_high_dv", range_name), steps: r.steps,
                    score: serde_json::json!({ "success": r.mission_success, "fuel_used": r.fuel_used, "burn_duration": r.burn_duration }),
                    proof_hash: r.proof_hash.clone(), reasoning_context: serde_json::json!({ "outcome": r.outcome, "intercept_range": range_name }),
                    data: r.telemetry.clone(),
                }
            }).collect();

            let proof_hashes: Vec<String> = range_results.iter().map(|r| r.proof_hash.clone()).collect();
            let run_proof = proof::seal_run(&proof_hashes);
            hash_list.push(run_proof);

            let dataset = Dataset {
                dataset_metadata: DatasetMetadata {
                    generator: "G^G Maven Interceptor Monte Carlo".to_string(), domain: "orbital_military".to_string(),
                    scenario: "high_dv_intercept".to_string(), trajectories: range_results.len(),
                    physics_engine: "genesis_core::orbital".to_string(), version: "1.0.0".to_string(), generated_at: output::now_iso(),
                }, trajectories: records,
            };

            let chunk_id = output::short_id(&mut rng);
            let file_path = format!("{}/dataset_{}_{}.json", dir_path, range_name, chunk_id);
            output::write_dataset(&file_path, &dataset).expect("Failed to write dataset");
        }
        let master_proof = proof::seal_run(&hash_list);
        if !json_output {
            println!("  SHA-256 Run Proof: {}", master_proof);
        }
    }

    let proofs: Vec<String> = results.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    write_maven_parquet(&results, &seal, &parquet_path);
}

fn write_maven_parquet(results: &[TrajectoryResult], seal: &str, out: &str) {
    if let Some(p) = std::path::Path::new(out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let n = results.len();
    let unique: std::collections::HashSet<&str> = results.iter().map(|r| r.proof_hash.as_str()).collect();
    assert_eq!(unique.len(), n, "proof_hash must be unique");
    let both = results.iter().filter(|r| {
        (r.is_held as u8) + (r.is_long_range as u8) + (r.is_target_lost as u8) != 1
    }).count();
    assert_eq!(both, 0, "exclusive partition");
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("intercept_range", DataType::Utf8, false),
        Field::new("target_offset_deg", DataType::Float64, false),
        Field::new("fuel_used_kg", DataType::Float64, false),
        Field::new("burn_duration_s", DataType::Float64, false),
        Field::new("mission_success", DataType::Boolean, false),
        Field::new("is_held", DataType::Boolean, false),
        Field::new("is_long_range", DataType::Boolean, false),
        Field::new("is_target_lost", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(results.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(results.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(StringArray::from(results.iter().map(|r| r.intercept_range).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(results.iter().map(|r| r.target_offset_deg).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(results.iter().map(|r| r.fuel_used).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(results.iter().map(|r| r.burn_duration).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(results.iter().map(|r| r.mission_success).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(results.iter().map(|r| r.is_held).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(results.iter().map(|r| r.is_long_range).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(results.iter().map(|r| r.is_target_lost).collect::<Vec<_>>())),
            Arc::new(StringArray::from(results.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(out).unwrap();
    let props = output::parquet_receipt_properties(seal, "G^G maven intercept dual-regime v1.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let nf = n as f64;
    let held = results.iter().filter(|r| r.is_held).count();
    let long = results.iter().filter(|r| r.is_long_range).count();
    let lost = results.iter().filter(|r| r.is_target_lost).count();
    eprintln!(
        "  exclusive held {held} ({:.1}%)  long-range {long} ({:.1}%)  target-lost {lost} ({:.1}%)",
        100.0 * held as f64 / nf,
        100.0 * long as f64 / nf,
        100.0 * lost as f64 / nf
    );
    eprintln!("  unique proofs {n}  seal {seal}\n  parquet {out}");
}
