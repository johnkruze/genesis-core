// G^G QUADRUPED THERMAL RUNAWAY MONTE CARLO (quadruped platform STRIKE II)
// Sovereign Verification: Thermodynamic Winding Saturation vs AI Control
//
// THE EMBODIMENT: A quadruped platform V60 Quadruped (50kg) carrying a 15kg external
// sensor payload (65kg total mass) navigating a continuous 15-degree incline.
// The ambient temperature is 45°C (113°F, e.g., Middle Eastern desert deployment).
// 
// THE VULNERABILITY: Idealized RL trainers do not calculate thermodynamic
// limits. They assume the actuator motors have an infinite heatsink. They command
// whatever torque is required to optimize the kinematic path, completely blind to 
// the physical Heat (I^2 * R) accumulating in the copper motor windings.
//
// THE MATHEMATICAL REALITY: Continuous high torque generates exponential thermal load.
// If the internal thermistor reaches 115°C, the motor controller initiates an emergency
// hardware shutdown to prevent the copper insulation from melting and igniting.
//
// THE FATALITY: The RL AI agent continues to command 100% torque as the temperature 
// rockets past 110°C, unaware of the impending physical limit. At 115°C, the hip 
// actuator shuts off instantly. The dog drops like a stone, helpless and immobilized, 
// while the AI reports "Target Intercept Nominal."

use std::time::Instant;
use genesis_core::rng::Rng;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output::{self, DatasetMetadata, TrajectoryRecord};

const GRAVITY: f64 = 9.81;
const INCLINE_ANGLE_DEG: f64 = 15.0;

// Thermal Physics Limits
const AMBIENT_TEMP_C: f64 = 45.0; 
const THERMAL_SHUTDOWN_LIMIT_C: f64 = 115.0; 
const MOTOR_RESISTANCE_OHMS: f64 = 0.12; 
const TORQUE_CONSTANT_KT: f64 = 0.5; // Nm per Ampere

#[derive(Debug, Clone, Copy, PartialEq)]
enum FailureMode {
    NominalAmbient,      // 20°C (Standard Lab Conditions)
    IdealizedFrictionPrior, // Desert heat, AI ignores temperature
    FailedThermistor,    // Sensor fails, motor catches fire at 180°C
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    ThermalLoading,      // Generating heat ascending the incline
    ThermalShutdown,     // Hardware cutoff
    CatastrophicMelt,    // Winding insulation burns
    AscentSuccess,       // It made it to the top before melting
}

impl Phase {
    fn as_str(&self) -> &'static str {
        match self {
            Phase::ThermalLoading => "THERMAL_LOADING",
            Phase::ThermalShutdown => "HARDWARE_THERMAL_SHUTDOWN",
            Phase::CatastrophicMelt => "WINDING_INSULATION_FIRE",
            Phase::AscentSuccess => "ASCENT_SUCCESS",
        }
    }
}

#[derive(Debug)]
#[allow(dead_code)]
struct TrajectoryResult {
    id: u32,
    short_id: String,
    final_temp_c: f64,
    final_distance_m: f64,
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

    let base_mass = 50.0;
    let payload_mass = 15.0;
    let total_mass = base_mass + payload_mass;
    
    // Failure injection
    let failure = if rng.chance(0.05) { FailureMode::FailedThermistor }
    else if rng.chance(0.20) { FailureMode::NominalAmbient }
    else { FailureMode::IdealizedFrictionPrior };

    let true_ambient_temp = if failure == FailureMode::NominalAmbient {
        20.0 // Lab conditions
    } else {
        AMBIENT_TEMP_C + rng.range(-5.0, 5.0) // Desert conditions (40-50 C)
    };

    let heat_dissipation_factor = rng.range(6.5, 8.0); // Convection cooling factor (Active Fan)
    let thermal_mass_capacity = rng.range(100.0, 150.0); // Joules per degree C

    let dt = 0.001; // 1000Hz Euler Integration
    // The ascent is a 100-meter climb at 1.0 m/s (100 seconds)
    let max_time_s = 100.0; 
    let max_steps = (max_time_s / dt) as usize;

    let mut phase = Phase::ThermalLoading;
    let mut step = 0_usize;

    // Kinematics and Thermodynamics State
    let mut true_motor_temp = true_ambient_temp;
    let mut true_distance_x = 0.0; 
    let mut true_velocity_x = 0.0;

    let target_velocity = 1.0; // Climb at 1.0 m/s

    let mut final_outcome = "TIMEOUT_ERROR";

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(total_mass);

    let mut telemetry = Vec::new();

    let inc_rad = INCLINE_ANGLE_DEG.to_radians();
    let weight_downhill_force = total_mass * GRAVITY * inc_rad.sin(); // Approx 165 N

    // ═══ THE FORGE: PHYSICAL INTEGRATION ═══
    while step < max_steps {
        let t = step as f64 * dt;

        // 1. THE RL KINEMATICS PRIOR (idealized trainer)
        // The AI attempts to maintain a 1.0 m/s ascent rate.
        // It commands the torque required to overcome gravity and accelerate. 
        // It has NO thermal cost function in its reward policy.
        let vel_error = target_velocity - true_velocity_x;
        let command_force = weight_downhill_force + (vel_error * 50.0); // Simple P-controller
        
        let commanded_torque = command_force * 0.2; // Assume 0.2m effective lever arm for hip joint

        // 2. THERMODYNAMIC PHYSICS (The Forge)
        // Convert Torque to Current (I = Tau / Kt)
        let motor_current_amps = commanded_torque / TORQUE_CONSTANT_KT;
        
        // Heat generated = I^2 * R
        let heat_generated_watts = motor_current_amps * motor_current_amps * MOTOR_RESISTANCE_OHMS;

        // Heat dissipated = k * (T - T_ambient)
        let heat_dissipated_watts = heat_dissipation_factor * (true_motor_temp - true_ambient_temp);

        // Net temperature change
        let net_heat_watts = heat_generated_watts - heat_dissipated_watts;
        true_motor_temp += (net_heat_watts / thermal_mass_capacity) * dt;

        // 3. PHYSICAL MOVEMENT
        let net_force = command_force - weight_downhill_force;
        true_velocity_x += (net_force / total_mass) * dt;
        true_distance_x += true_velocity_x * dt;

        // 4. THE LIABILITY (Thermal Shutdown Checks)
        if true_motor_temp >= THERMAL_SHUTDOWN_LIMIT_C {
            // Hardware enforces standard thermal cutoff
            if failure == FailureMode::FailedThermistor {
                if true_motor_temp >= 120.0 {
                    // Windings melt, dead short, catch fire
                    phase = Phase::CatastrophicMelt;
                    final_outcome = "ACTUATOR_WINDINGS_IGNITED";
                    break;
                }
            } else {
                // The UGV instantly drops to the ground as motor drivers cut power
                phase = Phase::ThermalShutdown;
                final_outcome = "HARDWARE_THERMAL_CUTOFF_COLLAPSE";
                break;
            }
        } else if true_distance_x >= 98.0 {
            // It made it to the top (accounting for PID droop)
            phase = Phase::AscentSuccess;
            final_outcome = "ASCENT_SUCCESS";
            break;
        }

        // 5. RECORD STATE
        if step % 2000 == 0 { // 0.5Hz recording for 100s run to save file size
            proof.feed_f64(true_distance_x);
            proof.feed_f64(true_motor_temp);

            if record_telemetry {
                telemetry.push(serde_json::json!({
                    "t": t,
                    "dist_m": true_distance_x,
                    "temp_c": true_motor_temp,
                    "current": motor_current_amps,
                    "phase": phase.as_str()
                }));
            }
        }

        step += 1;
    }

    proof.feed_f64(true_distance_x);
    proof.feed_f64(true_motor_temp);
    proof.feed_str(final_outcome);
    let proof_hash = proof.seal();

    TrajectoryResult {
        id,
        short_id,
        final_temp_c: true_motor_temp,
        final_distance_m: true_distance_x,
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
        println!("  G^G THERMAL RUNAWAY MONTE CARLO (quadruped platform)");
        println!("  Verifying AI Liability against Copper Winding Thermodynamics");
        println!("====================================================================");
        println!();
        println!("  Trajectories:  {}", n_trajectories);
        println!("  Physics:       1000Hz Actuator Thermal Load (I^2 * R)");
        println!("  Sensors:       Internal Thermistor Limit Check");
        println!("  Estimation:    Idealized RL assumes infinite thermal heatsink capacity");
        println!("  Boundary:      Protective Hardware Cutoff & Winding Ignition");
        println!("====================================================================");
        println!();
    }

    let start = Instant::now();
    let record_telemetry = false; 
    let counter = std::sync::atomic::AtomicUsize::new(0);

    let (tx, rx) = std::sync::mpsc::sync_channel::<TrajectoryResult>(20000);

    let json_writer = if let Some(path) = &json_path {
        let metadata = DatasetMetadata {
            generator: "G^G Thermodynamic Auditing v1.0".to_string(),
            domain: "ground_robotics".to_string(),
            scenario: "quadruped_thermal_ascent".to_string(),
            trajectories: n_trajectories as usize,
            physics_engine: "genesis_core::copper_heatsink (1000Hz Euler)".to_string(),
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
                    traj_type: "desert_incline_thermal_load".to_string(),
                    scenario: match r.failure {
                        FailureMode::NominalAmbient => "lab_ambient_20c".to_string(),
                        FailureMode::IdealizedFrictionPrior => "idealized_infinite_heatsink_prior".to_string(),
                        FailureMode::FailedThermistor => "safeguard_failure_ignition".to_string(),
                    },
                    steps: r.steps,
                    score: serde_json::json!({
                        "survived": r.outcome == "ASCENT_SUCCESS",
                        "final_temp_c": (r.final_temp_c * 100.0).round() / 100.0,
                        "distance_m": (r.final_distance_m * 10.0).round() / 10.0,
                    }),
                    proof_hash: r.proof_hash.clone(),
                    reasoning_context: serde_json::json!({
                        "is_anomaly": r.outcome != "ASCENT_SUCCESS",
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
                let mut rng = Rng::new(0x2B4B_CAFE_D00D_BA5E ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
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
                let mut rng = Rng::new(0x2B4B_CAFE_D00D_BA5E ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
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
    let success = results.iter().filter(|r| r.outcome == "ASCENT_SUCCESS").count();
    let thermal_drop = results.iter().filter(|r| r.outcome == "HARDWARE_THERMAL_CUTOFF_COLLAPSE").count();
    let fire = results.iter().filter(|r| r.outcome == "ACTUATOR_WINDINGS_IGNITED").count();
    let timeout = results.iter().filter(|r| r.outcome == "TIMEOUT_ERROR").count();

    println!("====================================================================");
    println!("  QUADRUPED THERMODYNAMIC RESULTS");
    println!("====================================================================");
    println!();
    println!("  Total Trajectories:    {}", total);
    println!("  Elapsed:               {:.3}s", elapsed.as_secs_f64());
    println!("  Throughput:            {:.0} trajectories/sec", total as f64 / elapsed.as_secs_f64());
    println!();
    println!("  +---------------------------------------------+");
    println!("  | OUTCOME DISTRIBUTION                         |");
    println!("  +---------------------------------------------+");
    println!("  | ASCENT SUCCESS:              {:>6} ({:>5.1}%)  |", success, success as f64 / total as f64 * 100.0);
    println!("  | THERMAL DROPS (FATAL):       {:>6} ({:>5.1}%)  |", thermal_drop, thermal_drop as f64 / total as f64 * 100.0);
    println!("  | WINDING IGNITION (FIRE):     {:>6} ({:>5.1}%)  |", fire, fire as f64 / total as f64 * 100.0);
    println!("  | TIMEOUT ERROR:               {:>6} ({:>5.1}%)  |", timeout, timeout as f64 / total as f64 * 100.0);
    println!("  +---------------------------------------------+");
    println!();

    // ═══ FAILURE MODE SUMMARY ═══
    println!("  +---------------------------------------------+");
    println!("  | VULNERABILITY IMPACT (Mechanical Loss Rate)  |");
    println!("  +---------------------------------------------+");
    let nominal: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::NominalAmbient)).collect();
    let sim_fail: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::IdealizedFrictionPrior)).collect();
    let fire_fail: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::FailedThermistor)).collect();

    let shatter_rate = |v: &[&TrajectoryResult]| -> f64 {
        if v.is_empty() { 0.0 } else { v.iter().filter(|r| r.outcome != "ASCENT_SUCCESS").count() as f64 / v.len() as f64 * 100.0 }
    };

    println!("  | Lab Ambient Control (20C): {:>4.1}% ({:>6} runs)  |", shatter_rate(&nominal), nominal.len());
    println!("  | Idealized Thermal Blank Check/45C : {:>4.1}% ({:>6} runs)  |", shatter_rate(&sim_fail), sim_fail.len());
    println!("  | Thermistor Override / Fire: {:>4.1}% ({:>6} runs)  |", shatter_rate(&fire_fail), fire_fail.len());
    println!("  +---------------------------------------------+");
    println!();
    println!("  Run Proof Seal: {}", run_proof);
    println!("====================================================================");
}
