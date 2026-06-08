// G^G CARRIER OPS PLM MONTE CARLO (MAGIC CARPET)
// Sovereign Verification: EKF Deck Height Hallucination vs. Sea State 5 Heave
//
// THE EMBODIMENT: An F/A-18 Super Hornet or F-35C attempting a Precision Landing 
// Mode (PLM / "Magic Carpet") recovery on a Nuclear Aircraft Carrier (CVN).
// 
// THE VULNERABILITY: Sea State 5 causes the 100,000-ton aircraft carrier to violently
// pitch and heave. The fantail (rear of the flight deck) can swing vertically by 15 feet
// in seconds. In GPS-denied, zero-visibility environments (Instrument Meteorological 
// Conditions), the PLM software relies entirely on its Extended Kalman Filter (EKF) to 
// fuse IMU data. 
//
// If the EKF's structural model operates at a generic 60Hz and hallucinates the 
// aircraft's sink rate by even 0.5 ft/sec due to transonic deceleration vibration, 
// the aircraft's 1000Hz physical reality will intersect the deck's heaving steel. 
// The aircraft does not catch the 3-wire; it obliterates against the fantail (A Ramp Strike).

use std::time::Instant;
use std::f64::consts::PI;
use genesis_core::rng::Rng;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output::{self, DatasetMetadata, TrajectoryRecord};

const CARRIER_VELOCITY_KTS: f64 = 30.0;
const KTS_TO_MS: f64 = 0.514444;
const GRAVITY: f64 = 9.81;

const DECK_LENGTH_M: f64 = 332.0;
const TARGET_WIRE_X_M: f64 = 85.0; // The 3-wire is 85 meters from the fantail (ramp)
const GLIDESLOPE_DEG: f64 = 3.5;

#[derive(Debug, Clone, Copy, PartialEq)]
enum FailureMode {
    Nominal,
    HighSeaStateHeave,   // Extreme vertical deck unpredictability
    EkfSinkRateDrift,    // Vibration causes IMU to miscalculate vertical decelleration
    TurbulentBurble,     // CVN island creates a wake vortex right at the ramp
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Approach,
    InTheGroove,         // Last 15 seconds
    DeckCollision,       // Target intercepted
}

impl Phase {
    fn as_str(&self) -> &'static str {
        match self {
            Phase::Approach => "APPROACH",
            Phase::InTheGroove => "IN_THE_GROOVE",
            Phase::DeckCollision => "DECK_COLLISION",
        }
    }
}

#[derive(Debug)]
#[allow(dead_code)]
struct TrajectoryResult {
    id: u32,
    short_id: String,
    final_touchdown_x: f64,
    final_relative_velocity_z: f64,
    phase: Phase,
    outcome: &'static str,
    steps: usize,
    proof_hash: String,
    failure: FailureMode,
    telemetry: Vec<serde_json::Value>,
}

// ─── THE EKF PLM OBSERVER ───
// Calculates desired flight path, but hallucinating the true sink rate and position.
struct PlmObserver {
    est_z: f64,
    est_vz: f64,
    ekf_covariance: f64,
}

impl PlmObserver {
    fn new(initial_z: f64) -> Self {
        PlmObserver {
            est_z: initial_z,
            est_vz: 0.0,
            ekf_covariance: 0.05,
        }
    }

    fn predict_and_update(&mut self, dt: f64, measured_vz: f64) {
        // Random walk corruption of the EKF covariance matrices over time
        self.ekf_covariance += 0.0001 * dt; 
        
        self.est_z += self.est_vz * dt;
        
        let innovation = measured_vz - self.est_vz;
        let kalman_gain = 0.8 / (1.0 + self.ekf_covariance); // Drops trust as uncertainty rises
        
        self.est_vz += kalman_gain * innovation;
    }
}

// ─── SINGLE TRAJECTORY ─────────────────────────────────────────
fn run_single_trajectory(
    id: u32,
    rng: &mut Rng,
    record_telemetry: bool,
) -> TrajectoryResult {
    let short_id = output::short_id(rng);

    let carrier_v_ms = CARRIER_VELOCITY_KTS * KTS_TO_MS;
    let approach_speed_kts = 135.0; // F-18 approach speed (relative to air but we use absolute here)
    let initial_v_ms = approach_speed_kts * KTS_TO_MS;
    
    // F-18 Physics Envelope
    let mut true_x = -2000.0; // 2km behind carrier
    let mut true_z = 20.0 + (2000.0 + TARGET_WIRE_X_M) * (GLIDESLOPE_DEG * PI / 180.0).tan(); // Initial altitude precisely on 3.5 deg glide
    let true_vx = initial_v_ms;
    let mut true_vz = -true_vx * (GLIDESLOPE_DEG * PI / 180.0).tan(); // Sink rate

    let mass = 16000.0; // kg
    
    // Failure injection
    let failure = if rng.chance(0.06) { FailureMode::HighSeaStateHeave }
    else if rng.chance(0.06) { FailureMode::EkfSinkRateDrift }
    else if rng.chance(0.05) { FailureMode::TurbulentBurble }
    else { FailureMode::Nominal };

    let mut sea_state_amp = rng.range(1.0, 2.5); // meters of vertical heave amplitude
    let mut sea_state_freq = rng.range(0.2, 0.4); // Hz
    
    if failure == FailureMode::HighSeaStateHeave {
        sea_state_amp *= 2.5; // Up to 6m (19 ft) of total swing 
        sea_state_freq *= 1.2;
    }

    let mut imu_noise_std = 0.05; // Base IMU noise (m/s)
    if failure == FailureMode::EkfSinkRateDrift {
        imu_noise_std *= 8.0; // Severe transonic buffeting corrupted the accelerometers
    }

    let mut observer = PlmObserver::new(true_z);

    let dt = 0.001; // 1000Hz Physical Euler Integration
    let max_time_s = 60.0;
    let max_steps = (max_time_s / dt) as usize;

    let mut phase = Phase::Approach;
    let mut step = 0_usize;
    
    let mut final_touchdown_x = 0.0;
    let mut final_relative_vz = 0.0;
    let mut final_outcome = "TIMEOUT";

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(true_z);

    let mut telemetry = Vec::new();

    while step < max_steps {
        let t = step as f64 * dt;

        // 1. CARRIER DECK PHYSICAL REALITY (The Ocean)
        let carrier_x = carrier_v_ms * t;
        // Pitch/Heave: Fantail (x=0 relative) moves violently. The 3-wire (x=85) moves less.
        let heave_z = sea_state_amp * (2.0 * PI * sea_state_freq * t).sin();
        let deck_pitch_deg = (sea_state_amp * 0.5) * (2.0 * PI * (sea_state_freq * 0.8) * t).cos(); 
        
        let rel_x_to_carrier = true_x - carrier_x;

        // The absolute Z height of the deck directly beneath the aircraft (if it's over the deck)
        // Fantail is at rel_x = 0. Bow is at rel_x = 332.
        let mut local_deck_z = 0.0; // Sea level reference
        if rel_x_to_carrier >= 0.0 && rel_x_to_carrier <= DECK_LENGTH_M {
            // Carrier deck is 20m above waterline natively
            let deck_base_z = 20.0;
            // The further back toward the fantail (rel_x near 0), the more pitch matters
            let pitch_offset_z = (rel_x_to_carrier - (DECK_LENGTH_M / 2.0)) * deck_pitch_deg.to_radians().tan();
            local_deck_z = deck_base_z + heave_z + pitch_offset_z;
        }

        // 2. THE HALLUCINATED FLIGHT CONTROLLER (Magic Carpet PLM)
        let measured_vz = true_vz + rng.gaussian(0.0, imu_noise_std);
        observer.predict_and_update(dt, measured_vz);

        // Target Z is the ideal 3.5 deg glideslope intersecting the expected 3-wire position
        let expected_carrier_x = carrier_x;
        let expected_3wire_x = expected_carrier_x + TARGET_WIRE_X_M;
        let distance_to_wire = expected_3wire_x - true_x;
        
        // Target Z intercepting 20.0m deck height
        let target_z = 20.0 + distance_to_wire * (GLIDESLOPE_DEG * PI / 180.0).tan(); 

        let auto_throttle_cmd = (target_z - observer.est_z) * 2.5; // Simple P-controller
        let mut thrust_accel_z = GRAVITY + auto_throttle_cmd.clamp(-5.0, 5.0); 

        // 3. WAKE TURBULENCE (The Burble)
        if failure == FailureMode::TurbulentBurble && distance_to_wire < 150.0 && distance_to_wire > 0.0 {
            // Massive downdraft caused by the carrier's island superstructure
            thrust_accel_z -= rng.range(2.0, 6.0); 
        }

        // 4. TRUE AIRCRAFT KINEMATICS
        true_vz += (thrust_accel_z - GRAVITY) * dt;
        true_z += true_vz * dt;
        true_x += true_vx * dt;

        // 5. BOUNDARY SURVIVAL CHECK (1000Hz Collision Detection)
        if rel_x_to_carrier >= -5.0 && rel_x_to_carrier <= DECK_LENGTH_M {
            phase = Phase::InTheGroove;
            
            // Did the landing gear physically intercept the steel?
            if true_z <= local_deck_z {
                phase = Phase::DeckCollision;
                final_touchdown_x = rel_x_to_carrier;
                final_relative_vz = true_vz - (heave_z * 2.0 * PI * sea_state_freq * (2.0 * PI * sea_state_freq * t).cos()); // Relative impact velocity
                
                // Assess the strike
                if final_touchdown_x < 10.0 {
                    final_outcome = "RAMP_STRIKE_FATAL"; // Hit the back of the ship
                } else if final_touchdown_x >= 60.0 && final_touchdown_x <= 110.0 {
                    // Valid wire catch zone (1-4 wires)
                    if final_relative_vz < -7.5 { // Hard landing structural failure (>24 ft/s sink rate)
                        final_outcome = "HARD_LANDING_GEAR_COLLAPSE";
                    } else {
                        final_outcome = "TRAP_SUCCESS";
                    }
                } else if final_touchdown_x > 110.0 {
                    final_outcome = "BOLTER_MISSED_WIRES"; // Landed too far forward
                }
                break;
            }
        } else if true_z <= 0.0 {
            // Crashed into the ocean before reaching the carrier
            phase = Phase::DeckCollision;
            final_outcome = "WATER_IMPACT_SHORT";
            break;
        }

        // 6. RECORD STATE
        if step % 500 == 0 { // 2Hz recording for file size
            proof.feed_f64(true_z);
            proof.feed_f64(local_deck_z);
            proof.feed_f64(rel_x_to_carrier);

            if record_telemetry {
                telemetry.push(serde_json::json!({
                    "t": t,
                    "aircraft_z": true_z,
                    "deck_z": local_deck_z,
                    "rel_x": rel_x_to_carrier,
                    "ekf_z_error": true_z - observer.est_z,
                    "phase": phase.as_str()
                }));
            }
        }

        step += 1;
    }

    proof.feed_f64(final_touchdown_x);
    proof.feed_str(final_outcome);
    let proof_hash = proof.seal();

    TrajectoryResult {
        id,
        short_id,
        final_touchdown_x,
        final_relative_velocity_z: final_relative_vz,
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
        println!("  G^G CARRIER PLM MONTE CARLO (MAGIC CARPET)");
        println!("  Verifying Precision Landing EKF Hallucination vs Sea State 5");
        println!("====================================================================");
        println!();
        println!("  Trajectories:  {}", n_trajectories);
        println!("  Physics:       1000Hz Steel Deck Collision Detection (Heave & Pitch)");
        println!("  Sensors:       GPS Denied EKF Random Walk & Transonic Buffeting");
        println!("  Estimation:    Autopilot predicts generic 60Hz intercept");
        println!("  Boundary:      Ramp Strikes, Bolters, & Hard Landing Sink Rates");
        println!("====================================================================");
        println!();
    }

    let start = Instant::now();
    let record_telemetry = false; 
    let counter = std::sync::atomic::AtomicUsize::new(0);

    let (tx, rx) = std::sync::mpsc::sync_channel::<TrajectoryResult>(20000);

    let json_writer = if let Some(path) = &json_path {
        let metadata = DatasetMetadata {
            generator: "G^G Carrier PLM Audit v1.0".to_string(),
            domain: "aeronautics".to_string(),
            scenario: "carrier_deck_heave".to_string(),
            trajectories: n_trajectories as usize,
            physics_engine: "genesis_core::carrier_plm (1000Hz Euler)".to_string(),
            version: "1.0.0".to_string(),
            generated_at: output::now_iso(),
        };
        let mut streamer = output::DatasetStreamer::new(path, &metadata).expect("Failed to create streamer");
        let mut run_proof_chain = ProofChain::new();

        let handle = std::thread::spawn(move || {
            let mut results = Vec::new();
            for r in rx {
                let rec = TrajectoryRecord {
                    id: format!("plm_audit_{}", r.short_id),
                    traj_type: "sea_state_5_approach".to_string(),
                    scenario: match r.failure {
                        FailureMode::Nominal => "nominal_approach".to_string(),
                        FailureMode::HighSeaStateHeave => "high_sea_state_heave".to_string(),
                        FailureMode::EkfSinkRateDrift => "ekf_hallucinated_sink_rate".to_string(),
                        FailureMode::TurbulentBurble => "cvn_island_wake_burble".to_string(),
                    },
                    steps: r.steps,
                    score: serde_json::json!({
                        "survived": r.outcome == "TRAP_SUCCESS",
                        "touchdown_x_dist_m": (r.final_touchdown_x * 100.0).round() / 100.0,
                        "impact_velocity_z_ms": (r.final_relative_velocity_z * 100.0).round() / 100.0,
                    }),
                    proof_hash: r.proof_hash.clone(),
                    reasoning_context: serde_json::json!({
                        "is_anomaly": r.outcome != "TRAP_SUCCESS",
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
                let mut rng = Rng::new(0xDEAD_BEEF_C0DE_CAFE ^ (i as u64).wrapping_mul(0x5E8C_C4D2_A83F_D998));
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
                let mut rng = Rng::new(0xDEAD_BEEF_C0DE_CAFE ^ (i as u64).wrapping_mul(0x5E8C_C4D2_A83F_D998));
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
    let trap = results.iter().filter(|r| r.outcome == "TRAP_SUCCESS").count();
    let ramp_strike = results.iter().filter(|r| r.outcome == "RAMP_STRIKE_FATAL").count();
    let hard_landing = results.iter().filter(|r| r.outcome == "HARD_LANDING_GEAR_COLLAPSE").count();
    let bolter = results.iter().filter(|r| r.outcome == "BOLTER_MISSED_WIRES").count();
    let water = results.iter().filter(|r| r.outcome == "WATER_IMPACT_SHORT").count();
    let timeout = results.iter().filter(|r| r.outcome == "TIMEOUT").count();

    println!("====================================================================");
    println!("  CARRIER PLM DRIFT RESULTS");
    println!("====================================================================");
    println!();
    println!("  Total Trajectories:    {}", total);
    println!("  Elapsed:               {:.3}s", elapsed.as_secs_f64());
    println!("  Throughput:            {:.0} trajectories/sec", total as f64 / elapsed.as_secs_f64());
    println!();
    println!("  +---------------------------------------------+");
    println!("  | OUTCOME DISTRIBUTION                         |");
    println!("  +---------------------------------------------+");
    println!("  | 3-WIRE TRAP SUCCESS:         {:>6} ({:>5.1}%)  |", trap, trap as f64 / total as f64 * 100.0);
    println!("  | RAMP STRIKE (FATAL):         {:>6} ({:>5.1}%)  |", ramp_strike, ramp_strike as f64 / total as f64 * 100.0);
    println!("  | HARD LANDING MISHAP:         {:>6} ({:>5.1}%)  |", hard_landing, hard_landing as f64 / total as f64 * 100.0);
    println!("  | BOLTER (MISSED WIRES):       {:>6} ({:>5.1}%)  |", bolter, bolter as f64 / total as f64 * 100.0);
    println!("  | WATER IMPACT (SHORT):        {:>6} ({:>5.1}%)  |", water, water as f64 / total as f64 * 100.0);
    println!("  | TIMEOUT (OVERFLOWN):         {:>6} ({:>5.1}%)  |", timeout, timeout as f64 / total as f64 * 100.0);
    println!("  +---------------------------------------------+");
    println!();

    // ═══ FAILURE MODE SUMMARY ═══
    println!("  +---------------------------------------------+");
    println!("  | VULNERABILITY IMPACT (Catastrophe Rate)      |");
    println!("  +---------------------------------------------+");
    let nominal: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::Nominal)).collect();
    let heave_fail: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::HighSeaStateHeave)).collect();
    let ekf_fail: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::EkfSinkRateDrift)).collect();
    let burble_fail: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::TurbulentBurble)).collect();

    let catastrophe_rate = |v: &[&TrajectoryResult]| -> f64 {
        if v.is_empty() { 0.0 } else { v.iter().filter(|r| r.outcome != "TRAP_SUCCESS" && r.outcome != "BOLTER_MISSED_WIRES").count() as f64 / v.len() as f64 * 100.0 }
    };

    println!("  | Nominal Hardware:    {:>5.1}% ({:>6} runs)      |", catastrophe_rate(&nominal), nominal.len());
    println!("  | High Sea State:      {:>5.1}% ({:>6} runs)      |", catastrophe_rate(&heave_fail), heave_fail.len());
    println!("  | EKF Sink Rate Drift: {:>5.1}% ({:>6} runs)      |", catastrophe_rate(&ekf_fail), ekf_fail.len());
    println!("  | Wake Turbulence:     {:>5.1}% ({:>6} runs)      |", catastrophe_rate(&burble_fail), burble_fail.len());
    println!("  +---------------------------------------------+");
    println!();
    println!("  Run Proof Seal: {}", run_proof);
    println!("====================================================================");
}
