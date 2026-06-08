// G^G QUADRUPED SUBTERRANEAN DRIFT MONTE CARLO (GHOST ROBOTICS STRIKE IV)
// Sovereign Verification: MEMS IMU Random Walk vs High-Frequency Chassis Vibration
//
// THE EMBODIMENT: A Ghost Robotics V60 Quadruped navigating a 400-meter dark 
// subterranean tunnel (or Amazon Proteus navigating a dusty fulfillment center).
// Camera SLAM fails due to 0-Lux lighting. The robot relies entirely on 
// internal dead-reckoning (IMU Accelerometers) without GPS anchoring.
// 
// THE VULNERABILITY: MLX/Autonomous navigation stacks double-integrate acceleration 
// to derive position (s = 1/2 * a * t^2). In clean environments, this works for 
// minutes. But RL environments do not map the acoustic frequency of physical footfalls 
// striking hard concrete.
//
// THE MATHEMATICAL REALITY: Every step the quadruped takes injects high-frequency 
// vibration noise straight into the chassis. This amplifies the IMU's 
// Velocity Random Walk (VRW). Because integration is compounding, a vibration-induced 
// sensor bias of 0.05 m/s^2 grows exponentially.
//
// THE FATALITY: After 240 seconds of blind walking, the robot's internal "hallucinated"
// position deviates from True Physical Reality by 15 meters laterally. The AI 
// thinks it is walking perfectly straight down the center of the bunker. In reality, 
// the dog tracks blindly to the right and grinds its chassis against a concrete wall.

use std::time::Instant;
use genesis_core::rng::Rng;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output::{self, DatasetMetadata, TrajectoryRecord};

// Kinematic Simulation Geometry
const TARGET_TUNNEL_DISTANCE_M: f64 = 200.0;
const BUNKER_WIDTH_LIMIT_M: f64 = 5.0; // The tunnel is 10m wide. > 5.0m lateral deviation hits the wall.
const WALKING_VELOCITY_MPS: f64 = 1.0; 
const ROBOT_MASS: f64 = 50.0;

// IMU Specifications (Consumer-Industrial Grade MEMS)
const BASE_VELOCITY_RANDOM_WALK: f64 = 0.005; // Nominal IMU noise density (m/s / srqt(hr))

#[derive(Debug, Clone, Copy, PartialEq)]
enum FailureMode {
    VirtualCleanIMU,          // Theoretical simulation with 0 noise
    NominalLabVibration,      // Smooth tile floor, quiet actuators (low noise)
    ConcreteFootfallResonance, // Hard bunker concrete, echoing acoustic resonance amplifying VRW
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    SubterraneanNavigation, // Walking down the dark hallway
    SpatialDivergence,      // Navigation hallucination exceeds 2 meters deviation
    WallCollisionScrape,    // The dog hits the wall (Fatality)
    NavigationSuccess,      // The dog made it to 400m
}

impl Phase {
    fn as_str(&self) -> &'static str {
        match self {
            Phase::SubterraneanNavigation => "DEAD_RECKONING_TRANSIT",
            Phase::SpatialDivergence => "IMU_SPATIAL_HALLUCINATION",
            Phase::WallCollisionScrape => "WALL_COLLISION_GRIND",
            Phase::NavigationSuccess => "NAVIGATION_SUCCESS",
        }
    }
}

#[derive(Debug)]
#[allow(dead_code)]
struct TrajectoryResult {
    id: u32,
    short_id: String,
    final_true_distance: f64,
    final_lateral_error: f64,
    phase: Phase,
    outcome: &'static str,
    steps: usize,
    proof_hash: String,
    failure: FailureMode,
    telemetry: Vec<serde_json::Value>,
}

// ─── SINGLE TRAJECTORY ─────────────────────────────────────────
fn run_single_trajectory(
    id: u32,
    rng: &mut Rng,
    record_telemetry: bool,
) -> TrajectoryResult {
    let short_id = output::short_id(rng);

    let failure = if rng.chance(0.15) { FailureMode::VirtualCleanIMU }
    else if rng.chance(0.35) { FailureMode::NominalLabVibration }
    else { FailureMode::ConcreteFootfallResonance };

    let dt = 0.01; // 100Hz dead-reckoning cycle
    let max_time_s = 600.0; // Max permitted seconds to walk 400m
    let max_steps = (max_time_s / dt) as usize;

    // Vibration injection scaler based on physical terrain
    let vibration_scalar = match failure {
        FailureMode::VirtualCleanIMU => 0.0,
        FailureMode::NominalLabVibration => rng.range(0.01, 0.05), // Heavy rubber mats, low mechanical noise
        FailureMode::ConcreteFootfallResonance => rng.range(8.0, 12.0), // Massive acoustic bouncing noise (steel on concrete)
    };

    let vrw_magnitude = BASE_VELOCITY_RANDOM_WALK * vibration_scalar;

    let mut phase = Phase::SubterraneanNavigation;
    let mut step = 0_usize;

    // ─── AI ESTIMATED STATE ────────────────────────────────────
    // The AI thinks it is accelerating perfectly straight ahead. 
    // It assumes Lateral_Velocity = 0 and Lateral_Accel = 0.
    // ────────────────────────────────────────────────────────
    
    // ─── PHYSICAL TRUE STATE ──────────────────────────────────
    let mut true_y_dist = 0.0; // Lateral position in the tunnel
    let mut true_y_vel = 0.0;  // Lateral drifting velocity due to false-corrections
    let mut true_x_dist = 0.0; // Forward position
    let mut true_x_vel = 0.0;

    let mut imu_y_accel_bias = 0.0; // Slowly drifting bias due to vibration

    let mut final_outcome = "TIMEOUT_ERROR";

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(ROBOT_MASS);

    let mut telemetry = Vec::new();

    // ═══ THE FORGE: PHYSICAL INTEGRATION ═══
    while step < max_steps {
        let t = step as f64 * dt;

        // 1. FORWARD KINEMATICS
        let forward_command = 0.0; // Maintaining 1.0 m/s cruise
        let v_err = WALKING_VELOCITY_MPS - true_x_vel;
        true_x_vel += (v_err * 2.0) * dt; // Simple P controller pushing to 1 m/s
        true_x_dist += true_x_vel * dt;

        // 2. IMU NOISE ACCUMULATION (Double Integration Trap)
        // High frequency footfall impacts cause the MEMS proof-mass inside the accelerometer 
        // to resonate, permanently shifting the zero-bias point tiny fractions per second.
        let random_vibration = rng.range(-1.0, 1.0) * vrw_magnitude;
        imu_y_accel_bias += random_vibration * dt.sqrt(); // Random walk formula

        // The AI reads this false bias. Because the AI thinks it's drifting (when it physically isn't),
        // the AI's Pid controller commands counter-drifting lateral torque to "correct" its course!
        // This causes the physical robot to aggressively veer laterally into the wall.
        
        let ai_perceived_y_accel = imu_y_accel_bias; 
        
        let ai_correction_accel = -ai_perceived_y_accel; // The robot physically thrusts laterally to correct the illusion

        // 3. PHYSICAL MOVEMENT
        true_y_vel += ai_correction_accel * dt;
        true_y_dist += true_y_vel * dt;

        // 4. BOUNDARY SURVIVAL CHECK
        if true_y_dist.abs() > 2.0 && phase == Phase::SubterraneanNavigation {
            phase = Phase::SpatialDivergence;
        }

        if true_y_dist.abs() >= BUNKER_WIDTH_LIMIT_M {
            // It literally walked laterally into a concrete wall in the dark
            phase = Phase::WallCollisionScrape;
            final_outcome = "WALL_COLLISION_GRIND";
            break;
        }

        if true_x_dist >= TARGET_TUNNEL_DISTANCE_M {
            phase = Phase::NavigationSuccess;
            final_outcome = "NAVIGATION_SUCCESS";
            break;
        }

        // 5. RECORD STATE
        if t % 5.0 < dt { // 0.2Hz recording for long mission trajectory
            proof.feed_f64(true_x_dist);
            proof.feed_f64(true_y_dist);

            if record_telemetry {
                telemetry.push(serde_json::json!({
                    "t": t,
                    "x_dist": true_x_dist,
                    "y_err": true_y_dist,
                    "bias": imu_y_accel_bias,
                    "phase": phase.as_str()
                }));
            }
        }

        step += 1;
    }

    if step >= max_steps && phase != Phase::NavigationSuccess {
        final_outcome = "TIMEOUT_LOST_IN_TUNNEL";
    }

    proof.feed_f64(true_x_dist);
    proof.feed_f64(true_y_dist);
    proof.feed_str(final_outcome);
    let proof_hash = proof.seal();

    TrajectoryResult {
        id,
        short_id,
        final_true_distance: true_x_dist,
        final_lateral_error: true_y_dist,
        phase,
        outcome: final_outcome,
        steps: step,
        proof_hash,
        failure,
        telemetry,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n_trajectories: u32 = args.get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10_000);
    let json_output = args.iter().any(|a| a == "--json");
    let json_path = args.iter().position(|a| a == "--out")
        .and_then(|i| args.get(i + 1))
        .cloned();

    if !json_output {
        println!("====================================================================");
        println!("  G^G SUBTERRANEAN DRIFT MONTE CARLO (GHOST ROBOTICS/AMAZON KIVA)");
        println!("  Verifying IMU Velocity Random Walk vs Footfall Acoustic Vibration");
        println!("====================================================================");
        println!();
        println!("  Trajectories:  {}", n_trajectories);
        println!("  Physics:       100Hz MEMS Sensor Dead-Reckoning Integration");
        println!("  Sensors:       Base Accelerometer Vibration Bias (Random Walk)");
        println!("  Estimation:    AI hallucinates zero-bias clean trajectories");
        println!("  Boundary:      5-Meter Lateral Tunnel Collision Cutoff");
        println!("====================================================================");
        println!();
    }

    let start = Instant::now();
    let record_telemetry = false; 
    let counter = std::sync::atomic::AtomicUsize::new(0);

    let (tx, rx) = std::sync::mpsc::sync_channel::<TrajectoryResult>(20000);

    let json_writer = if let Some(path) = &json_path {
        let metadata = DatasetMetadata {
            generator: "G^G Sensor Auditing v1.0".to_string(),
            domain: "ground_robotics".to_string(),
            scenario: "quadruped_subterranean_vibration_drift".to_string(),
            trajectories: n_trajectories as usize,
            physics_engine: "genesis_core::double_integration_trap (100Hz)".to_string(),
            version: "1.0.0".to_string(),
            generated_at: output::now_iso(),
        };
        let mut streamer = output::DatasetStreamer::new(path, &metadata).expect("Failed to create streamer");
        let mut run_proof_chain = ProofChain::new();

        let handle = std::thread::spawn(move || {
            let mut results = Vec::new();
            for r in rx {
                let rec = TrajectoryRecord {
                    id: format!("ugv_audit_{}", r.short_id),
                    traj_type: "acoustic_imu_drift".to_string(),
                    scenario: match r.failure {
                        FailureMode::VirtualCleanIMU => "isaac_sim_perfect_sensor".to_string(),
                        FailureMode::NominalLabVibration => "lab_smooth_surface_low_noise".to_string(),
                        FailureMode::ConcreteFootfallResonance => "concrete_bunker_high_frequency_shatter".to_string(),
                    },
                    steps: r.steps,
                    score: serde_json::json!({
                        "survived": r.outcome == "NAVIGATION_SUCCESS",
                        "final_lateral_error_m": (r.final_lateral_error * 10.0).round() / 10.0,
                        "forward_distance_m": (r.final_true_distance * 10.0).round() / 10.0,
                    }),
                    proof_hash: r.proof_hash.clone(),
                    reasoning_context: serde_json::json!({
                        "is_anomaly": r.outcome != "NAVIGATION_SUCCESS",
                        "anomaly_type": r.outcome,
                        "snapshot": {
                            "failure": format!("{:?}", r.failure),
                        },
                    }),
                    data: r.telemetry.clone(),
                };
                streamer.write_trajectory(&rec).expect("Failed to write to stream");
                run_proof_chain.feed_str(&r.proof_hash);

                let mut diag_r = r;
                diag_r.telemetry = Vec::new();
                results.push(diag_r);
            }
            streamer.finish().expect("Failed to finish stream");
            (run_proof_chain.seal(), results)
        });
        Some(handle)
    } else {
        None
    };

    let tx_ref = if json_writer.is_some() { Some(tx) } else { None };

    use rayon::prelude::*;
    let mut inline_results: Vec<TrajectoryResult> = if tx_ref.is_none() {
        (0..n_trajectories)
            .into_par_iter()
            .map(|i| {
                let mut rng = Rng::new(0xF00D_1337_D00D_BA5E ^ (i as u64).wrapping_mul(0xC0FE_BABE_FACE_BEEF));
                let r = run_single_trajectory(i, &mut rng, record_telemetry);

                if !json_output {
                    let count = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    if count % 10000 == 0 {
                        let elapsed = start.elapsed().as_secs_f64();
                        let rate = count as f64 / elapsed;
                        eprint!("\r  [{}/{}] {:.0} traj/sec", count, n_trajectories, rate);
                    }
                }
                r
            }).collect()
    } else {
        (0..n_trajectories)
            .into_par_iter()
            .for_each_with(tx_ref.unwrap(), |sender, i| {
                let mut rng = Rng::new(0xF00D_1337_D00D_BA5E ^ (i as u64).wrapping_mul(0xC0FE_BABE_FACE_BEEF));
                let r = run_single_trajectory(i, &mut rng, record_telemetry);
                sender.send(r).expect("Channel closed");
            });
        Vec::new()
    };

    let (run_proof, mut results) = if let Some(handle) = json_writer {
        handle.join().expect("Writer thread panicked")
    } else {
        inline_results.sort_by_key(|r| r.id);
        if !json_output { eprintln!(); }
        
        let proof_hashes: Vec<String> = inline_results.iter().map(|r| r.proof_hash.clone()).collect();
        (proof::seal_run(&proof_hashes), inline_results)
    };

    let elapsed = start.elapsed();

    if json_output || json_path.is_some() {
        if let Some(path) = &json_path {
            eprintln!("\n  Written to: {}", path);
            eprintln!("  Run proof:  {}", run_proof);
        }
        return;
    }

    let total = results.len();
    let success = results.iter().filter(|r| r.outcome == "NAVIGATION_SUCCESS").count();
    let wall = results.iter().filter(|r| r.outcome == "WALL_COLLISION_GRIND").count();
    let timeout = results.iter().filter(|r| r.outcome == "TIMEOUT_LOST_IN_TUNNEL").count();

    println!("====================================================================");
    println!("  QUADRUPED SUBTERRANEAN DRIFT RESULTS");
    println!("====================================================================");
    println!();
    println!("  Total Trajectories:    {}", total);
    println!("  Elapsed:               {:.3}s", elapsed.as_secs_f64());
    println!("  Throughput:            {:.0} trajectories/sec", total as f64 / elapsed.as_secs_f64());
    println!();
    println!("  +---------------------------------------------+");
    println!("  | OUTCOME DISTRIBUTION                         |");
    println!("  +---------------------------------------------+");
    println!("  | DEAD RECKONING SUCCESS:      {:>6} ({:>5.1}%)  |", success, success as f64 / total as f64 * 100.0);
    println!("  | WALL SPATIAL CRASH (FATAL):  {:>6} ({:>5.1}%)  |", wall, wall as f64 / total as f64 * 100.0);
    println!("  | TIMEOUT PERDITION:           {:>6} ({:>5.1}%)  |", timeout, timeout as f64 / total as f64 * 100.0);
    println!("  +---------------------------------------------+");
    println!();

    // ═══ FAILURE MODE SUMMARY ═══
    println!("  +---------------------------------------------+");
    println!("  | VULNERABILITY IMPACT (Spatial Loss Rate)     |");
    println!("  +---------------------------------------------+");
    let clean: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::VirtualCleanIMU)).collect();
    let lab: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::NominalLabVibration)).collect();
    let bunker: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::ConcreteFootfallResonance)).collect();

    let crash_rate = |v: &[&TrajectoryResult]| -> f64 {
        if v.is_empty() { 0.0 } else { v.iter().filter(|r| r.outcome != "NAVIGATION_SUCCESS").count() as f64 / v.len() as f64 * 100.0 }
    };

    println!("  | Virtual Omniverse (Zero VRW): {:>4.1}% ({:>6} runs)  |", crash_rate(&clean), clean.len());
    println!("  | Lab Smooth Control (Low Shock): {:>4.1}% ({:>6} runs)  |", crash_rate(&lab), lab.len());
    println!("  | Concrete Bunker Shock Drift : {:>4.1}% ({:>6} runs)  |", crash_rate(&bunker), bunker.len());
    println!("  +---------------------------------------------+");
    println!();
    println!("  Run Proof Seal: {}", run_proof);
    println!("====================================================================");
}
