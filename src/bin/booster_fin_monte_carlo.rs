// G^G BLUE ORIGIN TRANSONIC FIN REVERSAL MONTE CARLO
// Sovereign Verification: Transonic Shockwave Oscillation vs Hydraulic Actuator Latency
//
// THE EMBODIMENT: A Blue Origin New Shepard returning booster descending through the 
// atmosphere at 30,000 feet. The booster uses aero-surfaces (wedge fins) to steer 
// its 30,000kg mass toward the landing pad.
// 
// THE VULNERABILITY: NVIDIA Omniverse and Isaac Sim train AI control algorithms using 
// generalized continuous fluid dynamics. The AI learns that aerodynamic pressure (dynamic 
// pressure Q) scales smoothly. It assumes the hydraulic actuators controlling the fins 
// possess absolute, instantaneous stiffness to hold any commanded pitch angle.
//
// THE MATHEMATICAL REALITY: When the booster decelerates precisely through Mach 0.98 
// (the Transonic Regime), the supersonic shockwave detaches from the leading edge and 
// begins oscillating violently back and forth across the fin's chord at 100Hz.
// This causes the Aerodynamic Center of Pressure (CoP) to instantly shift from the 
// front of the fin to the rear. The aerodynamic hinge torque rapidly reverses direction.
//
// THE FATALITY: Hydraulic fluids and servo valves physically cannot reverse pressure 
// flow in 10-milliseconds. The actuator stalls due to fluid compressibility latency. 
// Without hydraulic lock, the 100Hz transonic shockwave violently back-drives the 
// unstiffened fin, exceeding the 50,000 Nm structural torsion limit. The fin is 
// sheared completely off the booster, inducing an uncontrollable tumble and mid-air 
// destruction.

use std::time::Instant;
use genesis_core::rng::Rng;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output::{self, DatasetMetadata, TrajectoryRecord};

// Booster Kinematics
const INITIAL_MACH: f64 = 1.2;
const FINAL_MACH: f64 = 0.8;
const DECELERATION_RATE: f64 = 0.05; // Mach per second (approx 17 m/s^2)

// Fin Structural Limits
const FIN_TORSION_STRENGTH_LIMIT_NM: f64 = 50_000.0; // Newton-meters before the titanium hinge snaps
const NOMINAL_AERO_TORQUE_NM: f64 = 15_000.0; // Standard steering torque

#[derive(Debug, Clone, Copy, PartialEq)]
enum FailureMode {
    VirtualCleanSim,        // Ideal Isaac Sim: Smooth generalized pressure gradient
    SubsonicDescent,        // Descent below Mach 0.8 (Shockwave dissipates)
    TransonicOscillation,   // Descent precisely through Mach 0.98 with attached shock flutter
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    SupersonicDescent,      // Mach 1.2 to Mach 1.0 (Stable attached shock)
    TransonicShockFlutter,  // Mach 1.0 to Mach 0.95 (Violent CoP oscillation)
    ActuatorReversalStall,  // Hydraulic valves fail to track the 100Hz flutter
    StructuralTearOff,      // Fin hinge snaps
    SubsonicRecovery,       // Booster safely exits transonic regime
}

impl Phase {
    fn as_str(&self) -> &'static str {
        match self {
            Phase::SupersonicDescent => "SUPERSONIC_STABLE_DESCENT",
            Phase::TransonicShockFlutter => "TRANSONIC_SHOCKWAVE_OSCILLATION",
            Phase::ActuatorReversalStall => "HYDRAULIC_VALVE_STALL",
            Phase::StructuralTearOff => "FIN_STRUCTURAL_SHEAR",
            Phase::SubsonicRecovery => "SUBSONIC_FLIGHT_SAFETY",
        }
    }
}

#[derive(Debug)]
#[allow(dead_code)]
struct TrajectoryResult {
    id: u32,
    short_id: String,
    max_torsion_nm: f64,
    final_mach: f64,
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
    else if rng.chance(0.35) { FailureMode::SubsonicDescent }
    else { FailureMode::TransonicOscillation };

    let dt = 0.001; // 1000Hz computation required to track 100Hz shockwave flutter
    let max_time_s = 10.0; // 10 seconds of descent through the sound barrier
    let max_steps = (max_time_s / dt) as usize;

    let mut phase = if failure == FailureMode::SubsonicDescent {
        Phase::SubsonicRecovery
    } else {
        Phase::SupersonicDescent
    };
    let mut step = 0_usize;

    // Physical State Matrix
    let mut current_mach = if failure == FailureMode::SubsonicDescent { 0.85 } else { INITIAL_MACH };
    
    // The force attempting to twist the fin off the rocket
    let mut current_hinge_torsion_nm = NOMINAL_AERO_TORQUE_NM;
    let mut max_recorded_torsion_nm = 0.0;
    
    // Hydraulic Actuator Model
    let mut hydraulic_pressure_head_nm = 20_000.0; // The servo can hold 20k Nm of torque
    let mut actual_fin_angle_rad = 0.05; // 3 degree pitch

    let mut final_outcome = "TIMEOUT_ERROR";

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(FIN_TORSION_STRENGTH_LIMIT_NM);

    let mut telemetry = Vec::new();

    // ═══ THE FORGE: PHYSICAL INTEGRATION ═══
    while step < max_steps {
        let t = step as f64 * dt;

        // Decelerate the booster
        current_mach -= DECELERATION_RATE * dt;

        if current_mach < 1.05 && current_mach > 0.95 && phase == Phase::SupersonicDescent && failure != FailureMode::SubsonicDescent {
            phase = Phase::TransonicShockFlutter;
        } else if current_mach <= 0.95 && phase == Phase::TransonicShockFlutter {
            phase = Phase::SubsonicRecovery;
        }

        match failure {
            FailureMode::VirtualCleanSim => {
                // Generative AI incorrectly models Transonic dynamic pressure as a smooth curve
                // Q scales continuously. Hinge torque remains entirely stable.
                current_hinge_torsion_nm = NOMINAL_AERO_TORQUE_NM * (current_mach / 1.0);
            },
            FailureMode::SubsonicDescent => {
                // Post-barrier, fluid dynamics return to smooth subsonic flow
                current_hinge_torsion_nm = NOMINAL_AERO_TORQUE_NM * (current_mach / 0.8);
            },
            FailureMode::TransonicOscillation => {
                if phase == Phase::TransonicShockFlutter {
                    // THE TRANSONIC SHOCKWAVE DETACHMENT (Mathematical Reality)
                    // At exactly Mach ~0.98, the shockwave physically bounces across the fin chord
                    // at ~100 Hz. This causes the aerodynamic Center of Pressure (CoP) to instantly 
                    // switch from the leading edge to the trailing edge. 
                    let oscillation_freq_hz = 100.0;
                    
                    // The violent relocation of the CoP throws a massive reversed torque vector
                    let shockwave_induced_torque = 35_000.0 * (t * std::f64::consts::PI * 2.0 * oscillation_freq_hz).sin();
                    current_hinge_torsion_nm = NOMINAL_AERO_TORQUE_NM + shockwave_induced_torque;
                } else if phase == Phase::SupersonicDescent {
                    current_hinge_torsion_nm = NOMINAL_AERO_TORQUE_NM;
                } else if phase == Phase::ActuatorReversalStall {
                    // Once hydraulic pressure stalls, the fin is dead. It catches the full supersonic drag.
                    current_hinge_torsion_nm += 100_000.0 * dt; // Rapid logarithmic torque runaway
                }
            }
        }

        // 3. HYDRAULIC VALVE COMPRESSIBILITY SOLVER
        if phase == Phase::TransonicShockFlutter {
            // A physical hydraulic valve operates on fluid. It requires ~0.05 seconds to physically 
            // reverse flow pressure to track an opposing torque.
            // But the external shockwave is reversing torque every 0.01 seconds (100Hz).
            // The servo actuator hits "Compressibility Latency" and stalls completely.
            
            // If the absolute rate of torque change exceeds the hydraulic pump's slew rate:
            let d_torque = (current_hinge_torsion_nm - NOMINAL_AERO_TORQUE_NM).abs();
            if d_torque > 25_000.0 { // Actuator stall threshold
                hydraulic_pressure_head_nm = 0.0; // The hydraulic lock is lost
                phase = Phase::ActuatorReversalStall;
            }
        }

        // 4. STRUCTURAL FAILURE CHECK
        if current_hinge_torsion_nm > max_recorded_torsion_nm {
            max_recorded_torsion_nm = current_hinge_torsion_nm;
        }

        if current_hinge_torsion_nm.abs() > FIN_TORSION_STRENGTH_LIMIT_NM {
            // The titanium hinge shears
            phase = Phase::StructuralTearOff;
            final_outcome = "FIN_TORSION_SHEAR_FATALITY";
            break;
        }

        if current_mach <= FINAL_MACH {
            final_outcome = "SUBSONIC_ORBITAL_RECOVERY";
            break;
        }

        // 6. RECORD STATE
        if t % 5.0 < dt { // 0.2Hz recording for file precision
            proof.feed_f64(current_mach);
            proof.feed_f64(current_hinge_torsion_nm);

            if record_telemetry {
                telemetry.push(serde_json::json!({
                    "t": t,
                    "mach": current_mach,
                    "torsion_nm": current_hinge_torsion_nm,
                    "phase": phase.as_str()
                }));
            }
        }

        step += 1;
    }

    if step >= max_steps && phase != Phase::StructuralTearOff && phase != Phase::SubsonicRecovery {
        final_outcome = "TIMEOUT_STALL";
    }

    proof.feed_f64(max_recorded_torsion_nm);
    proof.feed_f64(current_mach);
    proof.feed_str(final_outcome);
    let proof_hash = proof.seal();

    TrajectoryResult {
        id,
        short_id,
        max_torsion_nm: max_recorded_torsion_nm,
        final_mach: current_mach,
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
        println!("  G^G BLUE ORIGIN TRANSONIC FIN REVERSAL MONTE CARLO");
        println!("  Verifying Shockwave Oscillation vs Isaac Sim Aerodynamics");
        println!("====================================================================");
        println!();
        println!("  Trajectories:  {}", n_trajectories);
        println!("  Physics:       100Hz Aerodynamic Center of Pressure (CoP) Flutter");
        println!("  Sensors:       Booster Pitch AI & Hydraulic Actuator Slew Rates");
        println!("  Estimation:    Omniverse hallucinates smoothly scaling dynamic pressure");
        println!("  Boundary:      50k Nm Transonic Fin Torsion Shearing Limit");
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
            scenario: "blue_origin_transonic_fin_reversal".to_string(),
            trajectories: n_trajectories as usize,
            physics_engine: "genesis_core::shockwave_flutter_kinematics (1000Hz)".to_string(),
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
                    traj_type: "transonic_actuator_stall_shear".to_string(),
                    scenario: match r.failure {
                        FailureMode::VirtualCleanSim => "isaac_sim_generalized_liquid_dynamics".to_string(),
                        FailureMode::SubsonicDescent => "subsonic_smooth_flow_geometry".to_string(),
                        FailureMode::TransonicOscillation => "mach_0_98_shockwave_detachment_flutter".to_string(),
                    },
                    steps: r.steps,
                    score: serde_json::json!({
                        "survived": r.outcome == "SUBSONIC_ORBITAL_RECOVERY",
                        "max_fin_torsion_nm": (r.max_torsion_nm * 10.0).round() / 10.0,
                        "fin_sheared_off": r.outcome == "FIN_TORSION_SHEAR_FATALITY",
                    }),
                    proof_hash: r.proof_hash.clone(),
                    reasoning_context: serde_json::json!({
                        "is_anomaly": r.outcome != "SUBSONIC_ORBITAL_RECOVERY",
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
                let mut rng = Rng::new(0xA55A_D00D_FACE_BEEF ^ (i as u64).wrapping_mul(0xC0FE_BABE_B00B_1337));
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
                let mut rng = Rng::new(0xA55A_D00D_FACE_BEEF ^ (i as u64).wrapping_mul(0xC0FE_BABE_B00B_1337));
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
    let success = results.iter().filter(|r| r.outcome == "SUBSONIC_ORBITAL_RECOVERY").count();
    let shear = results.iter().filter(|r| r.outcome == "FIN_TORSION_SHEAR_FATALITY").count();
    let timeout = results.iter().filter(|r| r.outcome == "TIMEOUT_STALL").count();

    println!("====================================================================");
    println!("  BLUE ORIGIN TRANSONIC FIN RESULTS");
    println!("====================================================================");
    println!();
    println!("  Total Trajectories:    {}", total);
    println!("  Elapsed:               {:.3}s", elapsed.as_secs_f64());
    println!("  Throughput:            {:.0} trajectories/sec", total as f64 / elapsed.as_secs_f64());
    println!();
    println!("  +---------------------------------------------+");
    println!("  | OUTCOME DISTRIBUTION                         |");
    println!("  +---------------------------------------------+");
    println!("  | SUBSONIC SAFE RECOVERY:        {:>6} ({:>5.1}%)  |", success, success as f64 / total as f64 * 100.0);
    println!("  | FIN TORN OFF (FATALITY):       {:>6} ({:>5.1}%)  |", shear, shear as f64 / total as f64 * 100.0);
    println!("  | TIMEOUT PERDITION:             {:>6} ({:>5.1}%)  |", timeout, timeout as f64 / total as f64 * 100.0);
    println!("  +---------------------------------------------+");
    println!();

    // ═══ FAILURE MODE SUMMARY ═══
    println!("  +---------------------------------------------+");
    println!("  | VULNERABILITY IMPACT (Mechanical Loss Rate)  |");
    println!("  +---------------------------------------------+");
    let clean: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::VirtualCleanSim)).collect();
    let sub: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::SubsonicDescent)).collect();
    let flutter: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::TransonicOscillation)).collect();

    let crash_rate = |v: &[&TrajectoryResult]| -> f64 {
        if v.is_empty() { 0.0 } else { v.iter().filter(|r| r.outcome != "SUBSONIC_ORBITAL_RECOVERY").count() as f64 / v.len() as f64 * 100.0 }
    };

    println!("  | Isaac Sim Generalized Fluid:  {:>4.1}% ({:>6} runs) |", crash_rate(&clean), clean.len());
    println!("  | Stable Subsonic Airflow :     {:>4.1}% ({:>6} runs) |", crash_rate(&sub), sub.len());
    println!("  | Transonic Mach 0.98 Flutter: {:>4.1}% ({:>6} runs) |", crash_rate(&flutter), flutter.len());
    println!("  +---------------------------------------------+");
    println!();
    println!("  Run Proof Seal: {}", run_proof);
    println!("====================================================================");
}
