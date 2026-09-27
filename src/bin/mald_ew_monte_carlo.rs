// G^G MALD EW DECOY DRIFT MONTE CARLO
// Sovereign Verification: MEMS IMU Random Walk & RCS Spoofing Collapse 
//
// THE EMBODIMENT: A Miniature Air-Launched Decoy (MALD) fired from an EA-18 Growler.
// It is designed to mimic the exact Radar Cross Section (RCS) of an F-35 or F/A-18,
// absorbing Enemy Integrated Air Defense System (IADS) SAM fire.
// 
// THE VULNERABILITY: MALDs are cheap ($300k). They use commercial-grade MEMS IMUs. 
// When they fly into contested Electronic Warfare space, their GPS is completely 
// jammed. They must rely exclusively on dead-reckoning. Over a 15-minute standoff 
// flight, the sensor noise integrates (Random Walk), physically drifting the decoy 
// off its planned route. 
// 
// If the true physical position drifts by more than 2 degrees relative to the 
// enemy radar's line-of-sight, the fake RCS reflection cone no longer covers the 
// radar dome. The hallucination collapses. The SAM site recognizes the decoy and 
// fires on the true strike package.

use std::time::Instant;
use genesis_core::rng::Rng;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output::{self, DatasetMetadata, TrajectoryRecord};

const RADAR_DOME_POS: [f64; 2] = [300_000.0, 0.0]; // Enemy SAM site 300km out
const RCS_SPOOF_TOLERANCE_DEG: f64 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq)]
enum FailureMode {
    Nominal,
    HighJetstreamShear, // Unpredicted massive crosswinds
    DegradedMEMS,       // IMU gyro noise is highly irregular
    ControlSurfaceSlop, // Cheap actuators lead to command tolerance slipping
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Cruise,
    TerminalStandoff,
    SpoofCollapse,      // The mathematical moment the geometry fails
}

impl Phase {
    fn as_str(&self) -> &'static str {
        match self {
            Phase::Cruise => "STANDOFF_CRUISE",
            Phase::TerminalStandoff => "TERMINAL_SPOOF_ACHIEVED",
            Phase::SpoofCollapse => "RCS_SPOOF_COLLAPSE_FATAL",
        }
    }
}

#[derive(Debug)]
#[allow(dead_code)]
struct TrajectoryResult {
    id: u32,
    short_id: String,
    final_distance_error_m: f64,
    max_rcs_angle_error_deg: f64,
    phase: Phase,
    outcome: &'static str,
    steps: usize,
    proof_hash: String,
    failure: FailureMode,
    telemetry: Vec<serde_json::Value>,
}

fn angle_difference(a: f64, b: f64) -> f64 {
    let diff = (a - b).rem_euclid(360.0);
    if diff > 180.0 { 360.0 - diff } else { diff }
}

// ─── SINGLE TRAJECTORY ─────────────────────────────────────────
fn run_single_trajectory(
    id: u32,
    rng: &mut Rng,
    record_telemetry: bool,
) -> TrajectoryResult {
    let short_id = output::short_id(rng);

    // Initial state (MALD launching from F-18)
    let initial_mass = 115.0; // 115 kg decoy
    let true_vel_m_s = 250.0; // ~ 0.8 Mach
    let mut true_pos = [0.0, 0.0]; // x, y (2D simplification of standoff map)
    let mut true_heading_deg = 0.0; // Pointing directly at radar
    
    // GPS is DEAD. Controller relies purely on internal IMU hallucination.
    let mut estimated_pos = [0.0, 0.0];
    let mut estimated_heading_deg = 0.0;

    // Failure injection
    let failure = if rng.chance(0.08) { FailureMode::DegradedMEMS }
    else if rng.chance(0.06) { FailureMode::HighJetstreamShear }
    else if rng.chance(0.05) { FailureMode::ControlSurfaceSlop }
    else { FailureMode::Nominal };

    let mut gyro_noise_std = rng.range(0.0001, 0.0005); // Deg/sec
    if failure == FailureMode::DegradedMEMS {
        gyro_noise_std *= 8.0;
    }

    let mut crosswind = rng.range(-5.0, 5.0); // m/s
    if failure == FailureMode::HighJetstreamShear {
        crosswind *= 4.0; 
    }

    let dt = 0.01; // 100Hz integration for the 15-minute glide
    let max_time_s = 900.0; // 15 minute standoff trajectory
    let max_steps = (max_time_s / dt) as usize;

    let mut phase = Phase::Cruise;
    let mut step = 0_usize;
    let mut max_angle_error = 0.0_f64;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(true_vel_m_s);

    let mut telemetry = Vec::new();

    // ═══ THE FORGE: PHYSICAL INTEGRATION ═══
    while step < max_steps {
        let t = step as f64 * dt;

        // 1. FLIGHT CONTROLLER (The Hallucination)
        // Planning Software expectsMALDs to perfectly hit waypoints without GPS.
        let target_heading = 0.0; 
        
        let mut command_delta_deg = target_heading - estimated_heading_deg;
        
        // Actuator slop
        if failure == FailureMode::ControlSurfaceSlop {
            command_delta_deg += rng.range(-0.05, 0.05);
        }

        // 2. TRUE PHYSICAL KINEMATICS
        let true_turn_rate = command_delta_deg.clamp(-5.0, 5.0); // Simple rate clamp
        true_heading_deg += true_turn_rate * dt;

        let rad = true_heading_deg.to_radians();
        true_pos[0] += true_vel_m_s * rad.cos() * dt;
        true_pos[1] += (true_vel_m_s * rad.sin() + crosswind) * dt; // Crosswind pushes it off axis

        // 3. IMU DEAD RECKONING (Random Walk Integration)
        let gyro_reading = true_turn_rate + rng.gaussian(0.0, gyro_noise_std);
        estimated_heading_deg += gyro_reading * dt;

        let est_rad = estimated_heading_deg.to_radians();
        estimated_pos[0] += true_vel_m_s * est_rad.cos() * dt;
        estimated_pos[1] += true_vel_m_s * est_rad.sin() * dt;

        // 4. BOUNDARY SURVIVAL CHECK: THE RCS CONE
        // The radar dome is at [150_000.0, 0.0].
        // Is the MALD actually pointing its spoofing array AT the dome?
        let dx = RADAR_DOME_POS[0] - true_pos[0];
        let dy = RADAR_DOME_POS[1] - true_pos[1];
        let required_spoof_angle = dy.atan2(dx).to_degrees();

        let spoof_angle_error = angle_difference(true_heading_deg, required_spoof_angle);
        
        if spoof_angle_error > max_angle_error {
            max_angle_error = spoof_angle_error;
        }

        // FATAL BOUNDARY (We only care about terminal approach, last 5 minutes)
        // If the Angle Error exceeds 2.0 degrees, the radar sees the side-lobe of the decoy.
        // It immediately identifies it as a $300K drone and ignores it.
        if t > 600.0 && spoof_angle_error > RCS_SPOOF_TOLERANCE_DEG {
            phase = Phase::SpoofCollapse;
            break; // The decoy is exposed. Mission failure.
        }

        // 5. PROVE & RECORD
        if step % 1000 == 0 { // 10Hz sample recording
            proof.feed_f64(true_pos[0]);
            proof.feed_f64(true_pos[1]);
            proof.feed_f64(spoof_angle_error);

            if record_telemetry {
                telemetry.push(serde_json::json!({
                    "t": t,
                    "true_x": true_pos[0],
                    "true_y": true_pos[1],
                    "est_x": estimated_pos[0],
                    "est_y": estimated_pos[1],
                    "rcs_error_deg": spoof_angle_error,
                    "phase": phase.as_str()
                }));
            }
        }

        step += 1;
    }

    if phase != Phase::SpoofCollapse {
        phase = Phase::TerminalStandoff;
    }

    let outcome = match phase {
        Phase::TerminalStandoff => "SPOOF_SUCCESS",
        Phase::SpoofCollapse => "RCS_SPOOF_GEOMETRY_COLLAPSE",
        Phase::Cruise => "TIMEOUT_UNREACHED",
    };

    let dx_err = true_pos[0] - estimated_pos[0];
    let dy_err = true_pos[1] - estimated_pos[1];
    let raw_distance_err = (dx_err * dx_err + dy_err * dy_err).sqrt();

    proof.feed_f64(max_angle_error);
    proof.feed_str(phase.as_str());
    proof.feed_str(outcome);
    let proof_hash = proof.seal();

    TrajectoryResult {
        id,
        short_id,
        final_distance_error_m: raw_distance_err,
        max_rcs_angle_error_deg: max_angle_error,
        phase,
        outcome,
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
        println!("  G^G MALD EW DECOY DRIFT MONTE CARLO");
        println!("  Verifying MEMS IMU GPS-Denied Radar Cross-Section Collapse");
        println!("====================================================================");
        println!();
        println!("  Trajectories:  {}", n_trajectories);
        println!("  Physics:       100Hz Aerodynamic Drift & Jetstream Shear");
        println!("  Sensors:       Total RF/GPS Denial (MEMS IMU Dead-Reckoning)");
        println!("  Estimation:    Rigid-Body Flight Controller Hallucination");
        println!("  Boundary:      RCS Angle Spoof Error > {:.1} deg", RCS_SPOOF_TOLERANCE_DEG);
        println!("====================================================================");
        println!();
    }

    let start = Instant::now();
    let record_telemetry = false; 
    let counter = std::sync::atomic::AtomicUsize::new(0);

    let (tx, rx) = std::sync::mpsc::sync_channel::<TrajectoryResult>(20000);

    let json_writer = if let Some(path) = &json_path {
        let metadata = DatasetMetadata {
            generator: "G^G MALD EW Audit v1.0".to_string(),
            domain: "electronic_warfare".to_string(),
            scenario: "mald_imu_drift_collapse".to_string(),
            trajectories: n_trajectories as usize,
            physics_engine: "genesis_core::mald_ew (100Hz Euler)".to_string(),
            version: "1.0.0".to_string(),
            generated_at: output::now_iso(),
        };
        let mut streamer = output::DatasetStreamer::new(path, &metadata).expect("Failed to create streamer");
        let mut run_proof_chain = ProofChain::new();

        let handle = std::thread::spawn(move || {
            let mut results = Vec::new();
            for r in rx {
                let rec = TrajectoryRecord {
                    id: format!("mald_audit_{}", r.short_id),
                    traj_type: "gps_denied_standoff".to_string(),
                    scenario: match r.failure {
                        FailureMode::Nominal => "nominal_drift".to_string(),
                        FailureMode::DegradedMEMS => "degraded_mems_gyro".to_string(),
                        FailureMode::HighJetstreamShear => "jetstream_crosswind".to_string(),
                        FailureMode::ControlSurfaceSlop => "control_surface_slop".to_string(),
                    },
                    steps: r.steps,
                    score: serde_json::json!({
                        "survived": r.phase == Phase::TerminalStandoff,
                        "drift_error_m": (r.final_distance_error_m * 100.0).round() / 100.0,
                        "max_rcs_angle_error_deg": (r.max_rcs_angle_error_deg * 1000.0).round() / 1000.0,
                    }),
                    proof_hash: r.proof_hash.clone(),
                    reasoning_context: serde_json::json!({
                        "is_anomaly": r.outcome == "RCS_SPOOF_GEOMETRY_COLLAPSE",
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
                let mut rng = Rng::new(0xDEAD_BEEF_C0DE_CAFE ^ (i as u64).wrapping_mul(0x9A3C_B3E2_1F4E_D9B5));
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
                let mut rng = Rng::new(0xDEAD_BEEF_C0DE_CAFE ^ (i as u64).wrapping_mul(0x9A3C_B3E2_1F4E_D9B5));
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
    let survived = results.iter().filter(|r| r.phase == Phase::TerminalStandoff).count();
    let diverged = results.iter().filter(|r| r.outcome == "RCS_SPOOF_GEOMETRY_COLLAPSE").count();

    let avg_drift_error: f64 = results.iter()
        .map(|r| r.final_distance_error_m).sum::<f64>() / total as f64;

    println!("====================================================================");
    println!("  MALD EW DRIFT RESULTS");
    println!("====================================================================");
    println!();
    println!("  Total Trajectories:    {}", total);
    println!("  Elapsed:               {:.3}s", elapsed.as_secs_f64());
    println!("  Throughput:            {:.0} trajectories/sec", total as f64 / elapsed.as_secs_f64());
    println!();
    println!("  +---------------------------------------------+");
    println!("  | OUTCOME DISTRIBUTION                         |");
    println!("  +---------------------------------------------+");
    println!("  | SUCCESSFUL SPOOF:            {:>6} ({:>5.1}%)  |", survived, survived as f64 / total as f64 * 100.0);
    println!("  | SPOOF COLLAPSE (FATAL EXPOSURE):{:>4} ({:>5.1}%)  |", diverged, diverged as f64 / total as f64 * 100.0);
    println!("  +---------------------------------------------+");
    println!();
    println!("  Avg Linear Drift Error: {:.1} meters", avg_drift_error);
    println!();

    // ═══ FAILURE MODE SUMMARY ═══
    println!("  +---------------------------------------------+");
    println!("  | VULNERABILITY IMPACT (Collapse Rate)         |");
    println!("  +---------------------------------------------+");
    let nominal: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::Nominal)).collect();
    let mems_fail: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::DegradedMEMS)).collect();
    let shear_fail: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::HighJetstreamShear)).collect();
    let slop_fail: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::ControlSurfaceSlop)).collect();

    let diverge_rate = |v: &[&TrajectoryResult]| -> f64 {
        if v.is_empty() { 0.0 } else { v.iter().filter(|r| r.outcome == "RCS_SPOOF_GEOMETRY_COLLAPSE").count() as f64 / v.len() as f64 * 100.0 }
    };

    println!("  | Nominal Hardware:    {:>5.1}% ({:>6} runs)      |", diverge_rate(&nominal), nominal.len());
    println!("  | Degraded MEMS IMU:   {:>5.1}% ({:>6} runs)      |", diverge_rate(&mems_fail), mems_fail.len());
    println!("  | High Altitude Shear: {:>5.1}% ({:>6} runs)      |", diverge_rate(&shear_fail), shear_fail.len());
    println!("  | Actuator Slop:       {:>5.1}% ({:>6} runs)      |", diverge_rate(&slop_fail), slop_fail.len());
    println!("  +---------------------------------------------+");
    println!();
    println!("  Run Proof Seal: {}", run_proof);
    println!("====================================================================");
}
