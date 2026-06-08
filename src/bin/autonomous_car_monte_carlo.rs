// G^G Autonomous Car Monte Carlo — THE TERRAN TRUTH
// High-speed rigid body kinematics and tire friction on substrate.

use std::time::Instant;
use genesis_core::physics::terran::{
    Locomotion, RobotContact,
};
use genesis_core::rng::Rng;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output::{self, Dataset, DatasetMetadata, TrajectoryRecord};

// ─── FAILURE MODES ──────────────────────────────────────────────
#[derive(Debug, Clone, Copy)]
enum FailureMode {
    TractionLoss, // Hydroplaning or sliding
    Collision,    // Obstacle avoidance failure
    SensorBlind,  // LiDAR/Camera fault
}

// ─── MISSION PHASES ─────────────────────────────────────────────
#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Cruising,
    EvasiveManeuver,
    Braking,
    Complete,
    Failed,
}

impl Phase {
    fn as_str(&self) -> &'static str {
        match self {
            Phase::Cruising => "CRUISING",
            Phase::EvasiveManeuver => "EVASIVE_MANEUVER",
            Phase::Braking => "BRAKING",
            Phase::Complete => "COMPLETE",
            Phase::Failed => "FAILED",
        }
    }
}

// ─── TRAJECTORY RESULT ──────────────────────────────────────────
#[derive(Debug)]
#[allow(dead_code)]
struct TrajectoryResult {
    id: u32,
    short_id: String,
    persona: &'static str,
    robot_mass: f64,
    footprint: f64,
    locomotion: Locomotion,
    weather: &'static str,
    mission_success: bool,
    outcome: &'static str,
    phase: Phase,
    steps: usize,
    proof_hash: String,
    failure: Option<FailureMode>,
    telemetry: Vec<serde_json::Value>,
}

fn run_single_trajectory(
    id: u32,
    rng: &mut Rng,
    record_telemetry: bool,
) -> TrajectoryResult {
    let short_id = output::short_id(rng);

    // Weather condition impacts traction and sensors
    let (weather, friction_coef, blind_chance) = match rng.index(4) {
        0 => ("Clear", 0.9, 0.01),
        1 => ("Rain", 0.5, 0.05),
        2 => ("Snow", 0.3, 0.10),
        _ => ("Fog", 0.8, 0.15),
    };

    let persona = "Autonomous_Car";
    let locomotion = Locomotion::Wheeled;
    let robot_mass = rng.range(1500.0, 3000.0); // 1.5 - 3 tons
    let footprint = 0.15; // Tire contact patch approx

    let _robot = RobotContact {
        mass_kg: robot_mass,
        footprint_m2: footprint,
        locomotion,
    };

    // Failures
    let failure = if rng.chance(0.04) { Some(FailureMode::TractionLoss) }
    else if rng.chance(0.03) { Some(FailureMode::Collision) }
    else if rng.chance(blind_chance) { Some(FailureMode::SensorBlind) }
    else { None };

    let dt = 0.02; // 50 Hz control loop
    let max_steps = 3_000;
    let mut phase = Phase::Cruising;
    let mut step = 0_usize;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(robot_mass);
    proof.feed_str(weather);

    let mut telemetry = Vec::new();

    let mut speed_kph = rng.range(60.0, 120.0);
    let mut steering_angle = 0.0_f64;
    let mut lidar_confidence = 1.0_f64;
    let mut tire_slip = 0.0_f64;

    while step < max_steps {
        let t = step as f64 * dt;

        match phase {
            Phase::Cruising => {
                lidar_confidence = 1.0 - rng.range(0.0, blind_chance * 2.0);
                tire_slip = rng.range(0.0, 0.02) / friction_coef;

                if matches!(failure, Some(FailureMode::SensorBlind)) && lidar_confidence < 0.6 && rng.chance(0.05) {
                    phase = Phase::Braking; // Aegis reflex to brake
                } else if matches!(failure, Some(FailureMode::Collision)) && rng.chance(0.005) {
                    phase = Phase::EvasiveManeuver;
                } else if step > 2000 {
                    phase = Phase::Complete;
                }
            }
            Phase::EvasiveManeuver => {
                steering_angle = rng.range(10.0, 25.0); // Hard cut
                tire_slip = (speed_kph / 100.0) * steering_angle / (friction_coef * 20.0);
                speed_kph -= 10.0 * dt; // Scrubbing speed

                if matches!(failure, Some(FailureMode::TractionLoss)) && tire_slip > 1.0 {
                    phase = Phase::Failed; // Spun out / Collided
                } else if steering_angle < 5.0 || speed_kph < 30.0 {
                    phase = Phase::Complete;
                }
            }
            Phase::Braking => {
                speed_kph -= rng.range(15.0, 30.0) * dt; // Hard braking
                if speed_kph <= 0.0 {
                    speed_kph = 0.0;
                    if matches!(failure, Some(FailureMode::SensorBlind)) {
                        phase = Phase::Failed; // System halt due to blindness
                    } else {
                        phase = Phase::Complete;
                    }
                }
            }
            Phase::Complete | Phase::Failed => break,
        }

        if step % 20 == 0 {
            proof.feed_f64(speed_kph);
            if record_telemetry {
                telemetry.push(serde_json::json!({
                    "t": t,
                    "phase": phase.as_str(),
                    "speed_kph": (speed_kph * 10.0).round() / 10.0,
                    "steering_angle_deg": (steering_angle * 10.0).round() / 10.0,
                    "lidar_confidence": (lidar_confidence * 100.0).round() / 100.0,
                    "tire_slip_ratio": (tire_slip * 1000.0).round() / 1000.0,
                }));
            }
        }
        step += 1;
    }

    let mission_success = phase == Phase::Complete;
    let outcome = if phase == Phase::Failed {
        match failure {
            Some(FailureMode::TractionLoss) => "SPUN_OUT",
            Some(FailureMode::Collision) => "COLLISION_IMPACT",
            Some(FailureMode::SensorBlind) => "SENSOR_BLACKOUT_HALT",
            None => "UNKNOWN_FAILURE",
        }
    } else {
        "ROUTE_COMPLETE"
    };

    proof.feed_str(outcome);
    let proof_hash = proof.seal();

    TrajectoryResult {
        id, short_id, persona, robot_mass, footprint, locomotion, weather,
        mission_success, outcome, phase, steps: step,
        proof_hash, failure, telemetry,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n_trajectories: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(10_000);
    let json_output = args.iter().any(|a| a == "--json");
    let out_dir = args.iter().position(|a| a == "--out-dir").and_then(|i| args.get(i + 1)).cloned();

    if !json_output {
        println!("====================================================================");
        println!("  G^G AUTONOMOUS CAR MONTE CARLO — KINEMATICS & TRACTION");
        println!("====================================================================");
    }

    let mut rng = Rng::new(0xCA5_CA4_B007_A11E);
    let start = Instant::now();
    let record_telemetry = json_output || out_dir.is_some();
    let mut results = Vec::with_capacity(n_trajectories as usize);

    for i in 0..n_trajectories {
        results.push(run_single_trajectory(i, &mut rng, record_telemetry));
    }

    let elapsed = start.elapsed();

    if !json_output {
        let total = results.len();
        let success = results.iter().filter(|r| r.mission_success).count();
        let failures = total - success;
        println!("  Elapsed: {:.3}s | {} Trajectories", elapsed.as_secs_f64(), total);
        println!("  | ROUTE_COMPLETE:  {:>6} ({:>5.1}%)  |", success, success as f64 / total as f64 * 100.0);
        println!("  | FAILURES:        {:>6} ({:>5.1}%)  |", failures, failures as f64 / total as f64 * 100.0);
    }

    if let Some(base_dir) = out_dir {
        let date_str = &output::now_iso()[0..10]; // YYYY-MM-DD
        let mut grouped: std::collections::HashMap<String, Vec<&TrajectoryResult>> = std::collections::HashMap::new();

        for r in &results {
            grouped.entry(r.weather.to_string()).or_default().push(r);
        }

        let mut hash_list = Vec::new();

        for (weather_cond, cond_results) in grouped {
            let dir_path = format!("{}/{}/{}", base_dir, date_str, weather_cond);
            std::fs::create_dir_all(&dir_path).expect("Failed to create deeply nested data folders");

            let trajectory_records: Vec<_> = cond_results.iter().map(|r| {
                TrajectoryRecord {
                    id: format!("autonomous_car_{}_{}", r.short_id, weather_cond),
                    traj_type: "vehicle_kinematics".to_string(),
                    scenario: format!("{}_cruising", weather_cond),
                    steps: r.steps,
                    score: serde_json::json!({
                        "success": r.mission_success,
                    }),
                    proof_hash: r.proof_hash.clone(),
                    reasoning_context: serde_json::json!({
                        "anomaly": !r.mission_success,
                        "outcome": r.outcome,
                        "weather": r.weather,
                    }),
                    data: r.telemetry.clone(),
                }
            }).collect();

            let proof_hashes: Vec<String> = cond_results.iter().map(|r| r.proof_hash.clone()).collect();
            let run_proof = proof::seal_run(&proof_hashes);
            hash_list.push(run_proof);
            
            let dataset = Dataset {
                dataset_metadata: DatasetMetadata {
                    generator: "G^G Autonomous Car Monte Carlo".to_string(),
                    domain: "terran_wheeled".to_string(),
                    scenario: "autonomous_driving".to_string(),
                    trajectories: cond_results.len(),
                    physics_engine: "genesis_core::terran + friction model".to_string(),
                    version: "1.0.0".to_string(),
                    generated_at: output::now_iso(),
                },
                trajectories: trajectory_records,
            };
            
            let chunk_id = output::short_id(&mut rng);
            let file_path = format!("{}/dataset_{}_{}.json", dir_path, weather_cond, chunk_id);

            output::write_dataset(&file_path, &dataset).expect("Failed to write dataset");
        }
        let master_proof = proof::seal_run(&hash_list);
        if !json_output {
            println!("  SHA-256 Run Proof: {}", master_proof);
        }
    }
}
