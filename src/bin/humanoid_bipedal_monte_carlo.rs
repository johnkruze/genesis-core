// G^G BIPEDAL WHOLE-BODY LIFT COLLAPSE MONTE CARLO
// Sovereign Verification: Inverted Pendulum Slip vs Fluctuating Warehouse Friction
//
// THE EMBODIMENT: An Autonomous 50kg Humanoid
// G1 or Fauna Robotics "Sprout") attempting to lift a 20kg warehouse fulfillment package.
// 
// THE VULNERABILITY: Whole-body imitation policies teach robots to lift objects 
// using whole-body core and leg tension. To leverage the 20kg box upward, the robot 
// squats and pushes its feet forward against the ground. Idealized RL training 
// learning hallucinates that standard concrete floor friction (Mu = 0.7) is a guaranteed 
// physical constant.
//
// THE MATHEMATICAL REALITY: warehouse fulfillment centers accumulate microscopic cardboard 
// dust. A thin layer of dust drops the local static friction coefficient from 0.7 to 0.4.
// When the humanoid executes the whole-body lift, the horizontal shear force generated 
// by the robot's heels instantly breaches the degraded static friction wall.
//
// THE FATALITY: Static friction breaks into sliding kinetic friction (Mu = 0.2). The 
// robot's legs slip forward violently. Because a bipedal robot is a dynamically 
// unstable Inverted Pendulum, losing its base of support while holding 20kg causes an 
// unrecoverable backward collapse. The 70kg combined mass slams onto the concrete, 
// fracturing the robotic spine and rupturing the dorsal lithium battery pack.

use std::time::Instant;
use genesis_core::rng::Rng;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output::{self, DatasetMetadata, TrajectoryRecord};

const GRAVITY: f64 = 9.81;

// Mass Distribution
const HUMANOID_MASS_KG: f64 = 50.0;
const PACKAGE_MASS_KG: f64 = 20.0;
const TOTAL_MASS: f64 = HUMANOID_MASS_KG + PACKAGE_MASS_KG;

// Kinematic Geometry (meters)
const CENTER_OF_GRAVITY_HEIGHT: f64 = 0.85; // Height of CoG when lifting
const FEET_FORWARD_OFFSET: f64 = 0.3; // Distance feet are planted in front of CoG to leverage

#[derive(Debug, Clone, Copy, PartialEq)]
enum FailureMode {
    VirtualCleanSim,        // Idealized lab concrete (Mu = 0.8)
    NominalWarehouse,       // Standard clean warehouse floor (Mu = 0.6)
    MicroDustAccumulation,  // Cardboard dust drops friction (Mu = 0.35)
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    ApproachingPackage,     // Walking to the package
    WholeBodyTensionLift,    // Engaging whole-body core tension to heave the 20kg box
    KineticFrictionSlip,    // The feet break static hold and slip forward
    PendulumCollapse,       // The humanoid falls backwards uncontrollably
    LiftSuccess,            // The humanoid successfully lifts the package
}

impl Phase {
    fn as_str(&self) -> &'static str {
        match self {
            Phase::ApproachingPackage => "APPROACHING_PACKAGE",
            Phase::WholeBodyTensionLift => "WHOLE_BODY_TENSION_LIFT",
            Phase::KineticFrictionSlip => "KINETIC_FRICTION_SLIP",
            Phase::PendulumCollapse => "INVERTED_PENDULUM_COLLAPSE",
            Phase::LiftSuccess => "LIFT_SUCCESS",
        }
    }
}

#[derive(Debug)]
#[allow(dead_code)]
struct TrajectoryResult {
    id: u32,
    short_id: String,
    final_pitch_angle: f64,
    final_slip_velocity: f64,
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

    // Environmental Injection
    let failure = if rng.chance(0.15) { FailureMode::VirtualCleanSim }
    else if rng.chance(0.35) { FailureMode::NominalWarehouse }
    else { FailureMode::MicroDustAccumulation };

    let true_static_mu = match failure {
        FailureMode::VirtualCleanSim => rng.range(0.75, 0.85),
        FailureMode::NominalWarehouse => rng.range(0.55, 0.65),
        FailureMode::MicroDustAccumulation => rng.range(0.30, 0.40), // Lethal failure zone
    };

    // Kinetic sliding friction is always ~40% lower than static
    let true_kinetic_mu = true_static_mu * 0.6;

    let dt = 0.001; // 1000Hz Euler Integration
    let max_time_s = 4.0; 
    let max_steps = (max_time_s / dt) as usize;

    let lift_initiation_time = 1.0; 

    let mut phase = Phase::ApproachingPackage;
    let mut step = 0_usize;

    // Physical State Matrix
    let mut is_slipping = false;
    let mut foot_slip_velocity = 0.0; 
    let mut foot_slip_distance = 0.0; 
    
    // Inverted Pendulum Angle (0.0 = standing perfectly upright. Negative = falling backward)
    let mut pendulum_pitch_rad = 0.0;
    let mut pendulum_pitch_vel = 0.0;

    let mut final_outcome = "TIMEOUT_ERROR";

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(TOTAL_MASS);

    let mut telemetry = Vec::new();

    // ═══ THE FORGE: PHYSICAL INTEGRATION ═══
    while step < max_steps {
        let t = step as f64 * dt;

        let total_normal_force = TOTAL_MASS * GRAVITY;

        // 1. THE WHOLE-BODY HEAVE (idealized commanded lift)
        let mut commanded_shear_force = 0.0;

        if t >= lift_initiation_time && phase == Phase::ApproachingPackage {
            phase = Phase::WholeBodyTensionLift;
        }

        if phase == Phase::WholeBodyTensionLift || phase == Phase::KineticFrictionSlip {
            // To lift 20kg (half its body weight), the 50kg robot CANNOT just use its arms.
            // The policy commands a whole-body heave. The hips violently throw backward horizontally
            // to act as a counter-lever. This generates massive horizontal shear force at the feet.
            let horizontal_heave_accel = 4.5; // m/s^2 horizontal acceleration of the torso
            commanded_shear_force = TOTAL_MASS * horizontal_heave_accel; 
        }

        // 2. THERMODYNAMIC FRICTION CHECK
        if !is_slipping && commanded_shear_force > 0.0 {
            // Does the required lift leverage exceed the physical binding of the warehouse floor?
            let structural_grip = total_normal_force * true_static_mu;
            
            if commanded_shear_force > structural_grip {
                // The friction wall shatters. The feet begin sliding forward instantly.
                is_slipping = true;
                phase = Phase::KineticFrictionSlip;
            }
        }

        // 3. PHYSICAL SLIP KINEMATICS
        if is_slipping {
            // Once slipping, resistance plummets to Kinetic Mu
            let kinetic_grip = total_normal_force * true_kinetic_mu;
            
            // The motor still pushes with commanded shear, but only Kinetic Grip resists it
            let net_sliding_force = commanded_shear_force - kinetic_grip;
            
            if net_sliding_force > 0.0 {
                let foot_accel = net_sliding_force / TOTAL_MASS;
                foot_slip_velocity += foot_accel * dt;
                foot_slip_distance += foot_slip_velocity * dt;
            }
        }

        // 4. INVERTED PENDULUM COLLAPSE
        if is_slipping && foot_slip_distance > 0.05 { // If feet slip more than 5cm
            // The base of support is gone. The 70kg combined mass falls backward.
            // Simplified angular acceleration: alpha = Torque / Inertia
            // Torque = m * g * (distance feet traveled forward relative to CoG)
            let instability_torque = TOTAL_MASS * GRAVITY * foot_slip_distance;
            
            let rotational_inertia = TOTAL_MASS * (CENTER_OF_GRAVITY_HEIGHT * CENTER_OF_GRAVITY_HEIGHT);
            let angular_accel = -instability_torque / rotational_inertia; // Negative is falling backward

            pendulum_pitch_vel += angular_accel * dt;
            pendulum_pitch_rad += pendulum_pitch_vel * dt;
        }

        // 5. FATAL BOUNDARY CHECK
        if pendulum_pitch_rad <= -0.785 { // Falls 45 degrees backward
            phase = Phase::PendulumCollapse;
            final_outcome = "BATTERY_CASING_RUPTURE_FATALITY";
            break;
        }

        if t > 3.0 && !is_slipping {
            phase = Phase::LiftSuccess;
            final_outcome = "WHOLE_BODY_LIFT_SUCCESS";
            break;
        }

        // 6. RECORD STATE
        if t % 5.0 < dt { // 0.2Hz recording for file precision
            proof.feed_f64(foot_slip_distance);
            proof.feed_f64(pendulum_pitch_rad);

            if record_telemetry {
                telemetry.push(serde_json::json!({
                    "t": t,
                    "slip_dist": foot_slip_distance,
                    "pitch": pendulum_pitch_rad,
                    "phase": phase.as_str()
                }));
            }
        }

        step += 1;
    }

    if step >= max_steps && phase != Phase::LiftSuccess && phase != Phase::PendulumCollapse {
        final_outcome = "TIMEOUT_STALL";
    }

    proof.feed_f64(foot_slip_distance);
    proof.feed_f64(pendulum_pitch_rad);
    proof.feed_str(final_outcome);
    let proof_hash = proof.seal();

    TrajectoryResult {
        id,
        short_id,
        final_pitch_angle: pendulum_pitch_rad,
        final_slip_velocity: foot_slip_velocity,
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
        println!("  G^G BIPEDAL WHOLE-BODY LIFT COLLAPSE MONTE CARLO");
        println!("  Verifying Inverted Pendulum Stability vs Idealized Gait Fluidity");
        println!("====================================================================");
        println!();
        println!("  Trajectories:  {}", n_trajectories);
        println!("  Physics:       1000Hz High-Friction Static to Low-Mu Kinetic Shear");
        println!("  Sensors:       Humanoid Internal IMU & Ground Toe-Push Matrix");
        println!("  Estimation:    Idealized trainers assume concrete Mu is perfectly static");
        println!("  Boundary:      Unrecoverable Inverted Pendulum Battery Rupture");
        println!("====================================================================");
        println!();
    }

    let start = Instant::now();
    let record_telemetry = false; 
    let counter = std::sync::atomic::AtomicUsize::new(0);

    let (tx, rx) = std::sync::mpsc::sync_channel::<TrajectoryResult>(20000);

    let json_writer = if let Some(path) = &json_path {
        let metadata = DatasetMetadata {
            generator: "G^G Sovereign Auditing v1.0".to_string(),
            domain: "bipedal_robotics".to_string(),
            scenario: "humanoid_whole_body_lift_collapse".to_string(),
            trajectories: n_trajectories as usize,
            physics_engine: "genesis_core::inverted_pendulum_friction (1000Hz)".to_string(),
            version: "1.0.0".to_string(),
            generated_at: output::now_iso(),
        };
        let mut streamer = output::DatasetStreamer::new(path, &metadata).expect("Failed to create streamer");
        let mut run_proof_chain = ProofChain::new();

        let handle = std::thread::spawn(move || {
            let mut results = Vec::new();
            for r in rx {
                let rec = TrajectoryRecord {
                    id: format!("biped_audit_{}", r.short_id),
                    traj_type: "bipedal_friction_slip_collapse".to_string(),
                    scenario: match r.failure {
                        FailureMode::VirtualCleanSim => "idealized_immaculate_concrete".to_string(),
                        FailureMode::NominalWarehouse => "nominal_clean_warehouse_friction".to_string(),
                        FailureMode::MicroDustAccumulation => "cardboard_dust_shear_friction_drop".to_string(),
                    },
                    steps: r.steps,
                    score: serde_json::json!({
                        "survived": r.outcome == "WHOLE_BODY_LIFT_SUCCESS",
                        "final_pitch_deg": (r.final_pitch_angle.to_degrees() * 100.0).round() / 100.0,
                        "spine_shattered": r.outcome == "BATTERY_CASING_RUPTURE_FATALITY",
                    }),
                    proof_hash: r.proof_hash.clone(),
                    reasoning_context: serde_json::json!({
                        "is_anomaly": r.outcome != "WHOLE_BODY_LIFT_SUCCESS",
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
                let mut rng = Rng::new(0xDEAD_BEEF_C0FE_1337 ^ (i as u64).wrapping_mul(0x9E37_79B9_FACE_D00D));
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
                let mut rng = Rng::new(0xDEAD_BEEF_C0FE_1337 ^ (i as u64).wrapping_mul(0x9E37_79B9_FACE_D00D));
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
    let success = results.iter().filter(|r| r.outcome == "WHOLE_BODY_LIFT_SUCCESS").count();
    let collapse = results.iter().filter(|r| r.outcome == "BATTERY_CASING_RUPTURE_FATALITY").count();
    let timeout = results.iter().filter(|r| r.outcome == "TIMEOUT_STALL").count();

    println!("====================================================================");
    println!("  WHOLE-BODY BIPEDAL LIFT RESULTS");
    println!("====================================================================");
    println!();
    println!("  Total Trajectories:    {}", total);
    println!("  Elapsed:               {:.3}s", elapsed.as_secs_f64());
    println!("  Throughput:            {:.0} trajectories/sec", total as f64 / elapsed.as_secs_f64());
    println!();
    println!("  +---------------------------------------------+");
    println!("  | OUTCOME DISTRIBUTION                         |");
    println!("  +---------------------------------------------+");
    println!("  | WHOLE-BODY LIFT SUCCESS:       {:>6} ({:>5.1}%)  |", success, success as f64 / total as f64 * 100.0);
    println!("  | BATTERY RUPTURE (FATAL):     {:>6} ({:>5.1}%)  |", collapse, collapse as f64 / total as f64 * 100.0);
    println!("  | TIMEOUT PERDITION:           {:>6} ({:>5.1}%)  |", timeout, timeout as f64 / total as f64 * 100.0);
    println!("  +---------------------------------------------+");
    println!();

    // ═══ FAILURE MODE SUMMARY ═══
    println!("  +---------------------------------------------+");
    println!("  | VULNERABILITY IMPACT (Mechanical Loss Rate)  |");
    println!("  +---------------------------------------------+");
    let clean: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::VirtualCleanSim)).collect();
    let lab: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::NominalWarehouse)).collect();
    let dust: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::MicroDustAccumulation)).collect();

    let crash_rate = |v: &[&TrajectoryResult]| -> f64 {
        if v.is_empty() { 0.0 } else { v.iter().filter(|r| r.outcome != "WHOLE_BODY_LIFT_SUCCESS").count() as f64 / v.len() as f64 * 100.0 }
    };

    println!("  | Idealized Immaculate (Mu 0.8): {:>4.1}% ({:>6} runs) |", crash_rate(&clean), clean.len());
    println!("  | Nominal Warehouse (Mu 0.6):   {:>4.1}% ({:>6} runs) |", crash_rate(&lab), lab.len());
    println!("  | Dust Degraded Floor (Mu 0.35): {:>4.1}% ({:>6} runs) |", crash_rate(&dust), dust.len());
    println!("  +---------------------------------------------+");
    println!();
    println!("  Run Proof Seal: {}", run_proof);
    println!("====================================================================");
}
