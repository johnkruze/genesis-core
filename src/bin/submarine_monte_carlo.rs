// G^G Submarine Monte Carlo — THE MARINE TRUTH
// Deep-sea pressure containment, variable ballast, and GPS-denied navigation.

use std::time::Instant;
use genesis_core::physics::marine::*;
use genesis_core::rng::Rng;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output::{self, DatasetMetadata, TrajectoryRecord, Dataset};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase { DeepDive, SeabedSurvey, EmergencyAscent, Surfaced }
impl Phase { fn as_str(&self) -> &'static str { match self { Phase::DeepDive => "DEEP_DIVE", Phase::SeabedSurvey => "SEABED_SURVEY", Phase::EmergencyAscent => "EMERGENCY_ASCENT", Phase::Surfaced => "SURFACED" } } }

#[derive(Debug)]
#[allow(dead_code)]
struct TrajectoryResult {
    id: u32,
    short_id: String,
    persona: &'static str,
    mission_success: bool,
    max_depth: f64,
    battery_used_pct: f64,
    pressure_breach: bool,
    outcome: &'static str,
    phase: Phase,
    steps: usize,
    proof_hash: String,
    dive_zone: &'static str,
    telemetry: Vec<serde_json::Value>,
}

fn run_single_trajectory(id: u32, rng: &mut Rng, record_telemetry: bool) -> TrajectoryResult {
    let short_id = output::short_id(rng);
    let persona = "Deep_Blue_Submarine";
    let physics = MarinePhysics::default();
    
    let mass = rng.range(2000.0, 5000.0);
    let volume = mass / RHO_SEAWATER * 1.01; // Positively buoyant natively
    let mut auv = AuvModel {
        mass, volume, drag_area: 0.5, cd: 0.8, max_thrust: 3000.0,
        n_thrusters: 4, battery_wh: 10_000.0, battery_remaining: 10_000.0, reserve_fraction: 0.1,
    };

    // Depth Regimes
    let (dive_zone, target_depth, crush_depth) = match rng.index(3) {
        0 => ("Midnight_Zone", rng.range(1000.0, 3000.0), rng.range(3500.0, 4500.0)),
        1 => ("Abyssal_Zone", rng.range(3000.0, 6000.0), rng.range(6500.0, 7500.0)),
        _ => ("Hadal_Zone", rng.range(6000.0, 9000.0), rng.range(9500.0, 10500.0)),
    };

    let mut pos = [0.0, 0.0, -10.0];
    let mut vel = [0.0; 3];
    let max_mission_time = 3600.0 * 2.0; // 2 hour dive
    
    let dt = 0.5;
    let max_steps = (max_mission_time / dt) as usize;
    let mut step = 0;
    
    let mut phase = Phase::DeepDive;
    let mut max_reached_depth = 10.0_f64;
    let mut pressure_breach = false;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(mass);

    let mut telemetry = Vec::new();

    while step < max_steps {
        let depth = -pos[2];
        if depth > max_reached_depth { max_reached_depth = depth; }

        let external_pa = depth * RHO_SEAWATER * physics.gravity;
        
        if depth > crush_depth {
            pressure_breach = true;
            phase = Phase::EmergencyAscent;
            break; // Catastrophic implosion
        }

        let mut thrust = [0.0; 3];
        match phase {
            Phase::DeepDive => {
                if depth < target_depth { thrust[2] = -auv.max_thrust * 0.8; }
                else { phase = Phase::SeabedSurvey; }
            }
            Phase::SeabedSurvey => {
                thrust[0] = auv.max_thrust * 0.5; // Moving forward
                // Depth keeping
                let error = target_depth - depth;
                thrust[2] = (error * 10.0).clamp(-auv.max_thrust, auv.max_thrust);
                
                if step > 5000 { phase = Phase::EmergencyAscent; }
            }
            Phase::EmergencyAscent => {
                thrust[2] = auv.max_thrust; // Full up
                if depth <= 1.0 { phase = Phase::Surfaced; break; }
            }
            _ => break,
        }

        let power = auv.thrust_power(thrust[2].abs() + thrust[0].abs());
        if !auv.consume_energy(power + 20.0, dt) {
            phase = Phase::EmergencyAscent; // Battery low, must surface
            thrust[2] = auv.max_thrust;
            if auv.battery_remaining <= 0.0 { thrust[2] = 0.0; } // Dead
        }

        let buoyancy = physics.buoyancy(auv.volume);
        let weight = auv.mass * physics.gravity;
        let force_z = buoyancy - weight + thrust[2];
        
        // Very basic vertical/forward integration
        vel[2] += (force_z / auv.mass) * dt;
        vel[2] *= 0.95; // drag
        pos[2] += vel[2] * dt;
        vel[0] += (thrust[0] / auv.mass) * dt; vel[0] *= 0.8; pos[0] += vel[0]*dt;
        
        if pos[2] > 0.0 { pos[2] = 0.0; vel[2] = 0.0; phase = Phase::Surfaced; }

        if step % 2 == 0 {
            proof.feed_f64(depth);
            if record_telemetry {
                telemetry.push(serde_json::json!({
                    "t": step as f64 * dt, "phase": phase.as_str(),
                    "depth_m": (depth * 10.0).round() / 10.0,
                    "external_pressure_mpa": (external_pa / 1_000_000.0 * 100.0).round() / 100.0,
                    "battery_pct": (auv.battery_fraction() * 100.0).round() / 100.0,
                }));
            }
        }
        step += 1;
    }

    let mission_success = phase == Phase::Surfaced && !pressure_breach;
    let outcome = if pressure_breach { "HULL_CRUSHED" } else { phase.as_str() };

    proof.feed_f64(max_reached_depth);
    proof.feed_str(outcome);

    TrajectoryResult {
        id, short_id, persona, mission_success, max_depth: max_reached_depth,
        battery_used_pct: (1.0 - auv.battery_fraction()) * 100.0,
        pressure_breach, outcome, phase, steps: step,
        proof_hash: proof.seal(), dive_zone, telemetry,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n_trajectories: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(10_000);
    let json_output = args.iter().any(|a| a == "--json");
    let out_dir = args.iter().position(|a| a == "--out-dir").and_then(|i| args.get(i + 1)).cloned();

    let mut rng = Rng::new(0xDEE9_5EAA_5EAA_0000);
    let start = Instant::now();
    let mut results = Vec::with_capacity(n_trajectories as usize);
    let record_telemetry = json_output || out_dir.is_some();

    for i in 0..n_trajectories { results.push(run_single_trajectory(i, &mut rng, record_telemetry)); }

    if !json_output {
        let success = results.iter().filter(|r| r.mission_success).count();
        println!("  G^G SUBMARINE MONTE CARLO | {} runs | {:.2}s", n_trajectories, start.elapsed().as_secs_f64());
        println!("  | SURFACED (NOMINAL):{:>6} ({:>5.1}%)  |", success, success as f64 / n_trajectories as f64 * 100.0);
    }

    if let Some(base_dir) = out_dir {
        let date_str = &output::now_iso()[0..10];
        let mut grouped: std::collections::HashMap<String, Vec<&TrajectoryResult>> = std::collections::HashMap::new();

        for r in &results {
            grouped.entry(r.dive_zone.to_string()).or_default().push(r);
        }

        let mut hash_list = Vec::new();

        for (zone_name, zone_results) in grouped {
            let dir_path = format!("{}/{}/{}", base_dir, date_str, zone_name);
            std::fs::create_dir_all(&dir_path).expect("Failed to create deeply nested data folders");

            let records: Vec<_> = zone_results.iter().map(|r| {
                TrajectoryRecord {
                    id: format!("sub_{}_{}", r.short_id, zone_name), traj_type: "deep_sea_dive".to_string(),
                    scenario: format!("{}_exploration", zone_name), steps: r.steps,
                    score: serde_json::json!({ "success": r.mission_success, "max_depth": r.max_depth }),
                    proof_hash: r.proof_hash.clone(), reasoning_context: serde_json::json!({ "outcome": r.outcome, "dive_zone": zone_name }),
                    data: r.telemetry.clone(),
                }
            }).collect();

            let proof_hashes: Vec<String> = zone_results.iter().map(|r| r.proof_hash.clone()).collect();
            let run_proof = proof::seal_run(&proof_hashes);
            hash_list.push(run_proof);

            let dataset = Dataset {
                dataset_metadata: DatasetMetadata {
                    generator: "G^G Submarine Monte Carlo".to_string(), domain: "marine_subsurface".to_string(),
                    scenario: "autonomous_deep_dive".to_string(), trajectories: zone_results.len(),
                    physics_engine: "genesis_core::marine_buoyancy".to_string(), version: "1.0.0".to_string(), generated_at: output::now_iso(),
                }, trajectories: records,
            };

            let chunk_id = output::short_id(&mut rng);
            let file_path = format!("{}/dataset_{}_{}.json", dir_path, zone_name, chunk_id);
            output::write_dataset(&file_path, &dataset).expect("Failed to write dataset");
        }
        let master_proof = proof::seal_run(&hash_list);
        if !json_output {
            println!("  SHA-256 Run Proof: {}", master_proof);
        }
    }
}
