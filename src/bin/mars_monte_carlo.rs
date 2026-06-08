use std::time::Instant;
use genesis_core::physics::mars_edl::{MarsPhysics, EdlVehicle};
use genesis_core::rng::Rng;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output::{self, DatasetMetadata, TrajectoryRecord, Dataset, DatasetStreamer};
use rayon::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

#[derive(Debug)]
#[allow(dead_code)]
struct TrajectoryResult {
    id: u32,
    short_id: String,
    initial_altitude: f64,
    initial_velocity: f64,
    entry_angle_deg: f64,
    final_altitude: f64,
    final_velocity: f64,
    max_deceleration_g: f64,
    flight_time: f64,
    dust_storm: bool,
    mission_success: bool,
    anomaly: &'static str,
    steps: usize,
    proof_hash: String,
    retro_fired: bool,
    velocity_at_retro: f64,
    altitude_at_retro: f64,
    chute_deployed: bool,
    telemetry: Vec<serde_json::Value>,
}

fn run_single_trajectory(
    id: u32,
    physics: &MarsPhysics,
    vehicle: &EdlVehicle,
    rng: &mut Rng,
    record_telemetry: bool,
) -> TrajectoryResult {
    let short_id = output::short_id(rng);

    // Randomize initial conditions — post-peak-deceleration Mars EDL
    let init_alt = rng.range(20000.0, 60000.0);
    let init_speed = rng.range(150.0, 500.0);
    let entry_angle = rng.range(-25.0, -8.0);
    let dust_storm = rng.chance(0.15);

    let angle_rad = entry_angle.to_radians();
    let vx = init_speed * angle_rad.cos() * rng.range(0.2, 0.6);
    let vy = init_speed * angle_rad.cos() * rng.range(-0.1, 0.1);
    let vz = init_speed * angle_rad.sin();

    let mut pos = [0.0_f64, 0.0, init_alt];
    let mut vel = [vx, vy, vz];

    // EDL phase parameters (randomized per-trajectory)
    let chute_deploy_alt = rng.range(8000.0, 15000.0);
    let chute_drag_mult = rng.range(15.0, 30.0);
    let chute_failure = rng.chance(0.05);

    let retro_fire_alt = rng.range(1500.0, 4000.0);
    let retro_thrust = rng.range(15000.0, 30000.0);
    let retro_fuel_seconds = rng.range(60.0, 150.0);
    let retro_failure = rng.chance(0.08);

    let dust_chute_penalty = if dust_storm { rng.range(1000.0, 3000.0) } else { 0.0 };
    let effective_chute_alt = chute_deploy_alt - dust_chute_penalty;
    let base_cd = if dust_storm { vehicle.cd * 1.3 } else { vehicle.cd };

    let dt = 0.001; // 1000 Hz
    let max_steps = 600_000;
    let mut max_decel_g = 0.0_f64;
    let mut step = 0;
    let mut chute_deployed = false;
    let mut retro_firing = false;
    let mut retro_fuel_remaining = retro_fuel_seconds;
    let mut phase: &str = "ENTRY";
    let mut velocity_at_retro = 0.0_f64;
    let mut altitude_at_retro = 0.0_f64;
    let mut retro_ever_fired = false;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(init_alt);
    proof.feed_f64(init_speed);

    let mut telemetry = Vec::new();

    // SOMATIC LOOP: FEEL -> DECIDE -> ACTUATE -> PROVE
    while step < max_steps {
        let alt = pos[2];
        if alt <= 0.0 { break; }

        let speed = (vel[0] * vel[0] + vel[1] * vel[1] + vel[2] * vel[2]).sqrt();

        // === PHASE TRANSITIONS (Dark Window Brain) ===
        if !chute_deployed && alt < effective_chute_alt && !chute_failure {
            chute_deployed = true;
            phase = "CHUTE";
        }

        // Retro-propulsion ignition — SUICIDE BURN TIMING
        if !retro_firing && !retro_failure && retro_fuel_remaining > 0.0 && alt < retro_fire_alt {
            let net_decel = retro_thrust / vehicle.mass - physics.gravity;
            let required_alt = if net_decel > 0.0 { speed * speed / (2.0 * net_decel) } else { f64::MAX };
            if alt <= required_alt * 1.2 {
                retro_firing = true;
                retro_ever_fired = true;
                velocity_at_retro = speed;
                altitude_at_retro = alt;
                phase = "RETRO";
            }
        }

        let effective_drag_area = if chute_deployed {
            vehicle.drag_area * chute_drag_mult
        } else {
            vehicle.drag_area
        };

        // FEEL: compute atmospheric forces
        let rho = physics.atmosphere_density(alt);
        let drag_mag = 0.5 * rho * speed.powi(2) * base_cd * effective_drag_area;
        let mut force = [0.0_f64; 3];
        if speed > 1e-6 {
            force[0] = -drag_mag * (vel[0] / speed);
            force[1] = -drag_mag * (vel[1] / speed);
            force[2] = -drag_mag * (vel[2] / speed);
        }

        // Weight (always down)
        force[2] -= vehicle.mass * physics.gravity;

        // ACTUATE: Retro-propulsion — SUICIDE BURN PROFILE
        if retro_firing && retro_fuel_remaining > 0.0 {
            if speed > 2.0 {
                force[0] += -retro_thrust * (vel[0] / speed);
                force[1] += -retro_thrust * (vel[1] / speed);
                force[2] += -retro_thrust * (vel[2] / speed);
            } else {
                let target_vz = -1.5;
                let vz_error = vel[2] - target_vz;
                let vert_thrust = (vehicle.mass * physics.gravity + vz_error * vehicle.mass * 10.0)
                    .clamp(0.0, retro_thrust * 0.95);
                force[2] += vert_thrust;
                let h_speed = (vel[0] * vel[0] + vel[1] * vel[1]).sqrt();
                if h_speed > 0.01 {
                    let remaining = retro_thrust - vert_thrust;
                    if remaining > 0.0 {
                        force[0] += -remaining * (vel[0] / h_speed);
                        force[1] += -remaining * (vel[1] / h_speed);
                    }
                }
            }
            retro_fuel_remaining -= dt;
            if retro_fuel_remaining <= 0.0 {
                retro_firing = false;
                phase = "BALLISTIC";
            }
        }

        // INTEGRATE: Euler
        let ax = force[0] / vehicle.mass;
        let ay = force[1] / vehicle.mass;
        let az = force[2] / vehicle.mass;

        vel[0] += ax * dt;
        vel[1] += ay * dt;
        vel[2] += az * dt;

        pos[0] += vel[0] * dt;
        pos[1] += vel[1] * dt;
        pos[2] += vel[2] * dt;

        let accel_mag = (ax * ax + ay * ay + az * az).sqrt();
        let decel_g = accel_mag / 9.81;
        if decel_g > max_decel_g { max_decel_g = decel_g; }

        // PROVE: Hash every 1000th step (1 Hz proof cadence)
        if step % 1000 == 0 {
            proof.feed_f64(pos[2]);
            proof.feed_f64(vel[2]);

            // Record telemetry for JSON output (every 1Hz = every 1000 steps)
            if record_telemetry {
                telemetry.push(serde_json::json!({
                    "t": step as f64 * dt,
                    "alt": pos[2],
                    "vel": speed,
                    "phase": phase,
                    "pos": [pos[0], pos[1], pos[2]],
                    "rho": rho,
                }));
            }
        }

        step += 1;
    }

    let final_speed = (vel[0] * vel[0] + vel[1] * vel[1] + vel[2] * vel[2]).sqrt();
    let flight_time = step as f64 * dt;
    let success = pos[2] <= 0.0 && final_speed < 10.0;

    let anomaly = if pos[2] > 0.0 {
        "TIMEOUT_STILL_AIRBORNE"
    } else if chute_failure && retro_failure {
        "DUAL_FAILURE_LITHOBRAKE"
    } else if final_speed > 100.0 && chute_failure {
        "CHUTE_FAILURE_IMPACT"
    } else if final_speed > 100.0 && retro_failure {
        "RETRO_FAILURE_IMPACT"
    } else if final_speed > 100.0 {
        "LITHOBRAKING_FAILURE"
    } else if final_speed > 10.0 {
        "HARD_LANDING"
    } else if dust_storm && success {
        "NOMINAL_DUST_STORM"
    } else if success {
        "NOMINAL"
    } else {
        "UNKNOWN_FAILURE"
    };

    // Seal the proof
    proof.feed_f64(final_speed);
    proof.feed_str(phase);
    proof.feed_str(if success { "SUCCESS" } else { "FAILURE" });
    let proof_hash = proof.seal();

    TrajectoryResult {
        id,
        short_id,
        initial_altitude: init_alt,
        initial_velocity: init_speed,
        entry_angle_deg: entry_angle,
        final_altitude: pos[2].max(0.0),
        final_velocity: final_speed,
        max_deceleration_g: max_decel_g,
        flight_time,
        dust_storm,
        mission_success: success,
        anomaly,
        steps: step,
        proof_hash,
        retro_fired: retro_ever_fired,
        velocity_at_retro,
        altitude_at_retro,
        chute_deployed,
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
        println!("═══════════════════════════════════════════════════════════");
        println!("  G^G MARS EDL MONTE CARLO");
        println!("  Trajectories: {}", n_trajectories);
        println!("  Physics: genesis_core::mars_edl (0.38g, CO₂ exponential)");
        println!("  EDL Phases: ENTRY (20-60km) → CHUTE (8-15km) → RETRO (1-4km) → TOUCHDOWN");
        println!("  Integration: 1000Hz Euler, SHA-256 proof at 1Hz cadence");
        println!("  Vehicle: 1000kg, 10m² heat shield, Cd=1.2");
        println!("  Failure Modes: 5%% chute fail, 8%% retro fail, 15%% dust storm");
        println!("═══════════════════════════════════════════════════════════");
        println!();
    }

    let physics = MarsPhysics::default();
    let vehicle = EdlVehicle::default();

    let start = Instant::now();
    let record_telemetry = json_output || json_path.is_some();
    let counter = AtomicUsize::new(0);

    let (tx, rx) = mpsc::channel::<TrajectoryResult>();

    let json_writer = if let Some(path) = &json_path {
        let metadata = DatasetMetadata {
            generator: "G^G Mars Monte Carlo v1.0".to_string(),
            domain: "mars".to_string(),
            scenario: "edl_monte_carlo".to_string(),
            trajectories: n_trajectories as usize,
            physics_engine: "genesis_core::mars_edl (Euler 1000Hz)".to_string(),
            version: "1.0.0".to_string(),
            generated_at: output::now_iso(),
        };
        let mut streamer = DatasetStreamer::new(path, &metadata).expect("Failed to create streamer");
        let mut run_proof_chain = ProofChain::new();

        let handle = std::thread::spawn(move || {
            let mut results = Vec::new();
            for r in rx {
                let rec = TrajectoryRecord {
                    id: format!("mars_edl_{}", r.short_id),
                    traj_type: "edl_descent".to_string(),
                    scenario: if r.dust_storm { "dust_storm_edl".to_string() } else { "nominal_edl".to_string() },
                    steps: r.steps,
                    score: serde_json::json!({
                        "mission_success": r.mission_success,
                        "final_velocity": (r.final_velocity * 100.0).round() / 100.0,
                        "dispersion": 0.0,
                        "max_deceleration_g": (r.max_deceleration_g * 100.0).round() / 100.0,
                    }),
                    proof_hash: r.proof_hash.clone(),
                    reasoning_context: serde_json::json!({
                        "is_anomaly": r.anomaly != "NOMINAL" && r.anomaly != "NOMINAL_DUST_STORM",
                        "anomaly_type": r.anomaly,
                        "snapshot": {
                            "alt": (r.initial_altitude * 10.0).round() / 10.0,
                            "vel": (r.initial_velocity * 10.0).round() / 10.0,
                            "dust_storm": r.dust_storm,
                            "chute_deployed": r.chute_deployed,
                            "retro_fired": r.retro_fired,
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

    let mut inline_results: Vec<TrajectoryResult> = if tx_ref.is_none() {
        (0..n_trajectories)
            .into_par_iter()
            .map(|i| {
                let mut rng = Rng::new(0xDEAD_BEEF_CAFE_8008 ^ (i as u64).wrapping_mul(0x9E37_79B1_85EB_CA87));
                let r = run_single_trajectory(i, &physics, &vehicle, &mut rng, record_telemetry);

                if !json_output {
                    let count = counter.fetch_add(1, Ordering::Relaxed) + 1;
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
                let mut rng = Rng::new(0xDEAD_BEEF_CAFE_8008 ^ (i as u64).wrapping_mul(0x9E37_79B1_85EB_CA87));
                let r = run_single_trajectory(i, &physics, &vehicle, &mut rng, record_telemetry);
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

    // === JSON STREAM EXITED HERE ===
    let elapsed = start.elapsed();
    
    if json_output || json_path.is_some() {
        if let Some(path) = &json_path {
            eprintln!("  Written to: {}", path);
            eprintln!("  Run proof:  {}", run_proof);
        }
        return;
    }

    // === DIAGNOSTIC OUTPUT MODE (default) ===
    let total = results.len();
    let successes = results.iter().filter(|r| r.mission_success).count();
    let dust_storms = results.iter().filter(|r| r.dust_storm).count();
    let lithobraking = results.iter().filter(|r| r.anomaly == "LITHOBRAKING_FAILURE").count();
    let hard_landings = results.iter().filter(|r| r.anomaly == "HARD_LANDING").count();
    let timeouts = results.iter().filter(|r| r.anomaly == "TIMEOUT_STILL_AIRBORNE").count();
    let nominal_dust = results.iter().filter(|r| r.anomaly == "NOMINAL_DUST_STORM").count();
    let nominal = results.iter().filter(|r| r.anomaly == "NOMINAL").count();
    let chute_fail = results.iter().filter(|r| r.anomaly == "CHUTE_FAILURE_IMPACT").count();
    let retro_fail = results.iter().filter(|r| r.anomaly == "RETRO_FAILURE_IMPACT").count();
    let dual_fail = results.iter().filter(|r| r.anomaly == "DUAL_FAILURE_LITHOBRAKE").count();

    let avg_final_vel: f64 = results.iter().map(|r| r.final_velocity).sum::<f64>() / total as f64;
    let avg_flight_time: f64 = results.iter().map(|r| r.flight_time).sum::<f64>() / total as f64;
    let avg_max_g: f64 = results.iter().map(|r| r.max_deceleration_g).sum::<f64>() / total as f64;
    let max_g_overall = results.iter().map(|r| r.max_deceleration_g).fold(0.0_f64, f64::max);
    let min_final_vel = results.iter().map(|r| r.final_velocity).fold(f64::INFINITY, f64::min);
    let max_final_vel = results.iter().map(|r| r.final_velocity).fold(0.0_f64, f64::max);

    // Run proof generated dynamically in struct now.

    println!("═══════════════════════════════════════════════════════════");
    println!("  RESULTS");
    println!("═══════════════════════════════════════════════════════════");
    println!();
    println!("  Total Trajectories:    {}", total);
    println!("  Elapsed:               {:.3}s", elapsed.as_secs_f64());
    println!("  Throughput:            {:.0} trajectories/sec", total as f64 / elapsed.as_secs_f64());
    println!();
    println!("  ┌─────────────────────────────────────────────┐");
    println!("  │ OUTCOME DISTRIBUTION                        │");
    println!("  ├─────────────────────────────────────────────┤");
    println!("  │ NOMINAL (success):          {:>6} ({:.1}%)  │", nominal, nominal as f64 / total as f64 * 100.0);
    println!("  │ NOMINAL_DUST_STORM:         {:>6} ({:.1}%)  │", nominal_dust, nominal_dust as f64 / total as f64 * 100.0);
    println!("  │ HARD_LANDING (v>10 m/s):    {:>6} ({:.1}%)  │", hard_landings, hard_landings as f64 / total as f64 * 100.0);
    println!("  │ CHUTE_FAILURE_IMPACT:       {:>6} ({:.1}%)  │", chute_fail, chute_fail as f64 / total as f64 * 100.0);
    println!("  │ RETRO_FAILURE_IMPACT:       {:>6} ({:.1}%)  │", retro_fail, retro_fail as f64 / total as f64 * 100.0);
    println!("  │ DUAL_FAILURE_LITHOBRAKE:    {:>6} ({:.1}%)  │", dual_fail, dual_fail as f64 / total as f64 * 100.0);
    println!("  │ LITHOBRAKING (other):       {:>6} ({:.1}%)  │", lithobraking, lithobraking as f64 / total as f64 * 100.0);
    println!("  │ TIMEOUT (still airborne):   {:>6} ({:.1}%)  │", timeouts, timeouts as f64 / total as f64 * 100.0);
    println!("  └─────────────────────────────────────────────┘");
    println!();
    println!("  Mission Success Rate:  {:.2}%", successes as f64 / total as f64 * 100.0);
    println!("  Dust Storm Rate:       {:.2}%", dust_storms as f64 / total as f64 * 100.0);
    println!();
    println!("  ┌─────────────────────────────────────────────┐");
    println!("  │ PHYSICS STATISTICS                          │");
    println!("  ├─────────────────────────────────────────────┤");
    println!("  │ Avg Final Velocity:    {:>10.2} m/s       │", avg_final_vel);
    println!("  │ Min Final Velocity:    {:>10.2} m/s       │", min_final_vel);
    println!("  │ Max Final Velocity:    {:>10.2} m/s       │", max_final_vel);
    println!("  │ Avg Flight Time:       {:>10.2} s         │", avg_flight_time);
    println!("  │ Avg Max Deceleration:  {:>10.2} g         │", avg_max_g);
    println!("  │ Peak Deceleration:     {:>10.2} g         │", max_g_overall);
    println!("  └─────────────────────────────────────────────┘");
    println!();

    let retro_fired_count = results.iter().filter(|r| r.retro_fired).count();
    let chute_deployed_count = results.iter().filter(|r| r.chute_deployed).count();
    let avg_vel_at_retro: f64 = {
        let fired: Vec<_> = results.iter().filter(|r| r.retro_fired).collect();
        if fired.is_empty() { 0.0 } else { fired.iter().map(|r| r.velocity_at_retro).sum::<f64>() / fired.len() as f64 }
    };
    let avg_alt_at_retro: f64 = {
        let fired: Vec<_> = results.iter().filter(|r| r.retro_fired).collect();
        if fired.is_empty() { 0.0 } else { fired.iter().map(|r| r.altitude_at_retro).sum::<f64>() / fired.len() as f64 }
    };

    println!("  ┌─────────────────────────────────────────────┐");
    println!("  │ EDL PHASE DIAGNOSTICS                       │");
    println!("  ├─────────────────────────────────────────────┤");
    println!("  │ Chute Deployed:        {:>6} ({:.1}%)       │", chute_deployed_count, chute_deployed_count as f64 / total as f64 * 100.0);
    println!("  │ Retro Fired:           {:>6} ({:.1}%)       │", retro_fired_count, retro_fired_count as f64 / total as f64 * 100.0);
    println!("  │ Avg Velocity at Retro: {:>10.2} m/s       │", avg_vel_at_retro);
    println!("  │ Avg Altitude at Retro: {:>10.0} m         │", avg_alt_at_retro);
    println!("  └─────────────────────────────────────────────┘");
    println!();

    println!("  ┌─────────────────────────────────────────────────────────────────────────────────┐");
    println!("  │ SAMPLE TRAJECTORIES                                                             │");
    println!("  ├──────┬──────────┬──────────┬──────────┬────────┬────────────────────────────────┤");
    println!("  │  ID  │  Alt(km) │  V₀(m/s) │  Vf(m/s) │ Result │ Proof (first 16)               │");
    println!("  ├──────┼──────────┼──────────┼──────────┼────────┼────────────────────────────────┤");
    for r in results.iter().take(5) {
        println!("  │{:>5} │ {:>8.1} │ {:>8.1} │ {:>8.1} │ {:>6} │ {}… │",
            r.id,
            r.initial_altitude / 1000.0,
            r.initial_velocity,
            r.final_velocity,
            if r.mission_success { "OK" } else { "FAIL" },
            &r.proof_hash[..32],
        );
    }
    println!("  └──────┴──────────┴──────────┴──────────┴────────┴────────────────────────────────┘");
    println!();
    println!("  SHA-256 Run Proof: {}", run_proof);
    println!();
    println!("═══════════════════════════════════════════════════════════");
    println!("  G^G MARS EDL MONTE CARLO — SEALED");
    println!("  {} trajectories × 1000Hz × SHA-256 = SOVEREIGN", total);
    println!("═══════════════════════════════════════════════════════════");
}
