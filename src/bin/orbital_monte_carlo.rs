// G^G Orbital Monte Carlo — THE MAVEN QUESTION
// Can a satellite recover from tumble using only internal sensors in the dark window?
//
// THE EMBODIMENT: A satellite tumbling in eclipse. No star tracker. No sun sensor.
// Only rate gyros with drifting bias. Three reaction wheels that might be saturated.
// Thrusters with finite fuel. The body must feel its own rotation and arrest it
// before angular momentum tears it apart or the battery dies in the cold.
//
// This is the Dark Window. No human in the loop. The body proves sovereignty.

use std::time::Instant;
use genesis_core::physics::orbital::{
    self, OrbitalPhysics, SatelliteState,
    ReactionWheel, ThrusterSet, RateGyro, PowerSystem,
};
use genesis_core::rng::Rng;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output::{self, DatasetMetadata, TrajectoryRecord};

// ─── FAILURE MODES ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
enum FailureMode {
    WheelLock,        // 10% — one reaction wheel seizes
    GyroDriftSpike,   // 12% — gyro bias jumps 10x
    ThrusterStuckOn,  // 3%  — thruster fires continuously on one axis
    PowerEmergency,   // 8%  — battery capacity halved (cell failure)
}

// ─── RECOVERY PHASES ────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Tumble,          // Initial uncontrolled rotation
    RateReduction,   // Wheels/thrusters fighting to slow rotation
    FinePointing,    // Sub-degree pointing, limit cycles possible
    Stabilized,      // < 0.001 rad/s — VICTORY
    Lost,            // Power dead, fuel gone, or rate diverging
}

impl Phase {
    fn as_str(&self) -> &'static str {
        match self {
            Phase::Tumble => "TUMBLE",
            Phase::RateReduction => "RATE_REDUCTION",
            Phase::FinePointing => "FINE_POINTING",
            Phase::Stabilized => "STABILIZED",
            Phase::Lost => "LOST",
        }
    }
}

// ─── TRAJECTORY RESULT ──────────────────────────────────────────

#[derive(Debug)]
#[allow(dead_code)]
struct TrajectoryResult {
    id: u32,
    short_id: String,
    persona: &'static str,
    initial_omega_mag: f64,
    final_omega_mag: f64,
    altitude_km: f64,
    inertia_diag: [f64; 3],
    wheel_torque: f64,
    fuel_kg: f64,
    fuel_remaining: f64,
    gyro_noise: f64,
    battery_remaining_pct: f64,
    phase: Phase,
    outcome: &'static str,
    recovery_time_s: f64,
    peak_omega: f64,
    steps: usize,
    proof_hash: String,
    failure: Option<FailureMode>,
    wheel_saturations: u32,
    thruster_fires: u32,
    telemetry: Vec<serde_json::Value>,
}

// ─── SINGLE TRAJECTORY ─────────────────────────────────────────

fn run_single_trajectory(
    id: u32,
    rng: &mut Rng,
    record_telemetry: bool,
) -> TrajectoryResult {
    let short_id = output::short_id(rng);

    // Randomize spacecraft parameters
    let (persona, i_base, wheel_torque, wheel_momentum, thruster_torque) = match rng.index(4) {
        0 => ("MAVEN Satellite", rng.range(1000.0, 3000.0), rng.range(1.0, 5.0), rng.range(50.0, 150.0), rng.range(20.0, 100.0)),
        1 => ("mega-constellation v2", rng.range(500.0, 1000.0), rng.range(0.5, 2.0), rng.range(10.0, 50.0), rng.range(5.0, 20.0)),
        2 => ("6U CubeSat", rng.range(0.1, 0.5), rng.range(0.001, 0.01), rng.range(0.01, 0.1), rng.range(0.01, 0.1)),
        _ => ("Spy Satellite", rng.range(5000.0, 15000.0), rng.range(10.0, 50.0), rng.range(200.0, 1000.0), rng.range(100.0, 500.0)),
    };

    let altitude_km = rng.range(200.0, 800.0);
    // Inertia: keep ratios moderate (max 4:1) to avoid extreme coupling
    let ix = i_base * rng.range(0.5, 2.0);
    let iy = i_base * rng.range(0.5, 2.0);
    let iz = i_base * rng.range(0.5, 2.0);

    let fuel_kg = i_base * 0.02; // proportional fuel
    let initial_fuel = fuel_kg;
    let gyro_noise = rng.range(0.001, 0.01);
    let gyro_drift = rng.range(0.0001, 0.001);
    let battery_wh = i_base * 0.5;
    let solar_w = i_base * 0.3;

    // Initial tumble — range spans the capture boundary
    // Real tumble: 0.01-0.5 rad/s (post-separation), up to 2.0 (post-collision)
    let omega_x = rng.range(-1.0, 1.0);
    let omega_y = rng.range(-1.0, 1.0);
    let omega_z = rng.range(-1.0, 1.0);
    let initial_omega_mag = (omega_x*omega_x + omega_y*omega_y + omega_z*omega_z).sqrt();

    // Inject failure mode
    let failure = if rng.chance(0.10) { Some(FailureMode::WheelLock) }
    else if rng.chance(0.12) { Some(FailureMode::GyroDriftSpike) }
    else if rng.chance(0.03) { Some(FailureMode::ThrusterStuckOn) }
    else if rng.chance(0.08) { Some(FailureMode::PowerEmergency) }
    else { None };

    // Build satellite state
    let mut state = SatelliteState {
        position: [6378.137 + altitude_km, 0.0, 0.0],
        velocity: [0.0, (398600.4418 / (6378.137 + altitude_km)).sqrt(), 0.0],
        quaternion_attitude: [1.0, 0.0, 0.0, 0.0],
        angular_velocity: [omega_x, omega_y, omega_z],
        inertia_tensor: [
            [ix, 0.0, 0.0],
            [0.0, iy, 0.0],
            [0.0, 0.0, iz],
        ],
    };

    // Build subsystems
    let mut wheels = [
        ReactionWheel::new(wheel_torque, wheel_momentum),
        ReactionWheel::new(wheel_torque, wheel_momentum),
        ReactionWheel::new(wheel_torque, wheel_momentum),
    ];
    let mut thrusters = ThrusterSet::new(thruster_torque, 220.0, fuel_kg);
    let mut gyro = RateGyro::new(gyro_noise, gyro_drift);
    let mut power = PowerSystem::new(battery_wh, solar_w);

    // Apply failure modes
    match failure {
        Some(FailureMode::WheelLock) => {
            let axis = rng.index(3);
            wheels[axis].failed = true;
        }
        Some(FailureMode::GyroDriftSpike) => {
            gyro.bias_drift_rate *= 10.0;
        }
        Some(FailureMode::ThrusterStuckOn) => {
            thrusters.failed = true; // can't use thrusters reliably
        }
        Some(FailureMode::PowerEmergency) => {
            power.battery_wh *= 0.5;
            power.battery_max_wh *= 0.5;
        }
        None => {}
    }

    // Eclipse modeling
    let orbital_period = orbital::orbital_period_s(altitude_km);
    let eclipse_frac = orbital::eclipse_fraction(altitude_km);

    let dt = 0.01; // 100 Hz integration
    let max_time_s = 3600.0; // 1 hour max recovery window
    let max_steps = (max_time_s / dt) as usize;

    let mut phase = Phase::Tumble;
    let mut peak_omega = initial_omega_mag;
    let mut recovery_time = 0.0_f64;
    let mut wheel_saturations = 0_u32;
    let mut thruster_fires = 0_u32;
    let mut step = 0_usize;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(initial_omega_mag);
    proof.feed_f64(altitude_km);

    let mut telemetry = Vec::new();

    // ═══ SOMATIC LOOP: FEEL → DECIDE → ACTUATE → PROVE ═══
    while step < max_steps {
        let t = step as f64 * dt;

        // Eclipse: in shadow when orbital phase is in eclipse arc
        let orbit_phase = (t % orbital_period) / orbital_period;
        let in_sunlight = orbit_phase > eclipse_frac;

        // Power check
        let wheels_active = phase != Phase::Lost;
        if !power.step(dt, in_sunlight, wheels_active) {
            phase = Phase::Lost;
            break;
        }

        // ── FEEL: Read gyros (dark window — this is ALL the body has) ──
        let measured_omega = gyro.read(&state.angular_velocity, rng, dt);
        let measured_mag = orbital::omega_magnitude(&measured_omega);
        let true_mag = orbital::omega_magnitude(&state.angular_velocity);

        if true_mag > peak_omega { peak_omega = true_mag; }

        // ── PHASE TRANSITIONS ──
        match phase {
            Phase::Tumble => {
                if true_mag < 0.1 { phase = Phase::RateReduction; }
            }
            Phase::RateReduction => {
                if true_mag < 0.01 { phase = Phase::FinePointing; }
                if true_mag > 3.0 { phase = Phase::Lost; break; }
            }
            Phase::FinePointing => {
                if true_mag < 0.005 {
                    phase = Phase::Stabilized;
                    recovery_time = t;
                    break;
                }
                if true_mag > 0.1 { phase = Phase::RateReduction; }
            }
            Phase::Stabilized | Phase::Lost => break,
        }

        // ── DECIDE + ACTUATE: Phase-aware ADCS (real spacecraft strategy) ──
        let mut total_torque = [0.0_f64; 3];

        match phase {
            Phase::Tumble => {
                // HIGH RATE: Proportional thrusters with deadband.
                // Gain scaled to overwhelm gyroscopic coupling: tau = -kp * I * omega
                // where kp > max(|I_j - I_k|/I_i) to ensure stability
                if !thrusters.failed && thrusters.fuel_kg > 0.0 {
                    let deadband = 0.02; // rad/s
                    let kp = 0.5; // proportional gain (rad/s -> rad/s^2)
                    let thr_cmd = [
                        if measured_omega[0].abs() > deadband { -kp * ix * measured_omega[0] } else { 0.0 },
                        if measured_omega[1].abs() > deadband { -kp * iy * measured_omega[1] } else { 0.0 },
                        if measured_omega[2].abs() > deadband { -kp * iz * measured_omega[2] } else { 0.0 },
                    ];
                    let thr_torque = thrusters.fire(thr_cmd, dt);
                    for axis in 0..3 { total_torque[axis] += thr_torque[axis]; }
                    thruster_fires += 1;
                }
            }
            Phase::RateReduction => {
                // MEDIUM RATE: Wheels with thruster desaturation.
                let inertias = [ix, iy, iz];
                for axis in 0..3 {
                    let kp = 2.0 * inertias[axis].sqrt();
                    let cmd = -kp * measured_omega[axis];
                    let delivered = wheels[axis].command(cmd, dt);
                    total_torque[axis] += delivered;
                }
                // Desaturate wheels via thrusters
                let any_saturated = wheels.iter().any(|w| w.is_saturated());
                if any_saturated && !thrusters.failed && thrusters.fuel_kg > 0.0 {
                    wheel_saturations += 1;
                    let desat_cmd = [
                        -wheels[0].momentum * 0.5,
                        -wheels[1].momentum * 0.5,
                        -wheels[2].momentum * 0.5,
                    ];
                    let thr_torque = thrusters.fire(desat_cmd, dt);
                    for axis in 0..3 { total_torque[axis] += thr_torque[axis]; }
                    thruster_fires += 1;
                }
            }
            Phase::FinePointing => {
                // LOW RATE: Wheels only. Gentle proportional control.
                let inertias = [ix, iy, iz];
                for axis in 0..3 {
                    let kp = 1.0 * inertias[axis].sqrt();
                    let cmd = -kp * measured_omega[axis];
                    let delivered = wheels[axis].command(cmd, dt);
                    total_torque[axis] += delivered;
                }
            }
            Phase::Stabilized | Phase::Lost => {}
        }

        // ── INTEGRATE: Euler's rotational equations ──
        OrbitalPhysics::step_attitude(&mut state, &total_torque, dt);

        // NaN safety: if numerical instability occurs, declare lost
        if !state.angular_velocity[0].is_finite()
            || !state.angular_velocity[1].is_finite()
            || !state.angular_velocity[2].is_finite()
        {
            state.angular_velocity = [0.0; 3]; // zero out for stats
            phase = Phase::Lost;
            break;
        }

        // ── PROVE: SHA-256 at 1Hz cadence ──
        if step % 100 == 0 {
            proof.feed_f64(state.angular_velocity[0]);
            proof.feed_f64(state.angular_velocity[1]);
            proof.feed_f64(state.angular_velocity[2]);

            if record_telemetry {
                telemetry.push(serde_json::json!({
                    "t": t,
                    "omega": [state.angular_velocity[0], state.angular_velocity[1], state.angular_velocity[2]],
                    "omega_mag": true_mag,
                    "measured_mag": measured_mag,
                    "phase": phase.as_str(),
                    "battery_pct": power.charge_fraction() * 100.0,
                    "fuel_kg": thrusters.fuel_kg,
                    "wheel_h": [wheels[0].momentum, wheels[1].momentum, wheels[2].momentum],
                    "in_sunlight": in_sunlight,
                }));
            }
        }

        step += 1;
    }

    let final_omega_mag = orbital::omega_magnitude(&state.angular_velocity);

    let outcome = match phase {
        Phase::Stabilized => "RECOVERED",
        Phase::FinePointing => "FINE_POINTING_TIMEOUT",
        Phase::RateReduction => "RATE_REDUCTION_TIMEOUT",
        Phase::Tumble => "TUMBLE_TIMEOUT",
        Phase::Lost => {
            if power.charge_fraction() <= 0.0 { "POWER_DEAD" }
            else if final_omega_mag > 3.0 { "RATE_DIVERGENCE" }
            else { "LOST_UNKNOWN" }
        }
    };

    if phase != Phase::Stabilized {
        recovery_time = step as f64 * dt; // total time used
    }

    proof.feed_f64(final_omega_mag);
    proof.feed_str(phase.as_str());
    proof.feed_str(outcome);
    let proof_hash = proof.seal();

    TrajectoryResult {
        id,
        short_id,
        persona,
        initial_omega_mag,
        final_omega_mag,
        altitude_km,
        inertia_diag: [ix, iy, iz],
        wheel_torque,
        fuel_kg: initial_fuel,
        fuel_remaining: thrusters.fuel_kg,
        gyro_noise,
        battery_remaining_pct: power.charge_fraction() * 100.0,
        phase,
        outcome,
        recovery_time_s: recovery_time,
        peak_omega,
        steps: step,
        proof_hash,
        failure,
        wheel_saturations,
        thruster_fires,
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
        println!("  G^G ORBITAL MONTE CARLO — THE MAVEN QUESTION");
        println!("  Can a satellite recover from tumble in the dark window?");
        println!("====================================================================");
        println!();
        println!("  Trajectories:  {}", n_trajectories);
        println!("  Physics:       Euler rotational eqs + quaternion propagation");
        println!("  Actuators:     3x reaction wheels + thruster desaturation");
        println!("  Sensors:       Rate gyros only (DARK WINDOW — no star tracker)");
        println!("  Integration:   100Hz, SHA-256 proof at 1Hz cadence");
        println!("  Recovery:      TUMBLE -> RATE_REDUCTION -> FINE_POINTING -> STABILIZED");
        println!("  Failures:      Wheel lock 10%, Gyro spike 12%, Thruster stuck 3%, Power 8%");
        println!("====================================================================");
        println!();
    }

    let start = Instant::now();
    let record_telemetry = false; // Disabled by Commander directive to prevent SSD memory saturation
    let counter = std::sync::atomic::AtomicUsize::new(0);

    let (tx, rx) = std::sync::mpsc::sync_channel::<TrajectoryResult>(10000);

    let json_writer = if let Some(path) = &json_path {
        let metadata = DatasetMetadata {
            generator: "G^G Orbital Monte Carlo v1.0".to_string(),
            domain: "orbital".to_string(),
            scenario: "tumble_recovery_dark_window".to_string(),
            trajectories: n_trajectories as usize,
            physics_engine: "genesis_core::orbital (Euler rotational 100Hz)".to_string(),
            version: "1.0.0".to_string(),
            generated_at: output::now_iso(),
        };
        let mut streamer = output::DatasetStreamer::new(path, &metadata).expect("Failed to create streamer");
        let mut run_proof_chain = ProofChain::new();

        let handle = std::thread::spawn(move || {
            let mut results = Vec::new();
            for r in rx {
                let rec = TrajectoryRecord {
                    id: format!("orbital_tumble_{}", r.short_id),
                    traj_type: "tumble_recovery".to_string(),
                    scenario: match r.failure {
                        Some(FailureMode::WheelLock) => "wheel_lock".to_string(),
                        Some(FailureMode::GyroDriftSpike) => "gyro_drift_spike".to_string(),
                        Some(FailureMode::ThrusterStuckOn) => "thruster_stuck".to_string(),
                        Some(FailureMode::PowerEmergency) => "power_emergency".to_string(),
                        None => "nominal".to_string(),
                    },
                    steps: r.steps,
                    score: serde_json::json!({
                        "recovered": r.phase == Phase::Stabilized,
                        "final_omega_rad_s": (r.final_omega_mag * 1000.0).round() / 1000.0,
                        "recovery_time_s": (r.recovery_time_s * 10.0).round() / 10.0,
                        "fuel_remaining_pct": if r.fuel_kg > 0.0 {
                            (r.fuel_remaining / r.fuel_kg * 100.0 * 10.0).round() / 10.0
                        } else { 0.0 },
                    }),
                    proof_hash: r.proof_hash.clone(),
                    reasoning_context: serde_json::json!({
                        "is_anomaly": r.outcome != "RECOVERED",
                        "anomaly_type": r.outcome,
                        "snapshot": {
                            "persona": r.persona,
                            "initial_omega": (r.initial_omega_mag * 1000.0).round() / 1000.0,
                            "altitude_km": (r.altitude_km * 10.0).round() / 10.0,
                            "inertia": r.inertia_diag.map(|v| (v * 10.0).round() / 10.0),
                            "wheel_torque_nm": (r.wheel_torque * 100.0).round() / 100.0,
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
                let mut rng = Rng::new(0x0AB1_7A1E_DACE_DA4E ^ (i as u64).wrapping_mul(0x9E37_79B1_85EB_CA87));
                let r = run_single_trajectory(i, &mut rng, record_telemetry);

                if !json_output {
                    let count = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    if count % 1000 == 0 {
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
                let mut rng = Rng::new(0x0AB1_7A1E_DACE_DA4E ^ (i as u64).wrapping_mul(0x9E37_79B1_85EB_CA87));
                let r = run_single_trajectory(i, &mut rng, record_telemetry);
                sender.send(r).expect("Channel closed");
            });
        Vec::new()
    };

    let (run_proof, results) = if let Some(handle) = json_writer {
        handle.join().expect("Writer thread panicked")
    } else {
        inline_results.sort_by_key(|r| r.id);
        if !json_output { eprintln!(); }
        
        let proof_hashes: Vec<String> = inline_results.iter().map(|r| r.proof_hash.clone()).collect();
        (proof::seal_run(&proof_hashes), inline_results)
    };

    let elapsed = start.elapsed();

    // === JSON OUTPUT MODE ===
    if json_output || json_path.is_some() {
        if let Some(path) = &json_path {
            eprintln!("  Written to: {}", path);
            eprintln!("  Run proof:  {}", run_proof);
        } else {
            // Stdout stream logic isn't handled here for simplicity; usually file-target is expected
        }
        return;
    }

    // === DIAGNOSTIC OUTPUT MODE ===
    let total = results.len();
    let recovered = results.iter().filter(|r| r.phase == Phase::Stabilized).count();
    let fine_timeout = results.iter().filter(|r| r.outcome == "FINE_POINTING_TIMEOUT").count();
    let rate_timeout = results.iter().filter(|r| r.outcome == "RATE_REDUCTION_TIMEOUT").count();
    let tumble_timeout = results.iter().filter(|r| r.outcome == "TUMBLE_TIMEOUT").count();
    let power_dead = results.iter().filter(|r| r.outcome == "POWER_DEAD").count();
    let rate_diverge = results.iter().filter(|r| r.outcome == "RATE_DIVERGENCE").count();

    let recovered_results: Vec<&TrajectoryResult> = results.iter()
        .filter(|r| r.phase == Phase::Stabilized).collect();
    let avg_recovery = if recovered_results.is_empty() { 0.0 } else {
        recovered_results.iter().map(|r| r.recovery_time_s).sum::<f64>() / recovered_results.len() as f64
    };

    let avg_final_omega: f64 = results.iter()
        .map(|r| r.final_omega_mag).filter(|v| v.is_finite())
        .sum::<f64>() / total as f64;
    let avg_fuel_used: f64 = results.iter()
        .map(|r| r.fuel_kg - r.fuel_remaining).filter(|v| v.is_finite())
        .sum::<f64>() / total as f64;

    let proof_hashes: Vec<String> = results.iter().map(|r| r.proof_hash.clone()).collect();
    let run_proof = proof::seal_run(&proof_hashes);

    println!("====================================================================");
    println!("  RESULTS — THE MAVEN QUESTION");
    println!("====================================================================");
    println!();
    println!("  Total Trajectories:    {}", total);
    println!("  Elapsed:               {:.3}s", elapsed.as_secs_f64());
    println!("  Throughput:            {:.0} trajectories/sec", total as f64 / elapsed.as_secs_f64());
    println!();
    println!("  +---------------------------------------------+");
    println!("  | OUTCOME DISTRIBUTION                         |");
    println!("  +---------------------------------------------+");
    println!("  | RECOVERED (stabilized):     {:>6} ({:>5.1}%)  |", recovered, recovered as f64 / total as f64 * 100.0);
    println!("  | FINE_POINTING_TIMEOUT:      {:>6} ({:>5.1}%)  |", fine_timeout, fine_timeout as f64 / total as f64 * 100.0);
    println!("  | RATE_REDUCTION_TIMEOUT:     {:>6} ({:>5.1}%)  |", rate_timeout, rate_timeout as f64 / total as f64 * 100.0);
    println!("  | TUMBLE_TIMEOUT:             {:>6} ({:>5.1}%)  |", tumble_timeout, tumble_timeout as f64 / total as f64 * 100.0);
    println!("  | POWER_DEAD:                 {:>6} ({:>5.1}%)  |", power_dead, power_dead as f64 / total as f64 * 100.0);
    println!("  | RATE_DIVERGENCE:            {:>6} ({:>5.1}%)  |", rate_diverge, rate_diverge as f64 / total as f64 * 100.0);
    println!("  +---------------------------------------------+");
    println!();
    println!("  Recovery Rate:         {:.2}%", recovered as f64 / total as f64 * 100.0);
    println!("  Avg Recovery Time:     {:.1}s (for recovered only)", avg_recovery);
    println!("  Avg Final Omega:       {:.6} rad/s", avg_final_omega);
    println!("  Avg Fuel Used:         {:.2} kg", avg_fuel_used);
    println!();

    // ═══ THE MAVEN QUESTION: Recovery rate vs initial angular momentum ═══
    // Capture boundary in angular momentum space
    println!("  +----------------------------------------------------------+");
    println!("  | THE MAVEN QUESTION                                        |");
    println!("  | Recovery rate vs initial tumble rate (angular momentum)    |");
    println!("  +----------------------------------------------------------+");
    let n_buckets = 10;
    let omega_max = 1.8; // max initial omega magnitude
    let bucket_width = omega_max / n_buckets as f64;
    for b in 0..n_buckets {
        let lo = b as f64 * bucket_width;
        let hi = lo + bucket_width;
        let in_bucket: Vec<&TrajectoryResult> = results.iter()
            .filter(|r| r.initial_omega_mag >= lo && r.initial_omega_mag < hi)
            .collect();
        let bucket_total = in_bucket.len();
        let bucket_recovered = in_bucket.iter().filter(|r| r.phase == Phase::Stabilized).count();
        let rate = if bucket_total > 0 { bucket_recovered as f64 / bucket_total as f64 * 100.0 } else { 0.0 };
        let bar_len = (rate / 2.0) as usize;
        let bar: String = "#".repeat(bar_len);
        println!("  | {:>4.1}-{:>4.1} rad/s | {:>5}/{:>5} | {:>5.1}% |{:<50}|",
            lo, hi, bucket_recovered, bucket_total, rate, bar);
    }
    println!("  +----------------------------------------------------------+");
    println!();

    // ═══ FAILURE MODE IMPACT ═══
    println!("  +---------------------------------------------+");
    println!("  | FAILURE MODE IMPACT                          |");
    println!("  +---------------------------------------------+");
    let no_failure: Vec<&TrajectoryResult> = results.iter().filter(|r| r.failure.is_none()).collect();
    let wheel_lock: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, Some(FailureMode::WheelLock))).collect();
    let gyro_spike: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, Some(FailureMode::GyroDriftSpike))).collect();
    let thr_stuck: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, Some(FailureMode::ThrusterStuckOn))).collect();
    let pwr_emerg: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, Some(FailureMode::PowerEmergency))).collect();

    let recovery_rate = |v: &[&TrajectoryResult]| -> f64 {
        if v.is_empty() { 0.0 } else { v.iter().filter(|r| r.phase == Phase::Stabilized).count() as f64 / v.len() as f64 * 100.0 }
    };

    println!("  | No failure:    {:>5.1}% ({:>5} trajectories)  |", recovery_rate(&no_failure), no_failure.len());
    println!("  | Wheel lock:    {:>5.1}% ({:>5} trajectories)  |", recovery_rate(&wheel_lock), wheel_lock.len());
    println!("  | Gyro spike:    {:>5.1}% ({:>5} trajectories)  |", recovery_rate(&gyro_spike), gyro_spike.len());
    println!("  | Thruster stuck:{:>5.1}% ({:>5} trajectories)  |", recovery_rate(&thr_stuck), thr_stuck.len());
    println!("  | Power emerg:   {:>5.1}% ({:>5} trajectories)  |", recovery_rate(&pwr_emerg), pwr_emerg.len());
    println!("  +---------------------------------------------+");
    println!();

    // Sample trajectories
    println!("  +-------------------------------------------------------------------------+");
    println!("  | SAMPLE TRAJECTORIES                                                      |");
    println!("  +------+----------+----------+----------+---------+------------------------+");
    println!("  |  ID  | w0(r/s)  | wf(r/s)  | t_rec(s) | Outcome | Proof (16)             |");
    println!("  +------+----------+----------+----------+---------+------------------------+");
    for r in results.iter().take(8) {
        println!("  |{:>5} | {:>8.3} | {:>8.5} | {:>8.1} | {:>7} | {}... |",
            r.id,
            r.initial_omega_mag,
            r.final_omega_mag,
            r.recovery_time_s,
            if r.phase == Phase::Stabilized { "OK" } else { "FAIL" },
            &r.proof_hash[..24],
        );
    }
    println!("  +------+----------+----------+----------+---------+------------------------+");
    println!();
    println!("  SHA-256 Run Proof: {}", run_proof);
    println!();
    println!("====================================================================");
    println!("  G^G ORBITAL MONTE CARLO — SEALED");
    println!("  {} trajectories x 100Hz x SHA-256 = SOVEREIGN", total);
    println!("  DARK WINDOW. NO STAR TRACKER. THE BODY FELT ITS WAY.");
    println!("====================================================================");
}
