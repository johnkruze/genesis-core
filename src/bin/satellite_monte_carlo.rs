// G^G Satellite Monte Carlo — THE ORBITAL TRUTH
// LEO Station-keeping, thermal dynamics drifting, and power consumption for commercial clusters (commercial constellation / Google).

use std::time::Instant;
use genesis_core::physics::orbital::{
    self, OrbitalPhysics, SatelliteState,
    ReactionWheel, ThrusterSet, RateGyro, PowerSystem,
};
use genesis_core::rng::Rng;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output::{self, DatasetMetadata, TrajectoryRecord, Dataset};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    StationKeeping,
    EclipseDesat,
    ThermalFault,
    LossOfSignal,
}

impl Phase { fn as_str(&self) -> &'static str { match self { Phase::StationKeeping => "STATION_KEEPING", Phase::EclipseDesat => "ECLIPSE_DESAT", Phase::ThermalFault => "THERMAL_FAULT", Phase::LossOfSignal => "LOSS_OF_SIGNAL" } } }

#[derive(Debug)]
#[allow(dead_code)]
struct TrajectoryResult {
    id: u32,
    short_id: String,
    persona: &'static str,
    altitude_km: f64,
    battery_end_pct: f64,
    fuel_used: f64,
    max_temp_c: f64,
    mission_success: bool,
    outcome: &'static str,
    phase: Phase,
    steps: usize,
    proof_hash: String,
    orbit_regime: &'static str,
    telemetry: Vec<serde_json::Value>,
}

fn run_single_trajectory(id: u32, rng: &mut Rng, record_telemetry: bool) -> TrajectoryResult {
    let short_id = output::short_id(rng);
    let persona = "LEO_Satellite_Constellation";
    let (orbit_regime, altitude_km) = match rng.index(3) {
        0 => ("LEO_300km", rng.range(300.0, 400.0)),
        1 => ("LEO_400km", rng.range(400.0, 500.0)),
        _ => ("LEO_500km", rng.range(500.0, 600.0)),
    };
    
    let mass = rng.range(250.0, 600.0); // e.g. mega-constellation v2 Mini
    let i_base = mass * 1.5;
    let ix = i_base * 0.8; let iy = i_base * 1.0; let iz = i_base * 0.5;

    let mut state = SatelliteState {
        position: [6378.137 + altitude_km, 0.0, 0.0],
        velocity: [0.0, (398600.4418 / (6378.137 + altitude_km)).sqrt(), 0.0],
        quaternion_attitude: [1.0, 0.0, 0.0, 0.0],
        angular_velocity: [0.001, 0.001, 0.001], // Slight drift
        inertia_tensor: [[ix, 0.0, 0.0], [0.0, iy, 0.0], [0.0, 0.0, iz]],
    };

    let mut wheels = [ReactionWheel::new(0.5, 30.0), ReactionWheel::new(0.5, 30.0), ReactionWheel::new(0.5, 30.0)];
    let initial_fuel = 15.0;
    let mut thrusters = ThrusterSet::new(5.0, 220.0, initial_fuel);
    let mut gyro = RateGyro::new(0.001, 0.0001);
    let mut power = PowerSystem::new(800.0, 1500.0);

    let orbit_period = orbital::orbital_period_s(altitude_km);
    let eclipse_frac = orbital::eclipse_fraction(altitude_km);

    let dt = 0.5;
    let max_steps = 11_000; // About 1.5 orbits
    let mut phase = Phase::StationKeeping;
    let mut step = 0;
    let mut body_temp = 15.0_f64; // Celsius
    let mut max_temp = body_temp;
    
    let thermal_fault_chance = rng.chance(0.05);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(mass);

    let mut telemetry = Vec::new();

    while step < max_steps {
        let t = step as f64 * dt;
        let orbit_phase = (t % orbit_period) / orbit_period;
        let in_sunlight = orbit_phase > eclipse_frac;

        // Thermal Dynamics
        if in_sunlight { body_temp += 0.01 * dt; } else { body_temp -= 0.02 * dt; }
        if thermal_fault_chance && step > 5000 { body_temp += 0.05 * dt; } // Runaway thermal
        max_temp = max_temp.max(body_temp);

        if !power.step(dt, in_sunlight, true) { phase = Phase::LossOfSignal; break; }

        let omega = gyro.read(&state.angular_velocity, rng, dt);
        let mut total_torque = [0.0; 3];

        if body_temp > 85.0 {
            phase = Phase::ThermalFault;
            break; // Thermal shutdown
        }

        match phase {
            Phase::StationKeeping => {
                if !in_sunlight && wheels[0].momentum.abs() > 20.0 { phase = Phase::EclipseDesat; }
                for i in 0..3 { total_torque[i] = wheels[i].command(-0.1 * omega[i], dt); }
            }
            Phase::EclipseDesat => {
                let thr_cmd = [-wheels[0].momentum * 0.1, -wheels[1].momentum * 0.1, -wheels[2].momentum * 0.1];
                let thr_tq = thrusters.fire(thr_cmd, dt);
                for i in 0..3 { total_torque[i] += thr_tq[i]; }
                if wheels[0].momentum.abs() < 2.0 { phase = Phase::StationKeeping; }
            }
            _ => break,
        }

        OrbitalPhysics::step_attitude(&mut state, &total_torque, dt);

        if step % 100 == 0 {
            proof.feed_f64(body_temp);
            if record_telemetry {
                telemetry.push(serde_json::json!({
                    "t": t, "phase": phase.as_str(), "body_temp_c": (body_temp * 10.0).round() / 10.0,
                    "battery_pct": (power.charge_fraction() * 1000.0).round() / 10.0,
                    "in_sunlight": in_sunlight, "omega_mag": orbital::omega_magnitude(&state.angular_velocity),
                }));
            }
        }
        step += 1;
    }

    let mission_success = phase == Phase::StationKeeping || phase == Phase::EclipseDesat;
    let outcome = if mission_success { "ORBIT_NOMINAL" } else { phase.as_str() };

    proof.feed_f64(max_temp);
    proof.feed_str(outcome);

    TrajectoryResult {
        id, short_id, persona, altitude_km, battery_end_pct: power.charge_fraction() * 100.0,
        fuel_used: initial_fuel - thrusters.fuel_kg, max_temp_c: max_temp,
        mission_success, outcome, phase, steps: step, proof_hash: proof.seal(), orbit_regime, telemetry,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n_trajectories: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(10_000);
    let json_output = args.iter().any(|a| a == "--json");
    let out_dir = args.iter().position(|a| a == "--out-dir").and_then(|i| args.get(i + 1)).cloned();

    let mut rng = Rng::new(0xEA7_5A7_1122_3344);
    let start = Instant::now();
    let record_telemetry = json_output || out_dir.is_some();
    let mut results = Vec::with_capacity(n_trajectories as usize);

    for i in 0..n_trajectories { results.push(run_single_trajectory(i, &mut rng, record_telemetry)); }

    if !json_output {
        let success = results.iter().filter(|r| r.mission_success).count();
        println!("  G^G SATELLITE MONTE CARLO | {} runs | {:.2}s", n_trajectories, start.elapsed().as_secs_f64());
        println!("  | NOMINAL:       {:>6} ({:>5.1}%)  |", success, success as f64 / n_trajectories as f64 * 100.0);
    }

    if let Some(base_dir) = out_dir {
        let date_str = &output::now_iso()[0..10];
        let mut grouped: std::collections::HashMap<String, Vec<&TrajectoryResult>> = std::collections::HashMap::new();

        for r in &results {
            grouped.entry(r.orbit_regime.to_string()).or_default().push(r);
        }

        let mut hash_list = Vec::new();

        for (regime_name, regime_results) in grouped {
            let dir_path = format!("{}/{}/{}", base_dir, date_str, regime_name);
            std::fs::create_dir_all(&dir_path).expect("Failed to create deeply nested data folders");

            let records: Vec<_> = regime_results.iter().map(|r| {
                TrajectoryRecord {
                    id: format!("sat_{}_{}", r.short_id, regime_name), traj_type: "leo_station_keeping".to_string(),
                    scenario: format!("{}_constellation", regime_name), steps: r.steps,
                    score: serde_json::json!({ "success": r.mission_success, "fuel_used": r.fuel_used }),
                    proof_hash: r.proof_hash.clone(), reasoning_context: serde_json::json!({ "outcome": r.outcome, "max_temp": r.max_temp_c, "orbit_regime": regime_name }),
                    data: r.telemetry.clone(),
                }
            }).collect();

            let proof_hashes: Vec<String> = regime_results.iter().map(|r| r.proof_hash.clone()).collect();
            let run_proof = proof::seal_run(&proof_hashes);
            hash_list.push(run_proof);

            let dataset = Dataset {
                dataset_metadata: DatasetMetadata {
                    generator: "G^G Satellite Monte Carlo".to_string(), domain: "orbital_commercial".to_string(),
                    scenario: "station_keeping".to_string(), trajectories: regime_results.len(),
                    physics_engine: "genesis_core::orbital+thermal".to_string(), version: "1.0.0".to_string(), generated_at: output::now_iso(),
                }, trajectories: records,
            };

            let chunk_id = output::short_id(&mut rng);
            let file_path = format!("{}/dataset_{}_{}.json", dir_path, regime_name, chunk_id);
            output::write_dataset(&file_path, &dataset).expect("Failed to write dataset");
        }
        let master_proof = proof::seal_run(&hash_list);
        if !json_output {
            println!("  SHA-256 Run Proof: {}", master_proof);
        }
    }
}
