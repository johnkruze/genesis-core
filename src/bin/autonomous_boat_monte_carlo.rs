// G^G Autonomous Boat Monte Carlo — THE MARINE TRUTH
// High-speed surface vessel dynamics, wave slamming, and surface current tracking.

use std::time::Instant;
use genesis_core::physics::marine::*;
use genesis_core::rng::Rng;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output::{self, DatasetMetadata, TrajectoryRecord, Dataset};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase { Cruising, WaveSlam, Capsized, Complete }
impl Phase { fn as_str(&self) -> &'static str { match self { Phase::Cruising => "CRUISING", Phase::WaveSlam => "WAVE_SLAM", Phase::Capsized => "CAPSIZED", Phase::Complete => "COMPLETE" } } }

#[derive(Debug)]
#[allow(dead_code)]
struct TrajectoryResult {
    id: u32,
    short_id: String,
    persona: &'static str,
    mission_success: bool,
    max_speed_knots: f64,
    wave_height_m: f64,
    outcome: &'static str,
    phase: Phase,
    steps: usize,
    proof_hash: String,
    sea_state: &'static str,
    telemetry: Vec<serde_json::Value>,
}

fn run_single_trajectory(id: u32, rng: &mut Rng, record_telemetry: bool) -> TrajectoryResult {
    let short_id = output::short_id(rng);
    let persona = "Japan_USV_Interceptor";
    let mass = rng.range(500.0, 1500.0);
    
    // Wave state
    let (sea_state, wave_height_m) = match rng.index(4) {
        0 => ("Calm", rng.range(0.1, 0.5)), // Calm
        1 => ("Choppy", rng.range(0.5, 1.5)), // Choppy
        2 => ("Rough", rng.range(1.5, 3.0)), // Rough
        _ => ("Storm", rng.range(3.0, 6.0)), // Storm
    };

    let dt = 0.05;
    let max_steps = 3000;
    let mut step = 0;
    
    let mut phase = Phase::Cruising;
    let mut speed_ms = 0.0_f64;
    let target_speed_ms = rng.range(10.0, 25.0); // 20-50 knots
    let thrust_force = mass * 5.0; // High power/weight
    
    let mut max_speed = 0.0;
    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(mass);
    proof.feed_f64(wave_height_m);

    let mut telemetry = Vec::new();

    while step < max_steps {
        // Simple 1D surface dynamics for high-speed
        let drag = 0.5 * RHO_SEAWATER * speed_ms * speed_ms * 0.4 * 0.5; // Drag ~ V^2
        let accel = (thrust_force - drag) / mass;
        speed_ms += accel * dt;
        if speed_ms > max_speed { max_speed = speed_ms; }

        if speed_ms > target_speed_ms { speed_ms = target_speed_ms; }

        // Wave interactions
        if wave_height_m > 2.0 && rng.chance(0.01 * wave_height_m) {
            phase = Phase::WaveSlam;
            speed_ms *= 0.6; // Lose speed on slam
            // Capsize risk
            if rng.chance(0.05 * (wave_height_m / 6.0)) {
                phase = Phase::Capsized;
                break;
            }
        } else if phase == Phase::WaveSlam {
            phase = Phase::Cruising;
        }

        if step > 2500 && phase != Phase::Capsized { phase = Phase::Complete; }

        if step % 50 == 0 {
            proof.feed_f64(speed_ms);
            if record_telemetry {
                telemetry.push(serde_json::json!({
                    "t": step as f64 * dt, "phase": phase.as_str(),
                    "speed_knots": (speed_ms * 1.94384 * 10.0).round() / 10.0,
                    "wave_height_m": wave_height_m,
                }));
            }
        }
        step += 1;
    }

    let mission_success = phase == Phase::Complete || phase == Phase::Cruising;
    let outcome = if phase == Phase::Capsized { "HULL_CAPSIZED" } else { "ROUTE_COMPLETE" };

    proof.feed_str(outcome);

    TrajectoryResult {
        id, short_id, persona, mission_success, max_speed_knots: max_speed * 1.94384,
        wave_height_m, outcome, phase, steps: step,
        proof_hash: proof.seal(), sea_state, telemetry,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n_trajectories: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(10_000);
    let json_output = args.iter().any(|a| a == "--json");
    let out_dir = args.iter().position(|a| a == "--out-dir").and_then(|i| args.get(i + 1)).cloned();

    let mut rng = Rng::new(0xB0A7_1933_3333_3333);
    let start = Instant::now();
    let mut results = Vec::with_capacity(n_trajectories as usize);
    let record_telemetry = json_output || out_dir.is_some();

    for i in 0..n_trajectories { results.push(run_single_trajectory(i, &mut rng, record_telemetry)); }

    if !json_output {
        let success = results.iter().filter(|r| r.mission_success).count();
        println!("  G^G AUTONOMOUS BOAT MONTE CARLO | {} runs | {:.2}s", n_trajectories, start.elapsed().as_secs_f64());
        println!("  | ROUTE_COMPLETE:  {:>6} ({:>5.1}%)  |", success, success as f64 / n_trajectories as f64 * 100.0);
    }

    if let Some(base_dir) = out_dir {
        let date_str = &output::now_iso()[0..10]; // YYYY-MM-DD
        let mut grouped: std::collections::HashMap<String, Vec<&TrajectoryResult>> = std::collections::HashMap::new();

        for r in &results {
            grouped.entry(r.sea_state.to_string()).or_default().push(r);
        }

        let mut hash_list = Vec::new();

        for (state_name, state_results) in grouped {
            let dir_path = format!("{}/{}/{}", base_dir, date_str, state_name);
            std::fs::create_dir_all(&dir_path).expect("Failed to create deeply nested data folders");

            let records: Vec<_> = state_results.iter().map(|r| {
                TrajectoryRecord {
                    id: format!("autonomous_boat_{}_{}", r.short_id, state_name),
                    traj_type: "usv_surface_route".to_string(),
                    scenario: format!("{}_high_speed_surface_chase", state_name),
                    steps: r.steps,
                    score: serde_json::json!({ "success": r.mission_success, "max_speed_knots": r.max_speed_knots }),
                    proof_hash: r.proof_hash.clone(), reasoning_context: serde_json::json!({ "outcome": r.outcome, "wave_height_m": r.wave_height_m, "sea_state": state_name }),
                    data: r.telemetry.clone(),
                }
            }).collect();

            let proof_hashes: Vec<String> = state_results.iter().map(|r| r.proof_hash.clone()).collect();
            let run_proof = proof::seal_run(&proof_hashes);
            hash_list.push(run_proof);

            let dataset = Dataset {
                dataset_metadata: DatasetMetadata {
                    generator: "G^G ASV Monte Carlo".to_string(), domain: "marine_surface".to_string(),
                    scenario: "autonomous_boat".to_string(), trajectories: state_results.len(),
                    physics_engine: "genesis_core::marine_surface".to_string(), version: "1.0.0".to_string(), generated_at: output::now_iso(),
                }, trajectories: records,
            };

            let chunk_id = output::short_id(&mut rng);
            let file_path = format!("{}/dataset_{}_{}.json", dir_path, state_name, chunk_id);
            output::write_dataset(&file_path, &dataset).expect("Failed to write dataset");
        }
        let master_proof = proof::seal_run(&hash_list);
        if !json_output {
            println!("  SHA-256 Run Proof: {}", master_proof);
        }
    }
}
