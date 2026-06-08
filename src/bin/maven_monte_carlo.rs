// G^G Maven Monte Carlo — THE ORBITAL TRUTH
// Exo-atmospheric interceptor, deep space burn, high delta-V tracking.

use std::time::Instant;
use genesis_core::physics::orbital::{
    self, OrbitalPhysics, SatelliteState,
    ReactionWheel, ThrusterSet, RateGyro,
};
use genesis_core::rng::Rng;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output::{self, DatasetMetadata, TrajectoryRecord, Dataset};

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
    let outcome = if mission_success { "KINETIC_INTERCEPT" } else { phase.as_str() };

    proof.feed_f64(target_offset_deg);
    proof.feed_str(outcome);

    TrajectoryResult {
        id, short_id, persona, target_offset_deg, fuel_used: initial_fuel - thrusters.fuel_kg,
        burn_duration: burn_timer, mission_success, outcome, phase, steps: step,
        proof_hash: proof.seal(), intercept_range, telemetry,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n_trajectories: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(10_000);
    let json_output = args.iter().any(|a| a == "--json");
    let out_dir = args.iter().position(|a| a == "--out-dir").and_then(|i| args.get(i + 1)).cloned();

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
}
