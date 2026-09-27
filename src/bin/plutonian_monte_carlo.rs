use std::time::Instant;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output::{self, DatasetMetadata, TrajectoryRecord, Dataset};
use genesis_core::rng::Rng;
use genesis_core::physics::plutonian::{PlutonianCore, PhaseState};

#[derive(Debug)]
#[allow(dead_code)]
struct TrajectoryResult {
    id: u32,
    short_id: String,
    phase_shifts: u32,
    final_avg_entropy: f64,
    final_avg_coherence: f64,
    deep_time_survived: bool,
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
    
    let mut core = PlutonianCore::default();
    
    // Configure deep time params
    core.base_decay_rate = rng.range(0.00005, 0.0005);
    core.phase_shift_threshold = rng.range(0.75, 0.95);
    core.temporal_compression = rng.range(0.5, 2.0);
    
    let num_structures = rng.range(5.0, 50.0) as usize;
    
    for i in 0..num_structures {
        core.insert_node(&format!("Structure{}", i), PhaseState {
            energy_level: rng.range(0.6, 1.0),
            coherence: rng.range(0.5, 1.0),
            entropy: rng.range(0.0, 0.3),
        });
    }
    
    let dt_years = 100.0; // Century steps
    let max_steps = 10_000; // 1 Million Years
    let mut step = 0;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(core.base_decay_rate);
    proof.feed_f64(core.temporal_compression);

    let mut telemetry = Vec::new();
    let mut total_phase_shifts = 0;

    while step < max_steps {
        let shifted = core.step_time(dt_years);
        if shifted {
            total_phase_shifts += 1;
        }
        
        if step % 500 == 0 { // Record every 50,000 years
            let mut avg_e = 0.0;
            let mut avg_coh = 0.0;
            let mut avg_ent = 0.0;
            
            for state in core.nodes.values() {
                avg_e += state.energy_level;
                avg_coh += state.coherence;
                avg_ent += state.entropy;
            }
            
            let n = core.nodes.len() as f64;
            avg_e /= n;
            avg_coh /= n;
            avg_ent /= n;
            
            proof.feed_f64(avg_coh);
            proof.feed_f64(avg_ent);
            
            if record_telemetry {
                telemetry.push(serde_json::json!({
                    "t_years": step as f64 * dt_years,
                    "avg_energy": avg_e,
                    "avg_coherence": avg_coh,
                    "avg_entropy": avg_ent,
                    "phase_shifts": total_phase_shifts
                }));
            }
        }
        
        step += 1;
    }

    let mut final_ent = 0.0;
    let mut final_coh = 0.0;
    for state in core.nodes.values() {
        final_ent += state.entropy;
        final_coh += state.coherence;
    }
    let n = core.nodes.len() as f64;
    final_ent /= n;
    final_coh /= n;

    // Survival implies it didn't devolve into pure noise
    let deep_time_survived = final_coh > 0.2; 
    
    proof.feed_f64(final_ent);
    proof.feed_str(if deep_time_survived { "CRYSTALLIZED" } else { "DECAYED" });
    
    TrajectoryResult {
        id,
        short_id,
        phase_shifts: total_phase_shifts,
        final_avg_entropy: final_ent,
        final_avg_coherence: final_coh,
        deep_time_survived,
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

    let mut rng = Rng::new(0x701D_DECA_C048_0001);
    let start = Instant::now();
    let record_telemetry = json_output || json_path.is_some();

    let mut results = Vec::with_capacity(n_trajectories as usize);
    for i in 0..n_trajectories {
        results.push(run_single_trajectory(i, &mut rng, record_telemetry));
    }

    if json_output || json_path.is_some() {
        let records: Vec<TrajectoryRecord> = results.into_iter().map(|r| {
            TrajectoryRecord {
                id: format!("plutonian_decay_{}", r.short_id),
                traj_type: "deep_time_substrate".to_string(),
                scenario: "1M_year_phase_shift".to_string(),
                steps: r.steps,
                score: serde_json::json!({
                    "survived": r.deep_time_survived,
                    "phase_shifts": r.phase_shifts,
                    "final_entropy": (r.final_avg_entropy * 1000.0).round() / 1000.0,
                    "final_coherence": (r.final_avg_coherence * 1000.0).round() / 1000.0,
                }),
                proof_hash: r.proof_hash.clone(),
                reasoning_context: serde_json::json!({
                    "is_anomaly": !r.deep_time_survived,
                    "anomaly_type": if !r.deep_time_survived { "ABSOLUTE_DECAY" } else { "NOMINAL" },
                }),
                data: r.telemetry,
            }
        }).collect();

        let proof_hashes: Vec<_> = records.iter().map(|r| r.proof_hash.clone()).collect();
        let run_proof = proof::seal_run(&proof_hashes);

        let dataset = Dataset {
            dataset_metadata: DatasetMetadata {
                generator: "G^G Plutonian Monte Carlo v1.0".to_string(),
                domain: "plutonian".to_string(),
                scenario: "deep_time_decay".to_string(),
                trajectories: records.len(),
                physics_engine: "genesis_core::plutonian (Phase State Integrator)".to_string(),
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
    println!("Plutonian Monte Carlo completed {} trajectories in {:?}", n_trajectories, start.elapsed());
    println!("SHA-256 Run Proof: {}", run_proof);
}
