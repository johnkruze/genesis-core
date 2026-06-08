use std::time::Instant;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output::{self, DatasetMetadata, TrajectoryRecord, Dataset};
use genesis_core::rng::Rng;
use genesis_core::physics::energy::GridSystem;

#[derive(Debug)]
#[allow(dead_code)]
struct TrajectoryResult {
    id: u32,
    short_id: String,
    survived: bool,
    blackout_time: Option<f64>,
    avg_frequency: f64,
    min_frequency: f64,
    max_frequency: f64,
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
    
    let mut grid = GridSystem::default();
    
    // Randomize grid conditions
    grid.solar_capacity = rng.range(200.0, 800.0);
    grid.wind_capacity = rng.range(100.0, 600.0);
    grid.nuclear_capacity = rng.range(600.0, 1000.0);
    grid.nuclear_output = grid.nuclear_capacity; // Baseload running at max
    grid.hydro_capacity = rng.range(100.0, 400.0);
    grid.inertia_constant = rng.range(3.0, 6.0); // MW*s/MVA
    
    let dt = 1.0; // 1s simulation step (frequency dynamics are fast)
    let max_steps = 86400; // 1 Day
    let mut step = 0;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(grid.solar_capacity);
    proof.feed_f64(grid.wind_capacity);

    let mut telemetry = Vec::new();
    let mut blackout_time = None;
    
    let mut freq_sum = 0.0;
    let mut min_freq = 60.0;
    let mut max_freq = 60.0;
    
    // Environmental factors mapping to hours of the day
    let mut time_of_day_hours = rng.range(0.0, 24.0);

    while step < max_steps {
        time_of_day_hours += dt / 3600.0;
        if time_of_day_hours >= 24.0 { time_of_day_hours -= 24.0; }
        
        // Simulating solar irradiance curve (peak at noon)
        let solar_irradiance = if time_of_day_hours > 6.0 && time_of_day_hours < 18.0 {
            ((time_of_day_hours - 12.0) / 6.0).cos().max(0.0) * rng.range(0.8, 1.0)
        } else {
            0.0
        };
        
        // Wind speed simulation (some random walk + diurnal pattern)
        let wind_speed = 8.0 + (time_of_day_hours / 24.0 * std::f64::consts::PI * 2.0).sin() * 3.0 + rng.range(-2.0, 2.0);
        
        // Demand fluctuation (peak demand in evening)
        let demand_baseline = 1000.0 + (time_of_day_hours - 18.0).powi(2) * -5.0 + 300.0;
        let demand_fluctuation = demand_baseline - 1000.0 + rng.range(-50.0, 50.0);

        grid.step(dt, solar_irradiance, wind_speed.max(0.0), demand_fluctuation);
        
        freq_sum += grid.grid_frequency;
        if grid.grid_frequency < min_freq { min_freq = grid.grid_frequency; }
        if grid.grid_frequency > max_freq { max_freq = grid.grid_frequency; }
        
        if step % 3600 == 0 { // Hourly hash
            proof.feed_f64(grid.grid_frequency);
            proof.feed_f64(grid.solar_output);
            
            if record_telemetry {
                telemetry.push(serde_json::json!({
                    "t_hours": time_of_day_hours,
                    "frequency": grid.grid_frequency,
                    "demand": grid.current_demand,
                    "generation": {
                        "nuclear": grid.nuclear_output,
                        "solar": grid.solar_output,
                        "wind": grid.wind_output,
                        "hydro": grid.hydro_output
                    }
                }));
            }
        }
        
        if grid.is_blackout() && blackout_time.is_none() {
            blackout_time = Some(step as f64 * dt);
            break; 
        }
        
        step += 1;
    }

    let survived = blackout_time.is_none();
    let avg_frequency = freq_sum / step.max(1) as f64;

    proof.feed_f64(avg_frequency);
    proof.feed_str(if survived { "NOMINAL" } else { "BLACKOUT" });
    
    TrajectoryResult {
        id,
        short_id,
        survived,
        blackout_time,
        avg_frequency,
        min_frequency: min_freq,
        max_frequency: max_freq,
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

    let mut rng = Rng::new(0x681D_E4E6_9F1E_0099);
    let start = Instant::now();
    let record_telemetry = json_output || json_path.is_some();

    let mut results = Vec::with_capacity(n_trajectories as usize);
    for i in 0..n_trajectories {
        results.push(run_single_trajectory(i, &mut rng, record_telemetry));
    }

    if json_output || json_path.is_some() {
        let records: Vec<TrajectoryRecord> = results.into_iter().map(|r| {
            TrajectoryRecord {
                id: format!("energy_grid_{}", r.short_id),
                traj_type: "grid_frequency_dynamics".to_string(),
                scenario: "24h_intermittent_renewables".to_string(),
                steps: r.steps,
                score: serde_json::json!({
                    "survived": r.survived,
                    "min_frequency": (r.min_frequency * 1000.0).round() / 1000.0,
                    "avg_frequency": (r.avg_frequency * 1000.0).round() / 1000.0,
                    "blackout_time_s": r.blackout_time,
                }),
                proof_hash: r.proof_hash.clone(),
                reasoning_context: serde_json::json!({
                    "is_anomaly": !r.survived,
                    "anomaly_type": if !r.survived { "CASCADING_FAILURE" } else { "NOMINAL" },
                }),
                data: r.telemetry,
            }
        }).collect();

        let proof_hashes: Vec<_> = records.iter().map(|r| r.proof_hash.clone()).collect();
        let run_proof = proof::seal_run(&proof_hashes);

        let dataset = Dataset {
            dataset_metadata: DatasetMetadata {
                generator: "G^G Energy Monte Carlo v1.0".to_string(),
                domain: "energy".to_string(),
                scenario: "grid_frequency_dynamics".to_string(),
                trajectories: records.len(),
                physics_engine: "genesis_core::energy (Swing Equation)".to_string(),
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
    println!("Energy Monte Carlo completed {} trajectories in {:?}", n_trajectories, start.elapsed());
    println!("SHA-256 Run Proof: {}", run_proof);
}
