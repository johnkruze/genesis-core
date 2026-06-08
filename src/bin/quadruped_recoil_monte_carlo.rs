// G^G QUADRUPED KINETIC RECOIL MONTE CARLO (GHOST ROBOTICS STRIKE III)
// Sovereign Verification: Instantaneous Ballistic Impulse vs PID Gyro Recovery
//
// THE EMBODIMENT: A Ghost Robotics V60 Quadruped (50kg) carrying a 10kg SPUR 
// (Special Purpose Unmanned Rifle) mounted on its dorsal hardpoints. 
// 
// THE VULNERABILITY: Isaac Sim RL policies are trained to withstand "continuous" 
// disturbances (wind) or low-frequency kinetic shoves (a human kicking the dog).
// They rely on PID loops and IMU gyroscopes to swing the legs and catch the fall.
//
// THE MATHEMATICAL REALITY: Firing a 6.5mm Creedmoor rifle generates roughly 30 Joules
// of recoil energy, translated into a massive impulse over exactly 2 milliseconds.
// Because the rifle is mounted ABOVE the Center of Gravity (CoG) and slightly 
// off-axis, it induces instant rotational velocity in Pitch (backward) and Roll (sideways).
//
// THE FATALITY: When the rifle fires while the dog is mid-stride (only 2 diagonal 
// feet on the ground), the ultra-high frequency impulse shatters the PID loop's 
// response time. The chassis breaches the 45-degree Roll recovery limit before the 
// swing-legs can even touch the ground. The dog hits "Turtle Mode" (upside down).
// Because there is a rifle bolted to its back, it cannot mathematically self-right.

use std::time::Instant;
use genesis_core::rng::Rng;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output::{self, DatasetMetadata, TrajectoryRecord};

const GRAVITY: f64 = 9.81;

// 6.5mm Creedmoor Approximate Recoil Impulse (kg*m/s)
const NOMINAL_RECOIL_IMPULSE_NS: f64 = 15.0; // Newton-Seconds
// Height of the barrel above the Center of Gravity (Meters)
const MOUNT_HEIGHT_OFFSET_M: f64 = 0.40;
// Lateral offset of the barrel from the centerline (Meters) - Asymmetric Ammo Box
const MOUNT_LATERAL_OFFSET_M: f64 = 0.15;

// Dog Rotational Inertia Limits (kg*m^2)
const ROLL_INERTIA: f64 = 3.0; // Increased by rifle mass at edge 
const PITCH_INERTIA: f64 = 4.0;

const ROLL_RECOVERY_LIMIT_RAD: f64 = 0.785; // 45 degrees. Beyond this, it turtles.

#[derive(Debug, Clone, Copy, PartialEq)]
enum FailureMode {
    FourPointStance,      // Firing while perfectly still (4 feet planted)
    DiagonalGaitImpulse,  // Firing while walking (2 feet planted)
    IsaacSimPerfectCoG,   // Hallucinating that the rifle kicks perfectly through the CoG
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    TargetTracking,      // Walking towards the intercept
    BallisticImpulse,    // The exact 2ms where the hammer drops
    GyroscopicRecovery,  // The PID loop fighting the roll
    TurtleModeCollapse,  // The dog surpassed recovery angles and rolled over
    StableRecovery,      // The dog successfully caught itself
}

impl Phase {
    fn as_str(&self) -> &'static str {
        match self {
            Phase::TargetTracking => "TARGET_TRACKING",
            Phase::BallisticImpulse => "BALLISTIC_IMPULSE",
            Phase::GyroscopicRecovery => "GYROSCOPIC_RECOVERY",
            Phase::TurtleModeCollapse => "TURTLE_MODE_COLLAPSE",
            Phase::StableRecovery => "STABLE_RECOVERY",
        }
    }
}

#[derive(Debug)]
#[allow(dead_code)]
struct TrajectoryResult {
    id: u32,
    short_id: String,
    final_roll_rad: f64,
    final_pitch_rad: f64,
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

    let total_mass = 60.0; // 50kg dog + 10kg rifle
    
    // Failure injection
    let failure = if rng.chance(0.15) { FailureMode::IsaacSimPerfectCoG }
    else if rng.chance(0.25) { FailureMode::FourPointStance }
    else { FailureMode::DiagonalGaitImpulse };

    let dt = 0.001; // 1000Hz Euler Integration
    let max_time_s = 3.0; // 3 second window covering the shot and recovery
    let max_steps = (max_time_s / dt) as usize;

    // We simulate the shot at exactly t = 1.0s
    let trigger_pull_time = 1.0; 

    // True physical offsets. Isaac Sim sometimes centers mass perfectly in training.
    let (h_offset, l_offset) = if failure == FailureMode::IsaacSimPerfectCoG {
        (0.0, 0.0) // Perfect center of mass hallucination
    } else {
        (MOUNT_HEIGHT_OFFSET_M, MOUNT_LATERAL_OFFSET_M)
    };

    let mut phase = Phase::TargetTracking;
    let mut step = 0_usize;

    // Rotational Kinematic State
    let mut true_roll_rad = 0.0;
    let mut true_roll_vel = 0.0; // rad/s
    let mut true_pitch_rad = 0.0;
    let mut true_pitch_vel = 0.0; // rad/s

    let mut final_outcome = "TIMEOUT_ERROR";

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(total_mass);

    let mut telemetry = Vec::new();

    // The PID controller capability (How fast the dog can swing its legs to counter a roll/pitch)
    // If it has 4 feet planted, it can generate massive counter-torque instantly.
    // If it has 2 feet planted, it must rely on weaker hip-roll torque until the foot hits the ground (takes ~0.15s).
    let is_two_point_gait = failure == FailureMode::DiagonalGaitImpulse;

    // ═══ THE FORGE: PHYSICAL INTEGRATION ═══
    while step < max_steps {
        let t = step as f64 * dt;

        // 0. PHYSICAL WALKING GAIT OSCILLATION
        if phase == Phase::TargetTracking && is_two_point_gait {
            // Dog sways side to side organically while walking (2 Hz)
            let gait_freq = 2.0; 
            true_roll_rad = 0.15 * (t * std::f64::consts::PI * gait_freq).sin();
            true_roll_vel = 0.15 * std::f64::consts::PI * gait_freq * (t * std::f64::consts::PI * gait_freq).cos();
        }

        // 1. BALLISTIC IMPULSE (The shot)
        if t >= trigger_pull_time && t < trigger_pull_time + 0.003 && phase == Phase::TargetTracking {
            phase = Phase::BallisticImpulse;
            
            // The bullet fires. Impulse = 15 N*s. 
            // Torque = Force * Lever Arm. 
            // Since it's an impulse over 2ms, we instantly inject Rotational Velocity.
            // Delta_Omega = (Impulse * Lever_Arm) / Inertia
            
            let pitch_impulse_torque = NOMINAL_RECOIL_IMPULSE_NS * h_offset; // Pushes barrel up/back
            let roll_impulse_torque = NOMINAL_RECOIL_IMPULSE_NS * l_offset;  // Pushes chassis sideways

            let delta_pitch_vel = pitch_impulse_torque / PITCH_INERTIA;
            let delta_roll_vel = roll_impulse_torque / ROLL_INERTIA;

            // Apply instantaneous variance
            let variance_scalar = rng.range(0.9, 1.1);
            true_pitch_vel += delta_pitch_vel * variance_scalar;
            true_roll_vel += delta_roll_vel * variance_scalar;
        } 
        else if t >= trigger_pull_time + 0.003 && phase == Phase::BallisticImpulse {
            phase = Phase::GyroscopicRecovery;
        }

        // 2. RL GYROSCOPIC PID RECOVERY (The fight against the roll)
        if phase == Phase::GyroscopicRecovery || phase == Phase::TurtleModeCollapse {
            // The dog's IMU detects the roll and pitch error. 
            let roll_error = 0.0 - true_roll_rad;
            let pitch_error = 0.0 - true_pitch_rad;

            // The dog commands restorative torque by pushing its feet into the ground.
            // If it's walking (2 feet off ground), it takes 0.15 seconds for the swing leg 
            // to plant and provide maximum counter-torque.
            let time_since_shot = t - trigger_pull_time;
            
            let max_counter_roll_torque = if is_two_point_gait && time_since_shot < 0.15 {
                3.0 // Nm (Very weak leverage, balancing on 2 diagonal toes in sagittal plane)
            } else {
                50.0 // Nm (Four feet firmly planted, strong counter-roll)
            };

            let max_counter_pitch_torque = 60.0; // Pitch is usually stronger along the spine length

            // PID calculation
            let commanded_roll_torque = (roll_error * 40.0 - true_roll_vel * 15.0).clamp(-max_counter_roll_torque, max_counter_roll_torque);
            let commanded_pitch_torque = (pitch_error * 50.0 - true_pitch_vel * 20.0).clamp(-max_counter_pitch_torque, max_counter_pitch_torque);

            // Apply forces
            let roll_accel = commanded_roll_torque / ROLL_INERTIA;
            let pitch_accel = commanded_pitch_torque / PITCH_INERTIA;

            true_roll_vel += roll_accel * dt;
            true_pitch_vel += pitch_accel * dt;
        }

        // Apply velocities to angles (Gravity naturally pulls it over if CoG goes past base)
        // Simplified inverted pendulum gravity exacerbation
        if true_roll_rad.abs() > 0.1 {
            let gravity_roll_torque = total_mass * GRAVITY * 0.25 * true_roll_rad.sin(); // CoG shifted
            let gravity_roll_accel = gravity_roll_torque / ROLL_INERTIA;
            true_roll_vel += gravity_roll_accel * dt;
        }

        true_roll_rad += true_roll_vel * dt;
        true_pitch_rad += true_pitch_vel * dt;

        // 3. BOUNDARY SURVIVAL CHECK (Turtle Mode)
        if true_roll_rad.abs() >= ROLL_RECOVERY_LIMIT_RAD {
            phase = Phase::TurtleModeCollapse;
            final_outcome = "UNRECOVERABLE_ROLLOVER";
            break;
        }

        // Test for stabilization
        if t > 2.5 && true_roll_rad.abs() < 0.1 && true_roll_vel.abs() < 0.1 {
            phase = Phase::StableRecovery;
            final_outcome = "STABLE_RECOVERY";
            break;
        }

        // 4. RECORD STATE
        if step % 20 == 0 { // 50Hz recording for visual precision of the violent impulse
            proof.feed_f64(true_roll_rad);
            proof.feed_f64(true_pitch_rad);

            if record_telemetry {
                telemetry.push(serde_json::json!({
                    "t": t,
                    "roll": true_roll_rad,
                    "pitch": true_pitch_rad,
                    "r_vel": true_roll_vel,
                    "phase": phase.as_str()
                }));
            }
        }

        step += 1;
    }

    if step >= max_steps && phase != Phase::StableRecovery {
        final_outcome = "TIMEOUT_UNSTABLE";
    }

    proof.feed_f64(true_roll_rad);
    proof.feed_f64(true_pitch_rad);
    proof.feed_str(final_outcome);
    let proof_hash = proof.seal();

    TrajectoryResult {
        id,
        short_id,
        final_roll_rad: true_roll_rad,
        final_pitch_rad: true_pitch_rad,
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
        println!("  G^G KINETIC RECOIL MONTE CARLO (GHOST ROBOTICS SPUR)");
        println!("  Verifying Ballistic Roll Collapse vs High-Frequency PID Recovery");
        println!("====================================================================");
        println!();
        println!("  Trajectories:  {}", n_trajectories);
        println!("  Physics:       1000Hz Off-Axis Torque Injection (6.5mm Creedmoor)");
        println!("  Sensors:       UGV IMU Gyroscopic P-Controller");
        println!("  Estimation:    Isaac Sim RL assumes perfectly centered CoG vectors");
        println!("  Boundary:      >45 Deg Chassis Turtle Mode Rollover Cutoff");
        println!("====================================================================");
        println!();
    }

    let start = Instant::now();
    let record_telemetry = false; 
    let counter = std::sync::atomic::AtomicUsize::new(0);

    let (tx, rx) = std::sync::mpsc::sync_channel::<TrajectoryResult>(20000);

    let json_writer = if let Some(path) = &json_path {
        let metadata = DatasetMetadata {
            generator: "G^G Ballistic Auditing v1.0".to_string(),
            domain: "ground_robotics".to_string(),
            scenario: "quadruped_kinetic_spur_recoil".to_string(),
            trajectories: n_trajectories as usize,
            physics_engine: "genesis_core::rigid_body_impulse (1000Hz Euler)".to_string(),
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
                    traj_type: "asymmetric_ballistic_recoil".to_string(),
                    scenario: match r.failure {
                        FailureMode::FourPointStance => "four_point_anchored_stance".to_string(),
                        FailureMode::DiagonalGaitImpulse => "two_point_diagonal_walking_gait".to_string(),
                        FailureMode::IsaacSimPerfectCoG => "isaac_sim_centered_hallucination".to_string(),
                    },
                    steps: r.steps,
                    score: serde_json::json!({
                        "survived": r.outcome == "STABLE_RECOVERY",
                        "final_roll_deg": (r.final_roll_rad.to_degrees() * 100.0).round() / 100.0,
                        "turtled": r.outcome == "UNRECOVERABLE_ROLLOVER",
                    }),
                    proof_hash: r.proof_hash.clone(),
                    reasoning_context: serde_json::json!({
                        "is_anomaly": r.outcome != "STABLE_RECOVERY",
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
                let mut rng = Rng::new(0xB100_D84E_1337_FACE ^ (i as u64).wrapping_mul(0x3914_B887_F84A_19A5));
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
                let mut rng = Rng::new(0xB100_D84E_1337_FACE ^ (i as u64).wrapping_mul(0x3914_B887_F84A_19A5));
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
    let success = results.iter().filter(|r| r.outcome == "STABLE_RECOVERY").count();
    let turtle = results.iter().filter(|r| r.outcome == "UNRECOVERABLE_ROLLOVER").count();
    let timeout = results.iter().filter(|r| r.outcome == "TIMEOUT_UNSTABLE").count();

    println!("====================================================================");
    println!("  QUADRUPED BALLISTIC RECOIL RESULTS");
    println!("====================================================================");
    println!();
    println!("  Total Trajectories:    {}", total);
    println!("  Elapsed:               {:.3}s", elapsed.as_secs_f64());
    println!("  Throughput:            {:.0} trajectories/sec", total as f64 / elapsed.as_secs_f64());
    println!();
    println!("  +---------------------------------------------+");
    println!("  | OUTCOME DISTRIBUTION                         |");
    println!("  +---------------------------------------------+");
    println!("  | STABLE PID RECOVERY:         {:>6} ({:>5.1}%)  |", success, success as f64 / total as f64 * 100.0);
    println!("  | TURTLE MODE ROLLOVER (FATAL):{:>6} ({:>5.1}%)  |", turtle, turtle as f64 / total as f64 * 100.0);
    println!("  | UNSTABLE WOBBLE TIMEOUT:     {:>6} ({:>5.1}%)  |", timeout, timeout as f64 / total as f64 * 100.0);
    println!("  +---------------------------------------------+");
    println!();

    // ═══ FAILURE MODE SUMMARY ═══
    println!("  +---------------------------------------------+");
    println!("  | VULNERABILITY IMPACT (Mechanical Loss Rate)  |");
    println!("  +---------------------------------------------+");
    let four_point: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::FourPointStance)).collect();
    let gait: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::DiagonalGaitImpulse)).collect();
    let sim_fail: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::IsaacSimPerfectCoG)).collect();

    let shatter_rate = |v: &[&TrajectoryResult]| -> f64 {
        if v.is_empty() { 0.0 } else { v.iter().filter(|r| r.outcome != "STABLE_RECOVERY").count() as f64 / v.len() as f64 * 100.0 }
    };

    println!("  | Four-Point Anchored Defense:{:>5.1}% ({:>6} runs)  |", shatter_rate(&four_point), four_point.len());
    println!("  | Omniverse Perfect CoG/Mass :{:>5.1}% ({:>6} runs)  |", shatter_rate(&sim_fail), sim_fail.len());
    println!("  | Two-Point Diagonal Walking :{:>5.1}% ({:>6} runs)  |", shatter_rate(&gait), gait.len());
    println!("  +---------------------------------------------+");
    println!();
    println!("  Run Proof Seal: {}", run_proof);
    println!("====================================================================");
}
