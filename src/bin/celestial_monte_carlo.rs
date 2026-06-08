use std::time::Instant;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output::{self, DatasetMetadata, TrajectoryRecord, Dataset};
use genesis_core::rng::Rng;
use genesis_core::physics::ephemeris::{BodyState, NBodyState};

#[derive(Debug)]
#[allow(dead_code)]
struct TrajectoryResult {
    id: u32,
    short_id: String,
    closest_approach_km: f64,
    final_velocity_kms: f64,
    gravity_assist_delta_v: f64,
    survived: bool,
    steps: usize,
    proof_hash: String,
    telemetry: Vec<serde_json::Value>,
}

fn run_single_trajectory(
    id: u32,
    rng: &mut Rng,
    record_telemetry: bool,
) -> TrajectoryResult {
    let short_id = output::short_id(rng);
    let mut system = NBodyState::new();
    
    // Core bodies (simplified inner solar system)
    system.insert_body("Sun", BodyState {
        position: [0.0, 0.0, 0.0],
        velocity: [0.0, 0.0, 0.0],
        mu: 132712440018.0,
    });
    
    // Earth at ~1 AU
    system.insert_body("Earth", BodyState {
        position: [149597870.7, 0.0, 0.0],
        velocity: [0.0, 29.78, 0.0],
        mu: 398600.4418,
    });
    
    // Mars at ~1.5 AU
    system.insert_body("Mars", BodyState {
        position: [-100000000.0, -180000000.0, 4200000.0], // simple initial state
        velocity: [20.0, -12.0, 0.5],
        mu: 42828.375214,
    });

    // Probe (massless for physics purposes, but we give a tiny mass to compute forces if needed)
    let initial_speed = rng.range(10.0, 15.0); // km/s
    let entry_angle = rng.range(0.0, std::f64::consts::PI * 2.0);
    
    let probe_vx = initial_speed * entry_angle.cos();
    let probe_vy = initial_speed * entry_angle.sin();
    
    // Starting near Earth
    system.insert_body("Probe", BodyState {
        position: [149597870.7 + 50000.0, 50000.0, 0.0],
        velocity: [probe_vx, probe_vy, 0.0],
        mu: 1e-10, // effectively massless
    });

    let dt = 3600.0; // 1 hour step
    let max_steps = 24 * 365; // ~1 year simulation
    let mut step = 0;
    
    // NOTE (Documentation): While this is structurally an N-body gravitational simulation, 
    // it consists of only 4 bodies (Sun, Earth, Mars, Probe).
    // The Metal GPU kernel overhead far exceeds the 30-nanosecond CPU calculation.
    // The celestial domain remains natively CPU-bound for physical efficiency.

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(probe_vx);
    proof.feed_f64(probe_vy);

    let mut telemetry = Vec::new();

    let mut min_earth_dist = std::f64::MAX;
    let mut min_mars_dist = std::f64::MAX;
    let mut max_velocity = 0.0;

    let initial_velocity = (probe_vx*probe_vx + probe_vy*probe_vy).sqrt();

    while step < max_steps {
        system.step_nbody(dt);
        
        let earth = system.bodies.get("Earth").unwrap();
        let mars = system.bodies.get("Mars").unwrap();
        let probe = system.bodies.get("Probe").unwrap();

        let d_earth = ((probe.position[0]-earth.position[0]).powi(2) +
                       (probe.position[1]-earth.position[1]).powi(2) +
                       (probe.position[2]-earth.position[2]).powi(2)).sqrt();
                       
        let d_mars = ((probe.position[0]-mars.position[0]).powi(2) +
                      (probe.position[1]-mars.position[1]).powi(2) +
                      (probe.position[2]-mars.position[2]).powi(2)).sqrt();

        let v_probe = (probe.velocity[0].powi(2) + probe.velocity[1].powi(2) + probe.velocity[2].powi(2)).sqrt();
        
        if d_earth < min_earth_dist { min_earth_dist = d_earth; }
        if d_mars < min_mars_dist { min_mars_dist = d_mars; }
        if v_probe > max_velocity { max_velocity = v_probe; }

        if step % 24 == 0 { // daily hash
            proof.feed_f64(probe.position[0]);
            proof.feed_f64(v_probe);
            
            if record_telemetry {
                telemetry.push(serde_json::json!({
                    "t_days": step as f64 * dt / 86400.0,
                    "probe_pos": [probe.position[0], probe.position[1], probe.position[2]],
                    "d_mars": d_mars,
                    "v_probe": v_probe
                }));
            }
        }
        
        // Crash detection
        if min_earth_dist < 6371.0 || min_mars_dist < 3389.0 {
            break;
        }
        
        step += 1;
    }

    let probe_final = system.bodies.get("Probe").unwrap();
    let final_velocity = (probe_final.velocity[0].powi(2) + probe_final.velocity[1].powi(2) + probe_final.velocity[2].powi(2)).sqrt();
    let delta_v = final_velocity - initial_velocity; // Could be negative or positive depending on assist
    
    let survived = min_earth_dist > 6371.0 && min_mars_dist > 3389.0;
    let closest_approach = min_earth_dist.min(min_mars_dist);

    proof.feed_f64(final_velocity);
    proof.feed_f64(closest_approach);
    proof.feed_str(if survived { "SURVIVED" } else { "IMPACT" });

    TrajectoryResult {
        id,
        short_id,
        closest_approach_km: closest_approach,
        final_velocity_kms: final_velocity,
        gravity_assist_delta_v: delta_v,
        survived,
        steps: step,
        proof_hash: proof.seal(),
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

    let mut rng = Rng::new(0xDEAD_BEEF_CAFE_1234);
    let start = Instant::now();
    let record_telemetry = json_output || json_path.is_some();

    let mut results = Vec::with_capacity(n_trajectories as usize);
    for i in 0..n_trajectories {
        results.push(run_single_trajectory(i, &mut rng, record_telemetry));
    }

    if json_output || json_path.is_some() {
        let records: Vec<TrajectoryRecord> = results.into_iter().map(|r| {
            TrajectoryRecord {
                id: format!("celestial_orbit_{}", r.short_id),
                traj_type: "nbody_symplectic".to_string(),
                scenario: "gravity_assist".to_string(),
                steps: r.steps,
                score: serde_json::json!({
                    "survived": r.survived,
                    "closest_approach_km": (r.closest_approach_km * 100.0).round() / 100.0,
                    "delta_v": (r.gravity_assist_delta_v * 1000.0).round() / 1000.0,
                }),
                proof_hash: r.proof_hash.clone(),
                reasoning_context: serde_json::json!({
                    "is_anomaly": !r.survived,
                    "anomaly_type": if !r.survived { "PLANETARY_IMPACT" } else { "NOMINAL" },
                }),
                data: r.telemetry,
            }
        }).collect();

        let proof_hashes: Vec<_> = records.iter().map(|r| r.proof_hash.clone()).collect();
        let run_proof = proof::seal_run(&proof_hashes);

        let dataset = Dataset {
            dataset_metadata: DatasetMetadata {
                generator: "G^G Celestial Monte Carlo v1.0".to_string(),
                domain: "celestial".to_string(),
                scenario: "gravity_assist".to_string(),
                trajectories: records.len(),
                physics_engine: "genesis_core::ephemeris (Yoshida 4th Order Symplectic)".to_string(),
                version: "1.0.0".to_string(),
                generated_at: output::now_iso(),
            },
            trajectories: records,
        };

        if let Some(path) = &json_path {
            output::write_dataset(path, &dataset).expect("Failed to write JSON");
            eprintln!("  Written to: {}", path);
            eprintln!("  Run proof:  {}", run_proof);
        } else {
            serde_json::to_writer_pretty(std::io::stdout(), &dataset).unwrap();
        }
        return;
    }

    let proof_hashes: Vec<_> = results.iter().map(|r| r.proof_hash.clone()).collect();
    let run_proof = proof::seal_run(&proof_hashes);
    println!("Celestial Monte Carlo completed {} trajectories in {:?}", n_trajectories, start.elapsed());
    println!("SHA-256 Run Proof: {}", run_proof);
}
