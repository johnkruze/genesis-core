use std::time::Instant;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output::{self, DatasetMetadata, TrajectoryRecord, Dataset};
use genesis_core::rng::Rng;
use genesis_core::physics::rubble::{RubblePile, Boulder};
use genesis_core::taichi_bridge::{MetalPhysicsBridge, GpuBody};

#[derive(Debug)]
#[allow(dead_code)]
struct TrajectoryResult {
    id: u32,
    short_id: String,
    initial_kinetic_energy: f64,
    final_kinetic_energy: f64,
    max_dispersion: f64,
    survived: bool,
    steps: usize,
    proof_hash: String,
    telemetry: Vec<serde_json::Value>,
}

fn run_single_trajectory(
    id: u32,
    rng: &mut Rng,
    record_telemetry: bool,
    bridge: &MetalPhysicsBridge,
) -> TrajectoryResult {
    let short_id = output::short_id(rng);
    let mut pile = RubblePile::default();
    let num_boulders = rng.range(20.0, 100.0) as usize;
    
    let mut initial_ke = 0.0;
    
    // Spawn boulders in a roughly spherical cloud
    for i in 0..num_boulders {
        let r = rng.range(0.0, 5.0); // km
        let theta = rng.range(0.0, std::f64::consts::PI * 2.0);
        let phi = rng.range(0.0, std::f64::consts::PI);
        
        let x = r * phi.sin() * theta.cos();
        let y = r * phi.sin() * theta.sin();
        let z = r * phi.cos();
        
        // Slight rotation for the pile
        let vx = -y * 0.001;
        let vy = x * 0.001;
        let vz = rng.range(-0.0001, 0.0001);
        
        let mass = rng.range(1e9, 1e11); // kg
        let radius = rng.range(0.1, 0.5); // km
        
        initial_ke += 0.5 * mass * (vx*vx + vy*vy + vz*vz);

        pile.insert_boulder(&format!("B{}", i), Boulder {
            position: [x, y, z],
            velocity: [vx, vy, vz],
            mass,
            radius,
        });
    }

    let dt = 10.0; // 10s steps for dynamics
    let max_steps = 1000;
    let mut step = 0;

    // Allocate the GPU buffer and populate it with boulder data
    let tensor = bridge.allocate_orbital_tensor(num_boulders);
    let body_keys: Vec<String> = pile.boulders.keys().cloned().collect();

    // Copy boulder state TO GPU buffer (f64 → f32 for Metal)
    let mut gpu_bodies: Vec<GpuBody> = body_keys.iter().map(|k| {
        let b = pile.boulders.get(k).unwrap();
        GpuBody {
            px: b.position[0] as f32, py: b.position[1] as f32, pz: b.position[2] as f32, _p0: 0.0,
            vx: b.velocity[0] as f32, vy: b.velocity[1] as f32, vz: b.velocity[2] as f32, _v0: 0.0,
            mass: b.mass as f32, _m0: 0.0, _m1: 0.0, _m2: 0.0,
        }
    }).collect();
    bridge.write_bodies(tensor, &gpu_bodies);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(num_boulders as f64);
    proof.feed_f64(initial_ke);

    let mut telemetry = Vec::new();

    while step < max_steps {
        // GPU Compute: O(N²) N-body on Apple Silicon Metal
        bridge.dispatch_nbody_symplectic(tensor, num_boulders, dt);

        // Read GPU results back and update pile (GPU is authoritative)
        bridge.read_bodies(tensor, &mut gpu_bodies);
        for (i, key) in body_keys.iter().enumerate() {
            let b = pile.boulders.get_mut(key).unwrap();
            b.position[0] = gpu_bodies[i].px as f64;
            b.position[1] = gpu_bodies[i].py as f64;
            b.position[2] = gpu_bodies[i].pz as f64;
            b.velocity[0] = gpu_bodies[i].vx as f64;
            b.velocity[1] = gpu_bodies[i].vy as f64;
            b.velocity[2] = gpu_bodies[i].vz as f64;
        }

        if step % 100 == 0 {
            let center_boulder = pile.boulders.values().next().unwrap();
            proof.feed_f64(center_boulder.position[0]);
            proof.feed_f64(center_boulder.velocity[0]);

            if record_telemetry {
                telemetry.push(serde_json::json!({
                    "t": step as f64 * dt,
                    "boulder_count": pile.boulders.len(),
                    "center": [center_boulder.position[0], center_boulder.position[1], center_boulder.position[2]]
                }));
            }
        }

        step += 1;
    }

    bridge.free_orbital_tensor(tensor);

    // Measure final dispersion
    let mut max_dispersion = 0.0;
    let mut final_ke = 0.0;
    for b in pile.boulders.values() {
        let d = (b.position[0].powi(2) + b.position[1].powi(2) + b.position[2].powi(2)).sqrt();
        if d > max_dispersion { max_dispersion = d; }
        final_ke += 0.5 * b.mass * (b.velocity[0].powi(2) + b.velocity[1].powi(2) + b.velocity[2].powi(2));
    }

    let survived = max_dispersion < 20.0; // Is it still a cohesive pile?
    
    proof.feed_f64(max_dispersion);
    proof.feed_str(if survived { "COHESIVE" } else { "DISPERSED" });
    
    TrajectoryResult {
        id,
        short_id,
        initial_kinetic_energy: initial_ke,
        final_kinetic_energy: final_ke,
        max_dispersion,
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

    let mut rng = Rng::new(0xDEEF_1E0A_2B3C_4D5E);
    let start = Instant::now();
    let record_telemetry = json_output || json_path.is_some();
    
    let bridge = MetalPhysicsBridge::new();

    let mut results = Vec::with_capacity(n_trajectories as usize);
    for i in 0..n_trajectories {
        results.push(run_single_trajectory(i, &mut rng, record_telemetry, &bridge));
    }

    if json_output || json_path.is_some() {
        let records: Vec<TrajectoryRecord> = results.into_iter().map(|r| {
            TrajectoryRecord {
                id: format!("asteroid_neo_{}", r.short_id),
                traj_type: "neo_rubble_dynamics".to_string(),
                scenario: "impact_dispersion".to_string(),
                steps: r.steps,
                score: serde_json::json!({
                    "survived_cohesion": r.survived,
                    "max_dispersion_km": (r.max_dispersion * 100.0).round() / 100.0,
                    "energy_dissipation": r.initial_kinetic_energy - r.final_kinetic_energy,
                }),
                proof_hash: r.proof_hash.clone(),
                reasoning_context: serde_json::json!({
                    "is_anomaly": !r.survived,
                    "anomaly_type": if !r.survived { "CATASTROPHIC_DISRUPTION" } else { "NOMINAL" },
                }),
                data: r.telemetry,
            }
        }).collect();

        let proof_hashes: Vec<_> = records.iter().map(|r| r.proof_hash.clone()).collect();
        let run_proof = proof::seal_run(&proof_hashes);

        let dataset = Dataset {
            dataset_metadata: DatasetMetadata {
                generator: "G^G Asteroid Monte Carlo v1.0".to_string(),
                domain: "asteroid".to_string(),
                scenario: "rubble_pile_dynamics".to_string(),
                trajectories: records.len(),
                physics_engine: "genesis_core::rubble (O(N^2) N-Body)".to_string(),
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
    println!("Asteroid Monte Carlo completed {} trajectories in {:?}", n_trajectories, start.elapsed());
    println!("SHA-256 Run Proof: {}", run_proof);
}
