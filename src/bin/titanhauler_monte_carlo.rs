// G^G Titanhauler Monte Carlo — THE TERRAN TRUTH
// At what mass does the substrate reject the body?

use std::time::Instant;
use genesis_core::physics::terran::{
    self, Locomotion, RobotContact, SoilProfile, SoilType,
};
use genesis_core::rng::Rng;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output::{self, Dataset, DatasetMetadata, TrajectoryRecord};

// ─── FAILURE MODES ──────────────────────────────────────────────
#[derive(Debug, Clone, Copy)]
enum FailureMode {
    SoilCollapse,
    MoistureSpike,
    RobotStuck,
    TrackSnap, // New failure mode for Titanhauler
}

// ─── MISSION PHASES ─────────────────────────────────────────────
#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Approach,
    Contact,
    Hauling,
    Withdrawal,
    Complete,
    Failed,
}

impl Phase {
    fn as_str(&self) -> &'static str {
        match self {
            Phase::Approach => "APPROACH",
            Phase::Contact => "CONTACT",
            Phase::Hauling => "HAULING",
            Phase::Withdrawal => "WITHDRAWAL",
            Phase::Complete => "COMPLETE",
            Phase::Failed => "FAILED",
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
    robot_mass: f64,
    footprint: f64,
    locomotion: Locomotion,
    soil_type: SoilType,
    moisture: f64,
    glomalin: f64,
    surface_pressure: f64,
    yield_stress: f64,
    max_compaction: f64,
    compaction_depth: f64,
    mission_success: bool,
    outcome: &'static str,
    phase: Phase,
    steps: usize,
    proof_hash: String,
    failure: Option<FailureMode>,
    telemetry: Vec<serde_json::Value>,
}

fn run_single_trajectory(
    id: u32,
    rng: &mut Rng,
    record_telemetry: bool,
) -> TrajectoryResult {
    let short_id = output::short_id(rng);

    // Randomize soil
    let soil_type = match rng.index(4) {
        0 => SoilType::Sand,
        1 => SoilType::Loam,
        2 => SoilType::Clay,
        _ => SoilType::Andisol,
    };
    let moisture = rng.range(0.05, 0.40);
    // Mycelial life strengthens the soil - let's see if it can hold a Titanhauler
    let glomalin = rng.range(20.0, 150.0); // mg/g
    let mut soil = SoilProfile {
        soil_type,
        moisture,
        glomalin_mg_g: glomalin,
        compaction: rng.range(0.0, 0.3),
        depth_layers: 20,
    };

    // TITANHAULER SPECIFIC
    let persona = "Titanhauler";
    let locomotion = Locomotion::Tracked;
    // Mass: 20,000 kg to 200_000 kg
    let robot_mass = rng.range(20_000.0, 200_000.0);
    // Footprint: massive tracks
    let footprint = rng.range(10.0, 50.0);

    let robot = RobotContact {
        mass_kg: robot_mass,
        footprint_m2: footprint,
        locomotion,
    };

    // Inject failure
    let failure = if rng.chance(0.12) { Some(FailureMode::SoilCollapse) }
    else if rng.chance(0.15) { Some(FailureMode::MoistureSpike) }
    else if rng.chance(0.08) { Some(FailureMode::RobotStuck) }
    else if rng.chance(0.05) { Some(FailureMode::TrackSnap) }
    else { None };

    // Apply failure modes pre-computation
    match failure {
        Some(FailureMode::SoilCollapse) => { soil.compaction = 0.9; }
        Some(FailureMode::MoistureSpike) => { soil.moisture = rng.range(0.60, 0.95); }
        _ => {}
    }

    let surface_pressure = robot.surface_pressure();
    let yield_stress = soil.effective_yield_stress();

    let dt = 0.01;
    let max_steps = 15_000;
    let mut phase = Phase::Approach;
    let mut step = 0_usize;
    let mut contact_steps = 0_u32;
    let mut max_compaction = 0.0_f64;
    let mut compaction_depth = 0.0_f64;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(robot_mass);
    proof.feed_f64(glomalin);
    proof.feed_str(soil_type.as_str());

    let mut telemetry = Vec::new();

    while step < max_steps {
        let t = step as f64 * dt;

        match phase {
            Phase::Approach => {
                if t > 5.0 {
                    phase = Phase::Contact;
                }
            }
            Phase::Contact => {
                contact_steps += 1;

                let (compact_inc, depth) = soil.evaluate_contact(&robot);
                soil.compaction = (soil.compaction + compact_inc * dt).min(1.0);
                if compact_inc > max_compaction {
                    max_compaction = compact_inc;
                    compaction_depth = depth;
                }

                let rainfall = if rng.chance(0.02) { rng.range(0.0, 0.01) } else { 0.0 };
                terran::moisture_step(&mut soil, rainfall, dt);

                if matches!(failure, Some(FailureMode::RobotStuck)) && soil.compaction > 0.6 {
                    phase = Phase::Failed;
                    continue;
                }
                if matches!(failure, Some(FailureMode::TrackSnap)) && surface_pressure > yield_stress * 1.5 {
                    if step > 2000 && rng.chance(0.01) {
                        phase = Phase::Failed;
                        continue;
                    }
                }

                // Haul for a bit
                if contact_steps > 3000 {
                    phase = Phase::Hauling;
                }
            }
            Phase::Hauling => {
                // Moving forward under massive load
                contact_steps += 1;
                let (compact_inc, _) = soil.evaluate_contact(&robot);
                soil.compaction = (soil.compaction + compact_inc * dt * 0.5).min(1.0); // Continual compaction

                if soil.compaction >= 1.0 {
                    phase = Phase::Failed; // Catastrophic failure
                    continue;
                }

                if contact_steps > 10000 {
                    phase = Phase::Withdrawal;
                }
            }
            Phase::Withdrawal => {
                if step > (contact_steps as usize + 500) {
                    phase = Phase::Complete;
                }
            }
            Phase::Complete | Phase::Failed => break,
        }

        if step % 100 == 0 {
            proof.feed_f64(soil.compaction);
            proof.feed_f64(soil.moisture);

            if record_telemetry {
                telemetry.push(serde_json::json!({
                    "t": t,
                    "phase": phase.as_str(),
                    "compaction": (soil.compaction * 1000.0).round() / 1000.0,
                    "moisture": (soil.moisture * 1000.0).round() / 1000.0,
                    "pressure_pa": (surface_pressure * 10.0).round() / 10.0,
                    "yield_pa": (yield_stress * 10.0).round() / 10.0,
                }));
            }
        }
        step += 1;
    }

    let mission_success = phase == Phase::Complete;
    let outcome = if phase == Phase::Failed {
        if matches!(failure, Some(FailureMode::RobotStuck)) { "BOGGED_DOWN" }
        else if matches!(failure, Some(FailureMode::TrackSnap)) { "TRACK_SNAPPED" }
        else if soil.compaction >= 1.0 { "CATASTROPHIC_COMPACTION" }
        else { "SOIL_COLLAPSE" }
    } else {
        "HAUL_COMPLETE"
    };

    proof.feed_f64(soil.compaction);
    proof.feed_str(outcome);
    let proof_hash = proof.seal();

    TrajectoryResult {
        id,
        short_id,
        persona,
        robot_mass,
        footprint,
        locomotion,
        soil_type,
        moisture: soil.moisture,
        glomalin,
        surface_pressure,
        yield_stress,
        max_compaction,
        compaction_depth,
        mission_success,
        outcome,
        phase,
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
    let out_dir = args.iter().position(|a| a == "--out-dir")
        .and_then(|i| args.get(i + 1))
        .cloned();

    if !json_output {
        println!("====================================================================");
        println!("  G^G TITANHAULER MONTE CARLO — THE EXTREME TERRAN TRUTH");
        println!("  At what mass does the substrate reject the industrial behemoth?");
        println!("====================================================================");
        println!();
        println!("  Trajectories:  {}", n_trajectories);
        println!("  Robots:        Titanhaulers [20,000kg - 200,000kg]");
        println!("  Biology:       Glomalin [20-150 mg/g] shifts yield stress");
        println!("====================================================================");
        println!();
    }

    let mut rng = Rng::new(0x8A11_F900_9011_BEEF);
    let start = Instant::now();
    let record_telemetry = json_output || out_dir.is_some();

    let mut results: Vec<TrajectoryResult> = Vec::with_capacity(n_trajectories as usize);
    for i in 0..n_trajectories {
        let r = run_single_trajectory(i, &mut rng, record_telemetry);
        results.push(r);

        if !json_output && (i + 1) % 1000 == 0 {
            let elapsed = start.elapsed().as_secs_f64();
            let rate = (i + 1) as f64 / elapsed;
            eprint!("\r  [{}/{}] {:.0} traj/sec", i + 1, n_trajectories, rate);
        }
    }
    if !json_output { eprintln!(); }

    let elapsed = start.elapsed();

    // === DIAGNOSTIC OUTPUT ===
    if !json_output {
        let total = results.len();
        let success = results.iter().filter(|r| r.mission_success).count();
        let bogged = results.iter().filter(|r| r.outcome == "BOGGED_DOWN").count();
        let track_snap = results.iter().filter(|r| r.outcome == "TRACK_SNAPPED").count();
        let collapsed = results.iter().filter(|r| r.outcome == "SOIL_COLLAPSE" || r.outcome == "CATASTROPHIC_COMPACTION").count();

        println!("====================================================================");
        println!("  RESULTS — TITANHAULER TRUTH");
        println!("====================================================================");
        println!();
        println!("  Total Trajectories:    {}", total);
        println!("  Elapsed:               {:.3}s", elapsed.as_secs_f64());
        println!();
        println!("  | HAUL_COMPLETE:         {:>6} ({:>5.1}%)  |", success, success as f64 / total as f64 * 100.0);
        println!("  | BOGGED_DOWN:           {:>6} ({:>5.1}%)  |", bogged, bogged as f64 / total as f64 * 100.0);
        println!("  | TRACK_SNAPPED:         {:>6} ({:>5.1}%)  |", track_snap, track_snap as f64 / total as f64 * 100.0);
        println!("  | SOIL_COLLAPSE:         {:>6} ({:>5.1}%)  |", collapsed, collapsed as f64 / total as f64 * 100.0);
        println!();
    }

    // === JSON / FOLDER OUTPUT ===
    if let Some(base_dir) = out_dir {
        let date_str = &output::now_iso()[0..10]; // YYYY-MM-DD
        let mut grouped: std::collections::HashMap<String, Vec<&TrajectoryResult>> = std::collections::HashMap::new();

        for r in &results {
            let soil_str = r.soil_type.as_str().to_string();
            grouped.entry(soil_str).or_default().push(r);
        }

        for (soil_name, soil_results) in grouped {
            let dir_path = format!("{}/{}/{}", base_dir, date_str, soil_name);
            std::fs::create_dir_all(&dir_path).expect("Failed to create deeply nested data folders");

            // Build single dataset per soil type representing the sweep subset
            let trajectory_records: Vec<TrajectoryRecord> = soil_results.iter().map(|r| {
                TrajectoryRecord {
                    id: format!("titanhauler_{}_{}", r.short_id, soil_name),
                    traj_type: "heavy_haul_simulation".to_string(),
                    scenario: format!("{}_titanhauler", soil_name),
                    steps: r.steps,
                    score: serde_json::json!({
                        "mission_success": r.mission_success,
                        "compaction": (r.max_compaction * 1000.0).round() / 1000.0,
                        "pressure_yield_ratio": if r.yield_stress > 0.0 {
                            (r.surface_pressure / r.yield_stress * 100.0).round() / 100.0
                        } else { 0.0 },
                    }),
                    proof_hash: r.proof_hash.clone(),
                    reasoning_context: serde_json::json!({
                        "is_anomaly": !r.mission_success,
                        "anomaly_type": r.outcome,
                        "snapshot": {
                            "mass_kg": (r.robot_mass * 10.0).round() / 10.0,
                            "footprint_m2": (r.footprint * 1000.0).round() / 1000.0,
                            "soil": soil_name,
                            "glomalin_mg_g": (r.glomalin * 10.0).round() / 10.0,
                            "failure": format!("{:?}", r.failure),
                        },
                    }),
                    data: r.telemetry.clone(),
                }
            }).collect();

            let proof_hashes: Vec<String> = soil_results.iter().map(|r| r.proof_hash.clone()).collect();
            let run_proof = proof::seal_run(&proof_hashes);

            let dataset = Dataset {
                dataset_metadata: DatasetMetadata {
                    generator: "G^G Titanhauler Monte Carlo".to_string(),
                    domain: "terran_industrial".to_string(),
                    scenario: "heavy_haul".to_string(),
                    trajectories: soil_results.len(),
                    physics_engine: "genesis_core::terran (Boussinesq 100Hz scaled for extreme mass)".to_string(),
                    version: "1.0.0".to_string(),
                    generated_at: output::now_iso(),
                },
                trajectories: trajectory_records,
            };

            // E.g. dataset_titanhauler_sand_<hash>.json
            let chunk_id = output::short_id(&mut rng);
            let file_path = format!("{}/dataset_{}_{}.json", dir_path, soil_name, chunk_id);

            output::write_dataset(&file_path, &dataset).expect("Failed to write JSON fragment");
            eprintln!("  Written {} trajectories to: {}", soil_results.len(), file_path);
            eprintln!("  Fragment proof: {}", run_proof);
        }
    }
}
