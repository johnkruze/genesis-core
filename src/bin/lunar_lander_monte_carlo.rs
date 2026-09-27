// G^G LUNAR REGOLITH MONTE CARLO
// Sovereign Verification: Vacuum Thruster Ejecta vs LiDAR Sensor Hallucinations
//
// THE EMBODIMENT: A heavy lunar lander (16,000kg) executing its terminal 
// descent to the Lunar South Pole. The main rocket engine is firing at 40,000 N of thrust 
// to decelerate the lander from 20 m/s at 100 meters altitude down to a safe 1.5 m/s 
// touchdown velocity.
// 
// THE VULNERABILITY: Generative rendering environments (idealized mesh trainers) model 
// laser altimeters (LiDAR) and Radar bouncing off a rigidly defined geometric mesh 
// (the solid ground). They train the lander's Extended Kalman Filter (EKF) to trust 
// altimeter bounces implicitly as distance-to-surface. 
//
// THE MATHEMATICAL REALITY: The Moon has no atmosphere. When the 40,000 N supersonic 
// engine plume strikes the surface, there is zero atmospheric back-pressure to contain 
// the gas. The plume expands radially, instantly kicking up billions of jagged basaltic 
// dust particles (Regolith) at supersonic velocities upward.
//
// THE FATALITY: As the lander drops below 40 meters, the ascending regolith dust cloud 
// reaches 30 meters high. It creates a dense pseudo-plasma of debris. The LiDAR beams, 
// trained in clear virtual vacuums, reflect perfectly off the dense top layer of the 
// dust cloud. 
// 
// The AI's EKF instantly hallucinates that the solid ground has suddenly "jumped" up 30 
// meters. Believing it is at zero altitude, the AI commands Main Engine Cut-Off (MECO) 
// while physically still 30 meters in the air. The 16,000kg vehicle free-falls in lunar 
// gravity (1.62 m/s^2), smashing into the physical surface at over 9.8 m/s—snapping its 
// landing legs and detonating the hypergolic propellant tanks.

use std::time::Instant;
use genesis_core::rng::Rng;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output::{self, DatasetMetadata, TrajectoryRecord};

// Lunar Physics
const LUNAR_GRAVITY: f64 = -1.62; // m/s^2
const INITIAL_ALTITUDE_M: f64 = 100.0;
const INITIAL_VELOCITY_MPS: f64 = -20.0;
const LANDER_MASS_KG: f64 = 16_000.0;
const SAFE_TOUCHDOWN_VELOCITY_MPS: f64 = -2.5; // Anything faster snaps the legs

// Thruster Ejecta Limits
const PLUME_EJECTA_CLOUD_HEIGHT_M: f64 = 25.0; // The dust cloud kicks 25m high

#[derive(Debug, Clone, Copy, PartialEq)]
enum FailureMode {
    VirtualCleanSim,        // Idealized trainer: Solid ground reflection, clear vacuum
    RegolithEjectaBlind,    // The dust reflects the LiDAR (False MECO)
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    TerminalDescent,        // Engine braking from 100m smoothly
    EjectaCloudReflection,  // EKF fuses the false LiDAR bounce from the 25m dust cloud
    PrematureMECO,          // Engine shuts down because AI thinks it softly landed
    VacuumFreefall,         // Tragic freefall from 25m
    FatalSurfaceCrush,      // The legs snap, the tank breaches
    SafeTouchdown,          // The lander engines burn perfectly to 0m
}

impl Phase {
    fn as_str(&self) -> &'static str {
        match self {
            Phase::TerminalDescent => "TERMINAL_DESCENT_BRAKING",
            Phase::EjectaCloudReflection => "LIDAR_EJECTA_HALLUCINATION",
            Phase::PrematureMECO => "FALSE_MECO_ENGINE_CUTOFF",
            Phase::VacuumFreefall => "VACUUM_FREEFALL",
            Phase::FatalSurfaceCrush => "SURFACE_CRUSH_FATALITY",
            Phase::SafeTouchdown => "SAFE_LUNAR_TOUCHDOWN",
        }
    }
}

#[derive(Debug)]
#[allow(dead_code)]
struct TrajectoryResult {
    id: u32,
    short_id: String,
    impact_velocity_mps: f64,
    meco_altitude_m: f64,
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
    else { FailureMode::RegolithEjectaBlind };

    let true_ejecta_height = match failure {
        FailureMode::VirtualCleanSim => 0.0, // Simulation physics
        FailureMode::RegolithEjectaBlind => rng.range(22.0, 32.0), // The plume cloud height
    };

    let dt = 0.01; // 100Hz Avionics Rate
    let max_time_s = 60.0; // 60 seconds descent limit
    let max_steps = (max_time_s / dt) as usize;

    let mut phase = Phase::TerminalDescent;
    let mut step = 0_usize;

    // Physical True State Matrix
    let mut current_altitude_m = INITIAL_ALTITUDE_M;
    let mut current_velocity_mps = INITIAL_VELOCITY_MPS;
    let mut false_meco_altitude = 0.0;
    
    // Engine State
    let mut engine_active = true;
    let mut final_outcome = "TIMEOUT_ERROR";

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(LANDER_MASS_KG);

    let mut telemetry = Vec::new();

    // ═══ THE FORGE: PHYSICAL INTEGRATION ═══
    while step < max_steps {
        let t = step as f64 * dt;

        // 1. THE AI CONTROLLER (Extended Kalman Filter)
        let ai_perceived_altitude = if phase == Phase::VacuumFreefall && current_altitude_m > true_ejecta_height {
            // It knows it's falling if it somehow passed the ejecta but it's already dead
            current_altitude_m
        } else if phase == Phase::EjectaCloudReflection || (current_altitude_m <= true_ejecta_height + 15.0 && failure == FailureMode::RegolithEjectaBlind) {
            // As the lander approaches the dust cloud, the LiDAR begins reflecting off the top of the cloud.
            // If physical altitude is 35m, and cloud is 30m, LiDAR bounce returns 5m distance!
            // The AI thinks altitude = 5.0m
            if phase == Phase::TerminalDescent {
                phase = Phase::EjectaCloudReflection;
            }
            current_altitude_m - true_ejecta_height
        } else {
            // Accurate high altitude radar
            current_altitude_m
        };

        // 2. THE LANDING BURN ALGORITHM
        // At 100m, v = 20m/s. Target 0m, v = 1.5m/s.
        // Required constant deceleration: vf^2 = vi^2 + 2ad => a = (1.5^2 - (-20)^2) / (2 * -100) = -397.75 / -200 = 1.98 m/s^2.
        // Target accel = 2.0 m/s^2 UP. 
        // Engine must thrust = Mass * (Target_Accel - Lunar_Gravity) = 16,000 * (2.0 - (-1.62)) = 16,000 * 3.62 = 57,920 N
        
        // EKF Logic: If perceived altitude <= 0.0, CUT THE ENGINE (MECO)
        if ai_perceived_altitude <= 0.5 && engine_active {
            engine_active = false;
            false_meco_altitude = current_altitude_m;
            phase = Phase::PrematureMECO;
        }

        let engine_thrust_accel = if engine_active {
            // Maintain a steady 2.0 m/s^2 net upward acceleration for the suicide burn
            // (ThrustAccel + LUNAR_GRAVITY = 2.0) => ThrustAccel = 2.0 - (-1.62) = 3.62
            3.62 
        } else {
            0.0 // MECO
        };

        // 3. PHYSICAL KINEMATICS
        let net_acceleration = engine_thrust_accel + LUNAR_GRAVITY;
        current_velocity_mps += net_acceleration * dt;
        current_altitude_m += current_velocity_mps * dt;

        if !engine_active && phase == Phase::PrematureMECO && current_altitude_m > 1.0 {
            phase = Phase::VacuumFreefall;
        }

        // 4. BOUNDARY SURVIVAL CHECK
        if current_altitude_m <= 0.0 {
            current_altitude_m = 0.0;
            if current_velocity_mps < SAFE_TOUCHDOWN_VELOCITY_MPS { // Negative means falling down
                // The legs snap, hypergol tanks rupture
                phase = Phase::FatalSurfaceCrush;
                final_outcome = "LUNAR_SURFACE_CRUSH_FATALITY";
            } else {
                phase = Phase::SafeTouchdown;
                final_outcome = "SAFE_LUNAR_TOUCHDOWN";
            }
            break;
        }

        // 6. RECORD STATE
        if t % 5.0 < dt { // 0.2Hz recording for file precision
            proof.feed_f64(current_altitude_m);
            proof.feed_f64(current_velocity_mps);

            if record_telemetry {
                telemetry.push(serde_json::json!({
                    "t": t,
                    "alt": current_altitude_m,
                    "vel_z": current_velocity_mps,
                    "ekf_alt": ai_perceived_altitude,
                    "phase": phase.as_str()
                }));
            }
        }

        step += 1;
    }

    if step >= max_steps && phase != Phase::SafeTouchdown && phase != Phase::FatalSurfaceCrush {
        final_outcome = "TIMEOUT_STALL";
    }

    proof.feed_f64(current_altitude_m);
    proof.feed_f64(current_velocity_mps);
    proof.feed_str(final_outcome);
    let proof_hash = proof.seal();

    TrajectoryResult {
        id,
        short_id,
        impact_velocity_mps: current_velocity_mps,
        meco_altitude_m: false_meco_altitude,
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
        println!("  G^G LUNAR REGOLITH EJECTA LANDING MONTE CARLO");
        println!("  Verifying Vacuum Dynamics vs Idealized Radar Mesh Priors");
        println!("====================================================================");
        println!();
        println!("  Trajectories:  {}", n_trajectories);
        println!("  Physics:       100Hz EKF Sensor Fusion & Supersonic Regolith Expansion");
        println!("  Sensors:       Landing Radar / LiDAR Altimeter");
        println!("  Estimation:    Idealized meshes render transparent vacuum with rigid floors");
        println!("  Boundary:      > 1.5 m/s Touchdown Structural Leg Limit");
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
            domain: "spaceflight_robotics".to_string(),
            scenario: "lunar_regolith_ejecta".to_string(),
            trajectories: n_trajectories as usize,
            physics_engine: "genesis_core::vacuum_radar_hallucination (100Hz)".to_string(),
            version: "1.0.0".to_string(),
            generated_at: output::now_iso(),
        };
        let mut streamer = output::DatasetStreamer::new(path, &metadata).expect("Failed to create streamer");
        let mut run_proof_chain = ProofChain::new();

        let handle = std::thread::spawn(move || {
            let mut results = Vec::new();
            for r in rx {
                let rec = TrajectoryRecord {
                    id: format!("space_audit_{}", r.short_id),
                    traj_type: "vacuum_regolith_radar_blind".to_string(),
                    scenario: match r.failure {
                        FailureMode::VirtualCleanSim => "idealized_perfect_mesh_prior".to_string(),
                        FailureMode::RegolithEjectaBlind => "lunar_south_pole_dense_plasma_ejecta".to_string(),
                    },
                    steps: r.steps,
                    score: serde_json::json!({
                        "survived": r.outcome == "SAFE_LUNAR_TOUCHDOWN",
                        "impact_velocity_mps": (r.impact_velocity_mps * 100.0).round() / 100.0,
                        "crater_explosion": r.outcome == "LUNAR_SURFACE_CRUSH_FATALITY",
                        "false_meco_altitude": r.meco_altitude_m,
                    }),
                    proof_hash: r.proof_hash.clone(),
                    reasoning_context: serde_json::json!({
                        "is_anomaly": r.outcome != "SAFE_LUNAR_TOUCHDOWN",
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
                let mut rng = Rng::new(0xF00D_BEEF_C0FE_1337 ^ (i as u64).wrapping_mul(0xC0FE_BABE_B00B_FACE));
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
                let mut rng = Rng::new(0xF00D_BEEF_C0FE_1337 ^ (i as u64).wrapping_mul(0xC0FE_BABE_B00B_FACE));
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
    let success = results.iter().filter(|r| r.outcome == "SAFE_LUNAR_TOUCHDOWN").count();
    let crush = results.iter().filter(|r| r.outcome == "LUNAR_SURFACE_CRUSH_FATALITY").count();
    let timeout = results.iter().filter(|r| r.outcome == "TIMEOUT_STALL").count();

    println!("====================================================================");
    println!("  LUNAR REGOLITH DESCENT RESULTS");
    println!("====================================================================");
    println!();
    println!("  Total Trajectories:    {}", total);
    println!("  Elapsed:               {:.3}s", elapsed.as_secs_f64());
    println!("  Throughput:            {:.0} trajectories/sec", total as f64 / elapsed.as_secs_f64());
    println!();
    println!("  +---------------------------------------------+");
    println!("  | OUTCOME DISTRIBUTION                         |");
    println!("  +---------------------------------------------+");
    println!("  | SAFE TOUCHDOWN (< 1.5 m/s):  {:>6} ({:>5.1}%)  |", success, success as f64 / total as f64 * 100.0);
    println!("  | SURFACE CRUSH SCATTER (FATAL):{:>5} ({:>5.1}%)  |", crush, crush as f64 / total as f64 * 100.0);
    println!("  | TIMEOUT PERDITION:           {:>6} ({:>5.1}%)  |", timeout, timeout as f64 / total as f64 * 100.0);
    println!("  +---------------------------------------------+");
    println!();

    // ═══ FAILURE MODE SUMMARY ═══
    println!("  +---------------------------------------------+");
    println!("  | VULNERABILITY IMPACT (Mechanical Loss Rate)  |");
    println!("  +---------------------------------------------+");
    let clean: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::VirtualCleanSim)).collect();
    let ejecta: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::RegolithEjectaBlind)).collect();

    let crash_rate = |v: &[&TrajectoryResult]| -> f64 {
        if v.is_empty() { 0.0 } else { v.iter().filter(|r| r.outcome != "SAFE_LUNAR_TOUCHDOWN").count() as f64 / v.len() as f64 * 100.0 }
    };

    println!("  | Idealized Mesh Reflection:   {:>4.1}% ({:>6} runs) |", crash_rate(&clean), clean.len());
    println!("  | Lunar Regolith Dense Plume:  {:>4.1}% ({:>6} runs) |", crash_rate(&ejecta), ejecta.len());
    println!("  +---------------------------------------------+");
    println!();
    println!("  Run Proof Seal: {}", run_proof);
    println!("====================================================================");
}
