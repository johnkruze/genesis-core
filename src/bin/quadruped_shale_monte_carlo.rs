// G^G QUADRUPED SHALE MONTE CARLO (GHOST ROBOTICS STRIKE)
// Sovereign Verification: Inverse Kinematics Hallucination vs Kinetic Friction
//
// THE EMBODIMENT: A 50kg (110lb) Quadruped Unmanned Ground Vehicle (UGV) ascending
// a 25-degree incline. The terrain is loose shale/scree (fracturing rock) or black ice.
// 
// THE VULNERABILITY: UGVs are trained using Reinforcement Learning (RL) in software
// like NVIDIA Isaac Sim. The physics engines rely on generic "Mujoco" style friction
// models, which assume rigid ground (Static Friction Coefficient ~0.8). 
//
// THE MATHEMATICAL REALITY: When a 50kg dog shifts its weight to ONE planted hind 
// leg on a 25-degree slope, the normal force exceeds the shear strength of the shale.
// The static friction turns into sliding kinetic friction virtually instantly (from 
// mu=0.5 down to mu=0.1). 
//
// THE FATALITY: The robot's AI *assumes* the foot is locked in place and commands 
// 100% torque to the knee actuator to pull its body mass up the hill. Because the 
// foot is sliding freely backwards, there is ZERO mechanical resistance. The actuator
// spins uncontrollably, instantly hitting the servo's mechanical hard-stop. The leg 
// snaps backwards, shattering the gearbox in under 400 milliseconds.

use std::time::Instant;
use genesis_core::rng::Rng;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output::{self, DatasetMetadata, TrajectoryRecord};

const GRAVITY: f64 = 9.81;
const INCLINE_ANGLE_DEG: f64 = 25.0;

// Mechanical Hard-stop of the actuator (Radians). If the joint spins past this, it shatters.
const KNEE_JOINT_MAX_EXT_RAD: f64 = 2.8; 
const ACTUATOR_MAX_RAD_SEC: f64 = 14.0; // Fast dynamic servo speed
const ACTUATOR_MAX_TORQUE_NM: f64 = 110.0;

#[derive(Debug, Clone, Copy, PartialEq)]
enum FailureMode {
    Nominal,
    IsaacSimHallucination, // Assumes concrete friction (mu=0.9)
    FracturingShale,       // Surface shear causes immediate kinetic slip
    IcePatch,              // mu drops to 0.05 instantly mid-step
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    WeightTransfer,      // Shifting mass onto the planted foot
    TorqueExecution,     // Asserting torque to climb the hill
    ActuatorDestruction, // The joint overspun and shattered
    AscentSuccess,       // It made the step without slipping 
}

impl Phase {
    fn as_str(&self) -> &'static str {
        match self {
            Phase::WeightTransfer => "WEIGHT_TRANSFER",
            Phase::TorqueExecution => "TORQUE_EXECUTION",
            Phase::ActuatorDestruction => "JOINT_SHATTERED_FATAL",
            Phase::AscentSuccess => "STEP_SUCCESS",
        }
    }
}

#[derive(Debug)]
#[allow(dead_code)]
struct TrajectoryResult {
    id: u32,
    short_id: String,
    final_knee_angle_rad: f64,
    final_slip_distance_m: f64,
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

    let mass_kg = 50.0;
    
    // Failure injection
    let failure = if rng.chance(0.08) { FailureMode::IcePatch }
    else if rng.chance(0.06) { FailureMode::IsaacSimHallucination }
    else if rng.chance(0.06) { FailureMode::FracturingShale }
    else { FailureMode::Nominal };

    // Physical True Friction Parameters
    let mut true_static_mu = rng.range(0.65, 0.85); // Nominal dirt/rock
    let mut true_kinetic_mu = rng.range(0.4, 0.5); 

    if failure == FailureMode::FracturingShale {
        true_static_mu = rng.range(0.25, 0.40); // Shale gives way easily
        true_kinetic_mu = 0.1; // Scree sliding
    } else if failure == FailureMode::IcePatch {
        true_static_mu = 0.15;
        true_kinetic_mu = 0.05; // Absolute frictionless slip
    }

    // AI Hallucination parameters (What the Inverse Kinematics solver *thinks* is happening)
    let ai_assumed_mu = if failure == FailureMode::IsaacSimHallucination {
        0.9 // NVIDIA omniverse concrete
    } else {
        0.5 // Standard tactical estimate
    };

    let dt = 0.001; // 1000Hz Euler Integration
    let max_time_s = 2.0; // Single step cycle
    let max_steps = (max_time_s / dt) as usize;

    let mut phase = Phase::WeightTransfer;
    let mut step = 0_usize;

    // Kinematics State
    let mut true_foot_slip_x = 0.0; // How far the foot has dragged backwards down the hill
    let mut true_foot_slip_vel = 0.0;
    let mut true_knee_angle = 1.0; // Radians (Bent knee)
    let mut true_knee_vel = 0.0;

    let mut final_outcome = "TIMEOUT_ERROR";

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(mass_kg);

    let mut telemetry = Vec::new();

    let inc_rad = INCLINE_ANGLE_DEG.to_radians();
    let weight_normal_force = mass_kg * GRAVITY * inc_rad.cos();
    let weight_downhill_force = mass_kg * GRAVITY * inc_rad.sin();

    // ═══ THE FORGE: PHYSICAL INTEGRATION ═══
    while step < max_steps {
        let t = step as f64 * dt;

        // 1. DYNAMIC WEIGHT TRANSFER
        // The robot shifts weight from 4 legs to 2 legs during the gait cycle.
        // At t=0.5, nearly 80% of the dog's weight is dynamically pushed onto the single rear ground foot.
        let dynamic_load_factor = if t < 0.5 {
            0.25 + (t / 0.5) * 0.55 // Ramping from 25% of weight to 80%
        } else {
            0.8
        };
        
        let active_normal_force = weight_normal_force * dynamic_load_factor;
        let active_downhill_force = weight_downhill_force * dynamic_load_factor;

        if t > 0.5 && phase == Phase::WeightTransfer {
            phase = Phase::TorqueExecution;
        }

        // 2. THE RL INVERSE KINEMATICS HALLUCINATION (Isaac Sim)
        // The dog attempts to exert torque against the ground to pull its body up the hill.
        // It calculates required torque assuming the foot won't slip (T = F * r).
        let required_hill_force = active_downhill_force + 25.0; // Overcome gravity + ascend
        let hallucinated_max_grip = active_normal_force * ai_assumed_mu; 
        
        // If the AI thinks grip is > required force, it applies it instantly.
        let commanded_torque = if required_hill_force <= hallucinated_max_grip {
            required_hill_force // Lever arm simplified to 1.0 for relative torque
        } else {
            // It thinks it will slip, so it limits torque to prevent slip, or maxes out
            hallucinated_max_grip.min(ACTUATOR_MAX_TORQUE_NM)
        };

        // 3. TRUE TERRAIN PHYSICS (The Forge)
        let true_static_grip = active_normal_force * true_static_mu;
        
        // The motor asserts the commanded torque laterally against the terrain
        let total_shear_force = commanded_torque;

        let mut foot_acceleration = 0.0;

        if true_foot_slip_vel > 0.05 || total_shear_force > true_static_grip {
            // SLIPPING. The shale broke.
            let sliding_friction_force = active_normal_force * true_kinetic_mu;
            
            // The foot accelerates rapidly backwards down the hill because torque is still being applied
            let net_slip_force = total_shear_force - sliding_friction_force;
            foot_acceleration = net_slip_force / (mass_kg * 0.2); // Equivalent mass of the slipping limb
            
            true_foot_slip_vel += foot_acceleration * dt;
            true_foot_slip_x += true_foot_slip_vel * dt;
        } else {
            // Grip holds
            true_foot_slip_vel = 0.0;
        }

        // 4. ACTUATOR DESTRUCTION MATH
        // When the foot slips, the knee joint receives NO external resistance. 
        // The commanded torque completely over-spins the free-floating servo.
        if true_foot_slip_vel > 0.1 {
            // Free-spin acceleration of the unloaded servo
            let joint_inertia = 0.15; // kg*m^2
            let angular_accel = commanded_torque / joint_inertia;
            
            true_knee_vel += angular_accel * dt;
            if true_knee_vel > ACTUATOR_MAX_RAD_SEC {
                true_knee_vel = ACTUATOR_MAX_RAD_SEC; // Motor electrical RPM limit
            }
            
            true_knee_angle += true_knee_vel * dt;
        } else {
            // Normal controlled step extension
            true_knee_angle += 0.5 * dt; // Slow controlled extension
        }

        // 5. BOUNDARY SURVIVAL CHECK (Mechanical Hard-stop Impact)
        if true_knee_angle >= KNEE_JOINT_MAX_EXT_RAD {
            // The joint spun past its structural limit
            phase = Phase::ActuatorDestruction;
            final_outcome = "GEARBOX_SHATTERED_EXTREME_VELOCITY";
            break;
        } else if t >= 1.5 {
            phase = Phase::AscentSuccess;
            final_outcome = "STEP_SUCCESS";
            break;
        }

        // 6. RECORD STATE
        if step % 200 == 0 { // 5Hz recording
            proof.feed_f64(true_foot_slip_x);
            proof.feed_f64(true_knee_angle);

            if record_telemetry {
                telemetry.push(serde_json::json!({
                    "t": t,
                    "slip_m": true_foot_slip_x,
                    "knee_rad": true_knee_angle,
                    "slip_vel": true_foot_slip_vel,
                    "torque": commanded_torque,
                    "phase": phase.as_str()
                }));
            }
        }

        step += 1;
    }

    proof.feed_f64(true_foot_slip_x);
    proof.feed_f64(true_knee_angle);
    proof.feed_str(final_outcome);
    let proof_hash = proof.seal();

    TrajectoryResult {
        id,
        short_id,
        final_knee_angle_rad: true_knee_angle,
        final_slip_distance_m: true_foot_slip_x,
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
        println!("  G^G QUADRUPED SHALE MONTE CARLO (GHOST ROBOTICS)");
        println!("  Verifying Terrain Kinematics Hallucination against Live Actuators");
        println!("====================================================================");
        println!();
        println!("  Trajectories:  {}", n_trajectories);
        println!("  Physics:       1000Hz Friction State Translation (Static -> Kinetic)");
        println!("  Sensors:       UGV Inverse Kinematics Torque Allocation");
        println!("  Estimation:    Generative RL training assumes rigid concrete traction");
        println!("  Boundary:      Mechanical Actuator Hard-Stop Shatter vs Yield Limits");
        println!("====================================================================");
        println!();
    }

    let start = Instant::now();
    let record_telemetry = false; 
    let counter = std::sync::atomic::AtomicUsize::new(0);

    let (tx, rx) = std::sync::mpsc::sync_channel::<TrajectoryResult>(20000);

    let json_writer = if let Some(path) = &json_path {
        let metadata = DatasetMetadata {
            generator: "G^G Terrain Auditing v1.0".to_string(),
            domain: "ground_robotics".to_string(),
            scenario: "quadruped_shale_ascent".to_string(),
            trajectories: n_trajectories as usize,
            physics_engine: "genesis_core::quadruped_terrain (1000Hz Euler)".to_string(),
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
                    traj_type: "scree_incline_25deg".to_string(),
                    scenario: match r.failure {
                        FailureMode::Nominal => "nominal_soil".to_string(),
                        FailureMode::IsaacSimHallucination => "isaac_sim_concrete_hallucination".to_string(),
                        FailureMode::FracturingShale => "fracturing_shale_scree".to_string(),
                        FailureMode::IcePatch => "black_ice_lateral_slip".to_string(),
                    },
                    steps: r.steps,
                    score: serde_json::json!({
                        "survived": r.outcome == "STEP_SUCCESS",
                        "final_slip_m": (r.final_slip_distance_m * 1000.0).round() / 1000.0,
                        "knee_angle_rad": (r.final_knee_angle_rad * 1000.0).round() / 1000.0,
                    }),
                    proof_hash: r.proof_hash.clone(),
                    reasoning_context: serde_json::json!({
                        "is_anomaly": r.outcome != "STEP_SUCCESS",
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
                let mut rng = Rng::new(0xDEAD_BEEF_C0DE_CAFE ^ (i as u64).wrapping_mul(0x712B_A3E4_F09D_18A2));
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
                let mut rng = Rng::new(0xDEAD_BEEF_C0DE_CAFE ^ (i as u64).wrapping_mul(0x712B_A3E4_F09D_18A2));
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
    let success = results.iter().filter(|r| r.outcome == "STEP_SUCCESS").count();
    let shattered = results.iter().filter(|r| r.outcome == "GEARBOX_SHATTERED_EXTREME_VELOCITY").count();

    println!("====================================================================");
    println!("  QUADRUPED KINEMATICS RESULTS");
    println!("====================================================================");
    println!();
    println!("  Total Trajectories:    {}", total);
    println!("  Elapsed:               {:.3}s", elapsed.as_secs_f64());
    println!("  Throughput:            {:.0} trajectories/sec", total as f64 / elapsed.as_secs_f64());
    println!();
    println!("  +---------------------------------------------+");
    println!("  | OUTCOME DISTRIBUTION                         |");
    println!("  +---------------------------------------------+");
    println!("  | KINEMATIC STEP SUCCESS:      {:>6} ({:>5.1}%)  |", success, success as f64 / total as f64 * 100.0);
    println!("  | GEARBOX SHATTERED (FATAL):   {:>6} ({:>5.1}%)  |", shattered, shattered as f64 / total as f64 * 100.0);
    println!("  +---------------------------------------------+");
    println!();

    // ═══ FAILURE MODE SUMMARY ═══
    println!("  +---------------------------------------------+");
    println!("  | VULNERABILITY IMPACT (Mechanical Loss Rate)  |");
    println!("  +---------------------------------------------+");
    let nominal: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::Nominal)).collect();
    let sim_fail: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::IsaacSimHallucination)).collect();
    let shale_fail: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::FracturingShale)).collect();
    let ice_fail: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::IcePatch)).collect();

    let shatter_rate = |v: &[&TrajectoryResult]| -> f64 {
        if v.is_empty() { 0.0 } else { v.iter().filter(|r| r.outcome != "STEP_SUCCESS").count() as f64 / v.len() as f64 * 100.0 }
    };

    println!("  | Nominal Terrain (Dirt): {:>5.1}% ({:>6} runs)   |", shatter_rate(&nominal), nominal.len());
    println!("  | Omniverse Hallucination : {:>5.1}% ({:>6} runs)   |", shatter_rate(&sim_fail), sim_fail.len());
    println!("  | Loose Shale Fracture:  {:>5.1}% ({:>6} runs)   |", shatter_rate(&shale_fail), shale_fail.len());
    println!("  | Structural Black Ice:   {:>5.1}% ({:>6} runs)   |", shatter_rate(&ice_fail), ice_fail.len());
    println!("  +---------------------------------------------+");
    println!();
    println!("  Run Proof Seal: {}", run_proof);
    println!("====================================================================");
}
