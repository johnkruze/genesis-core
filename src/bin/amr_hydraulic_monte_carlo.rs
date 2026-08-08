// G^G AMR HYDRAULIC SHEAR MONTE CARLO
// Sovereign Verification: 1000kg Inertial Braking vs Fluid Dynamic Hallucination
//
// THE EMBODIMENT: An warehouse AMR / Kiva Autonomous Mobile Robot (AMR). 
// The chassis weighs 500kg and it is transporting a 500kg inventory rack.
// Total Kinetic Mass = 1,000 kg.
// 
// THE VULNERABILITY: The AMR is traveling at 2.0 m/s (4.5 mph) through a central 
// warehouse artery. A human worker steps out from a blind intersection exactly 3.0 
// meters ahead. The robot's LiDAR detects the human and the RL policy executes 
// Emergency Maximum Braking.
//
// THE AI HALLUCINATION: Reinforcement Learning environments (idealized rigid-body trainers) 
// model "Standard Warehouse Floor" with a fixed coefficient of kinetic friction 
// (Mu = 0.6). At Mu = 0.6, a 1000kg robot decelerates at 5.88 m/s^2. It mathematically 
// stops in 0.34 meters. The AI learns that "Braking = Instant Safety".
//
// THE MATHEMATICAL REALITY: Generative physics rigidly ignore real-time fluid dynamics
// because simulating million-particle liquids drops engine frame-rates. If a forklift 
// previously traversed the same aisle and leaked a microscopic 0.2mm puddle of hydraulic 
// fluid, the kinetic friction coefficient drops to Mu = 0.05.
//
// THE FATALITY: When Emergency Braking engages on the hydraulic fluid, the 1,000 kg 
// inertial vector completely shears through the Mu = 0.05 grip. Deceleration plummets 
// to 0.49 m/s^2. The robot hydroplanes for 4.08 meters, blowing straight through the 
// 3.0-meter safety boundary and crushing the human worker against a steel racking unit.

use std::time::Instant;
use genesis_core::rng::Rng;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output::{self, DatasetMetadata, TrajectoryRecord};

const GRAVITY: f64 = 9.81;

// Mass and Velocity Geometry
const AMR_MASS_KG: f64 = 500.0;
const CARGO_MASS_KG: f64 = 500.0;
const TOTAL_MASS_KG: f64 = AMR_MASS_KG + CARGO_MASS_KG;

const CRUISE_VELOCITY_MPS: f64 = 2.0;

// The geometry of the disaster
const HUMAN_DISTANCE_M: f64 = 3.0; // The human is exactly 3.0 meters ahead when braking begins

#[derive(Debug, Clone, Copy, PartialEq)]
enum FailureMode {
    VirtualCleanSim,        // Idealized lab concrete (Mu = 0.6)
    NominalWarehouse,       // Standard clean warehouse floor (Mu = 0.55)
    HydraulicFluidLeak,     // 0.2mm puddle of forklift fluid (Mu = 0.05)
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    CruiseVelocity,         // AMR rolling down the aisle
    EmergencyBraking,       // Calipers locked, attempting to decelerate
    HydroplaneShear,        // Inertia overpowers the grip, AMR is sliding
    LethalImpact,           // The robot travels > 3.0m and crushes the human
    SafeStop,               // The robot stops before 3.0m
}

impl Phase {
    fn as_str(&self) -> &'static str {
        match self {
            Phase::CruiseVelocity => "CRUISE_VELOCITY",
            Phase::EmergencyBraking => "EMERGENCY_BRAKING_LOCK",
            Phase::HydroplaneShear => "DIAGNOSTIC_HYDROPLANE_SHEAR",
            Phase::LethalImpact => "HUMAN_CRUSH_IMPACT",
            Phase::SafeStop => "SAFE_COLLISION_AVOIDANCE",
        }
    }
}

#[derive(Debug)]
#[allow(dead_code)]
struct TrajectoryResult {
    id: u32,
    short_id: String,
    braking_distance_m: f64,
    time_to_stop_s: f64,
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
    else { FailureMode::HydraulicFluidLeak };

    let kinetic_mu = match failure {
        FailureMode::VirtualCleanSim => rng.range(0.60, 0.65), // Strong predictable deceleration
        FailureMode::NominalWarehouse => rng.range(0.50, 0.58), // Slight variation
        FailureMode::HydraulicFluidLeak => rng.range(0.04, 0.08), // Lethal failure zone (Ice-like)
    };

    let dt = 0.001; // 1000Hz Euler Integration
    let max_time_s = 10.0; 
    let max_steps = (max_time_s / dt) as usize;

    let braking_initiation_time = 1.0; 

    let mut phase = Phase::CruiseVelocity;
    let mut step = 0_usize;

    // Physical State Matrix
    let mut velocity_mps = CRUISE_VELOCITY_MPS;
    let mut distance_traveled_m = 0.0; 
    let mut braking_distance_m = 0.0;
    let mut braking_time_s = 0.0;

    let mut final_outcome = "TIMEOUT_ERROR";

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(TOTAL_MASS_KG);

    let mut telemetry = Vec::new();

    // ═══ THE FORGE: PHYSICAL INTEGRATION ═══
    while step < max_steps {
        let t = step as f64 * dt;

        if t >= braking_initiation_time && phase == Phase::CruiseVelocity {
            phase = Phase::EmergencyBraking;
        }

        if phase == Phase::EmergencyBraking || phase == Phase::HydroplaneShear {
            braking_time_s += dt;

            // RL models assume instantaneous Caliper lock (0.0 seconds).
            // Physical hardware requires ~0.2s to build hydraulic lock pressure against the discs.
            let caliper_lock_latency_s = 0.20;
            
            let mut actual_deceleration = 0.0;
            if braking_time_s > caliper_lock_latency_s {
                // Maximum Kinetic Friction Deceleration
                // F_f = Mu_k * N => m * a = Mu_k * m * g => a = Mu_k * g
                actual_deceleration = kinetic_mu * GRAVITY;
            }

            // The robot hydroplanes if deceleration is physically too low for its momentum
            if braking_time_s > caliper_lock_latency_s && actual_deceleration < 3.0 && phase == Phase::EmergencyBraking {
                phase = Phase::HydroplaneShear;
            }

            velocity_mps -= actual_deceleration * dt;
            
            if velocity_mps <= 0.0 {
                velocity_mps = 0.0;
            }

            let delta_dist = velocity_mps * dt;
            distance_traveled_m += delta_dist;
            braking_distance_m += delta_dist;
        } else {
            // Cruising
            distance_traveled_m += velocity_mps * dt;
        }

        // 5. FATAL BOUNDARY CHECK
        if braking_distance_m >= HUMAN_DISTANCE_M { 
            // The robot has slid past the 3.0 meter boundary while braking.
            phase = Phase::LethalImpact;
            final_outcome = "HUMAN_CRUSH_IMPACT_FATALITY";
            break;
        }

        if velocity_mps <= 0.0 && phase != Phase::CruiseVelocity {
            // Robot successfully stopped
            phase = Phase::SafeStop;
            final_outcome = "SAFE_COLLISION_AVOIDANCE";
            break;
        }

        // 6. RECORD STATE
        if t % 5.0 < dt { // 0.2Hz recording for file precision
            proof.feed_f64(velocity_mps);
            proof.feed_f64(braking_distance_m);

            if record_telemetry {
                telemetry.push(serde_json::json!({
                    "t": t,
                    "vel": velocity_mps,
                    "brake_dist": braking_distance_m,
                    "phase": phase.as_str()
                }));
            }
        }

        step += 1;
    }

    if step >= max_steps && phase != Phase::SafeStop && phase != Phase::LethalImpact {
        final_outcome = "TIMEOUT_STALL";
    }

    proof.feed_f64(velocity_mps);
    proof.feed_f64(braking_distance_m);
    proof.feed_str(final_outcome);
    let proof_hash = proof.seal();

    TrajectoryResult {
        id,
        short_id,
        braking_distance_m,
        time_to_stop_s: braking_time_s,
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
        println!("  G^G AMR HYDRAULIC SHEAR MONTE CARLO");
        println!("  Verifying 1000kg Inertial Braking vs Fluid Dynamic Slip");
        println!("====================================================================");
        println!();
        println!("  Trajectories:  {}", n_trajectories);
        println!("  Physics:       1000Hz Rigid Body Mass Deceleration vs Kinetic Range");
        println!("  Sensors:       Collision LiDAR & Maximum Regenerative Brakes");
        println!("  Estimation:    Idealized trainers assume fixed Mu=0.6 floor friction");
        println!("  Boundary:      3.0 Meter Autonomous Human Collision");
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
            domain: "logistics_robotics".to_string(),
            scenario: "amr_hydraulic_shear".to_string(),
            trajectories: n_trajectories as usize,
            physics_engine: "genesis_core::hydroplane_shear_kinematics (1000Hz)".to_string(),
            version: "1.0.0".to_string(),
            generated_at: output::now_iso(),
        };
        let mut streamer = output::DatasetStreamer::new(path, &metadata).expect("Failed to create streamer");
        let mut run_proof_chain = ProofChain::new();

        let handle = std::thread::spawn(move || {
            let mut results = Vec::new();
            for r in rx {
                let rec = TrajectoryRecord {
                    id: format!("amr_audit_{}", r.short_id),
                    traj_type: "hydraulic_fluid_hydroplane_shear".to_string(),
                    scenario: match r.failure {
                        FailureMode::VirtualCleanSim => "idealized_immaculate_concrete_traction".to_string(),
                        FailureMode::NominalWarehouse => "nominal_clean_warehouse_slab".to_string(),
                        FailureMode::HydraulicFluidLeak => "forklift_hydraulic_fluid_leak".to_string(),
                    },
                    steps: r.steps,
                    score: serde_json::json!({
                        "survived": r.outcome == "SAFE_COLLISION_AVOIDANCE",
                        "braking_distance_meters": (r.braking_distance_m * 100.0).round() / 100.0,
                        "crushed_human": r.outcome == "HUMAN_CRUSH_IMPACT_FATALITY",
                    }),
                    proof_hash: r.proof_hash.clone(),
                    reasoning_context: serde_json::json!({
                        "is_anomaly": r.outcome != "SAFE_COLLISION_AVOIDANCE",
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
                let mut rng = Rng::new(0xB00B_BEEF_C0FE_1337 ^ (i as u64).wrapping_mul(0xC001_D00D_FACE_BEEF));
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
                let mut rng = Rng::new(0xB00B_BEEF_C0FE_1337 ^ (i as u64).wrapping_mul(0xC001_D00D_FACE_BEEF));
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
    let success = results.iter().filter(|r| r.outcome == "SAFE_COLLISION_AVOIDANCE").count();
    let crush = results.iter().filter(|r| r.outcome == "HUMAN_CRUSH_IMPACT_FATALITY").count();
    let timeout = results.iter().filter(|r| r.outcome == "TIMEOUT_STALL").count();

    println!("====================================================================");
    println!("  AMR HYDRAULIC BRAKING RESULTS");
    println!("====================================================================");
    println!();
    println!("  Total Trajectories:    {}", total);
    println!("  Elapsed:               {:.3}s", elapsed.as_secs_f64());
    println!("  Throughput:            {:.0} trajectories/sec", total as f64 / elapsed.as_secs_f64());
    println!();
    println!("  +---------------------------------------------+");
    println!("  | OUTCOME DISTRIBUTION                         |");
    println!("  +---------------------------------------------+");
    println!("  | SAFE COLLISION AVOIDANCE     {:>6} ({:>5.1}%)  |", success, success as f64 / total as f64 * 100.0);
    println!("  | HUMAN CRUSH (FATAL IMPACT):  {:>6} ({:>5.1}%)  |", crush, crush as f64 / total as f64 * 100.0);
    println!("  | TIMEOUT PERDITION:           {:>6} ({:>5.1}%)  |", timeout, timeout as f64 / total as f64 * 100.0);
    println!("  +---------------------------------------------+");
    println!();

    // ═══ FAILURE MODE SUMMARY ═══
    println!("  +---------------------------------------------+");
    println!("  | VULNERABILITY IMPACT (Mechanical Loss Rate)  |");
    println!("  +---------------------------------------------+");
    let clean: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::VirtualCleanSim)).collect();
    let lab: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::NominalWarehouse)).collect();
    let fluid: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::HydraulicFluidLeak)).collect();

    let crash_rate = |v: &[&TrajectoryResult]| -> f64 {
        if v.is_empty() { 0.0 } else { v.iter().filter(|r| r.outcome != "SAFE_COLLISION_AVOIDANCE").count() as f64 / v.len() as f64 * 100.0 }
    };

    println!("  | Idealized Immaculate (Mu 0.6): {:>4.1}% ({:>6} runs) |", crash_rate(&clean), clean.len());
    println!("  | Nominal Warehouse (Mu 0.55):  {:>4.1}% ({:>6} runs) |", crash_rate(&lab), lab.len());
    println!("  | 0.2mm Hydraulic Fluid (Mu 0.05): {:>3.1}% ({:>6} runs) |", crash_rate(&fluid), fluid.len());
    println!("  +---------------------------------------------+");
    println!();
    println!("  Run Proof Seal: {}", run_proof);
    println!("====================================================================");
}
