// G^G Humanoid Monte Carlo — THE TERRAN TRUTH
// Bipedal stability, slip, and substrate yielding.

use std::time::Instant;
use genesis_core::physics::terran::{
    Locomotion, RobotContact, SoilProfile, SoilType,
};
use genesis_core::rng::Rng;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output::{self, Dataset, DatasetMetadata, TrajectoryRecord};

// ─── FAILURE MODES ──────────────────────────────────────────────
#[derive(Debug, Clone, Copy)]
enum FailureMode {
    FootSlip,
    BalanceLoss,
    SoilYield,
    ActuatorFault,
}

// ─── MISSION PHASES ─────────────────────────────────────────────
#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Standing,
    Walking,
    Running,
    Recovery,
    Complete,
    Failed,
}

impl Phase {
    fn as_str(&self) -> &'static str {
        match self {
            Phase::Standing => "STANDING",
            Phase::Walking => "WALKING",
            Phase::Running => "RUNNING",
            Phase::Recovery => "RECOVERY",
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
        0 => SoilType::Sand, // High slip risk
        1 => SoilType::Loam,
        2 => SoilType::Clay, // Sticky when wet
        _ => SoilType::Andisol,
    };
    let moisture = rng.range(0.05, 0.40);
    let glomalin = rng.range(20.0, 150.0);
    let mut soil = SoilProfile {
        soil_type,
        moisture,
        glomalin_mg_g: glomalin,
        compaction: rng.range(0.0, 0.2),
        depth_layers: 10,
    };

    let persona = "Humanoid";
    let locomotion = Locomotion::Legged;
    let robot_mass = rng.range(60.0, 150.0); // Bipedal mass
    let footprint = rng.range(0.02, 0.05);   // Two feet or one foot depending on stance, avg contact area

    let robot = RobotContact {
        mass_kg: robot_mass,
        footprint_m2: footprint,
        locomotion,
    };

    // Inject failure based on soil type and random chance
    let mut failure_chance = 0.05;
    if matches!(soil.soil_type, SoilType::Sand) { failure_chance += 0.08; } // Sand slips
    if matches!(soil.soil_type, SoilType::Clay) && moisture > 0.3 { failure_chance += 0.10; } // Wet clay slips
    
    let failure = if rng.chance(failure_chance) { Some(FailureMode::FootSlip) }
    else if rng.chance(0.05) { Some(FailureMode::BalanceLoss) }
    else if rng.chance(0.02) { Some(FailureMode::SoilYield) }
    else if rng.chance(0.01) { Some(FailureMode::ActuatorFault) }
    else { None };

    let surface_pressure = robot.surface_pressure() * 2.0; // Dynamic multiplier for walking impact
    let yield_stress = soil.effective_yield_stress();

    let dt = 0.01;
    let max_steps = 5_000;
    let mut phase = Phase::Standing;
    let mut step = 0_usize;
    let mut contact_steps = 0_u32;
    let mut max_compaction = 0.0_f64;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(robot_mass);
    proof.feed_str(soil_type.as_str());

    let mut telemetry = Vec::new();

    // Telemetry specific
    let mut zmp_error = 0.0;
    let mut slip_ratio = 0.0;
    let mut stability_margin = 1.0;

    while step < max_steps {
        let t = step as f64 * dt;

        match phase {
            Phase::Standing => {
                stability_margin = 0.95;
                if t > 2.0 { phase = Phase::Walking; }
            }
            Phase::Walking => {
                contact_steps += 1;
                let (compact_inc, _) = soil.evaluate_contact(&robot);
                soil.compaction = (soil.compaction + compact_inc * dt).min(1.0);
                max_compaction = max_compaction.max(compact_inc);

                zmp_error = rng.range(-0.02, 0.02);
                slip_ratio = if matches!(soil.soil_type, SoilType::Sand) { rng.range(0.05, 0.15) } else { rng.range(0.0, 0.05) };
                
                stability_margin = 1.0 - (slip_ratio * 2.0) - zmp_error.abs() * 10.0;

                // Failure trigger logic
                if matches!(failure, Some(FailureMode::FootSlip)) && slip_ratio > 0.12 && rng.chance(0.02) {
                    phase = Phase::Recovery;
                } else if matches!(failure, Some(FailureMode::BalanceLoss)) && zmp_error.abs() > 0.018 && rng.chance(0.05) {
                    phase = Phase::Recovery;
                } else if surface_pressure > yield_stress && matches!(failure, Some(FailureMode::SoilYield)) {
                    phase = Phase::Recovery;
                }

                if contact_steps > 1500 && rng.chance(0.001) {
                    phase = Phase::Running;
                } else if contact_steps > 3000 {
                    phase = Phase::Complete;
                }
            }
            Phase::Running => {
                contact_steps += 1;
                zmp_error = rng.range(-0.05, 0.05);
                slip_ratio = rng.range(0.1, 0.25);
                stability_margin = 1.0 - (slip_ratio * 1.5) - zmp_error.abs() * 8.0;

                if matches!(failure, Some(FailureMode::FootSlip)) && slip_ratio > 0.2 && rng.chance(0.05) {
                    phase = Phase::Recovery;
                } else if contact_steps > 4000 {
                    phase = Phase::Walking;
                }
            }
            Phase::Recovery => {
                // Attempt to regain balance
                stability_margin += 0.05;
                if stability_margin < 0.2 || rng.chance(0.05) {
                    phase = Phase::Failed; // Fall down
                } else if stability_margin > 0.8 {
                    phase = Phase::Standing;
                }
            }
            Phase::Complete | Phase::Failed => break,
        }

        if step % 50 == 0 {
            proof.feed_f64(stability_margin);
            if record_telemetry {
                telemetry.push(serde_json::json!({
                    "t": t,
                    "phase": phase.as_str(),
                    "zmp_error_m": (zmp_error * 1000.0).round() / 1000.0,
                    "slip_ratio": (slip_ratio * 100.0).round() / 100.0,
                    "stability_margin": (stability_margin * 100.0).round() / 100.0,
                    "compaction": (soil.compaction * 1000.0).round() / 1000.0,
                }));
            }
        }
        step += 1;
    }

    let mission_success = phase == Phase::Complete;
    let outcome = if phase == Phase::Failed {
        match failure {
            Some(FailureMode::FootSlip) => "FALL_SLIP",
            Some(FailureMode::BalanceLoss) => "FALL_BALANCE",
            Some(FailureMode::SoilYield) => "FALL_SOIL_YIELD",
            Some(FailureMode::ActuatorFault) => "ACTUATOR_FAULT",
            None => "UNKNOWN_FALL",
        }
    } else {
        "LOCOMOTION_NOMINAL"
    };

    proof.feed_str(outcome);
    let proof_hash = proof.seal();

    TrajectoryResult {
        id, short_id, persona, robot_mass, footprint, locomotion, soil_type,
        moisture: soil.moisture, glomalin, surface_pressure, yield_stress,
        max_compaction, mission_success, outcome, phase, steps: step,
        proof_hash, failure, telemetry,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n_trajectories: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(10_000);
    let json_output = args.iter().any(|a| a == "--json");
    let out_dir = args.iter().position(|a| a == "--out-dir").and_then(|i| args.get(i + 1)).cloned();

    if !json_output {
        println!("====================================================================");
        println!("  G^G HUMANOID MONTE CARLO — BIPEDAL DYNAMICS");
        println!("====================================================================");
    }

    let mut rng = Rng::new(0xB19E_D00D_BAAD_F00D);
    let start = Instant::now();
    let record_telemetry = json_output || out_dir.is_some();
    let mut results = Vec::with_capacity(n_trajectories as usize);

    for i in 0..n_trajectories {
        results.push(run_single_trajectory(i, &mut rng, record_telemetry));
    }

    let elapsed = start.elapsed();

    if !json_output {
        let total = results.len();
        let success = results.iter().filter(|r| r.mission_success).count();
        let falls = total - success;
        println!("  Elapsed: {:.3}s | {} Trajectories", elapsed.as_secs_f64(), total);
        println!("  | NOMINAL:       {:>6} ({:>5.1}%)  |", success, success as f64 / total as f64 * 100.0);
        println!("  | FALLS:         {:>6} ({:>5.1}%)  |", falls, falls as f64 / total as f64 * 100.0);
    }

    if let Some(base_dir) = out_dir {
        let date_str = &output::now_iso()[0..10]; // YYYY-MM-DD
        let mut grouped: std::collections::HashMap<String, Vec<&TrajectoryResult>> = std::collections::HashMap::new();

        for r in &results {
            grouped.entry(r.soil_type.as_str().to_string()).or_default().push(r);
        }

        let mut hash_list = Vec::new();

        for (soil_str, cond_results) in grouped {
            let dir_path = format!("{}/{}/{}", base_dir, date_str, soil_str);
            std::fs::create_dir_all(&dir_path).expect("Failed to create deeply nested data folders");

            let trajectory_records: Vec<_> = cond_results.iter().map(|r| {
                TrajectoryRecord {
                    id: format!("humanoid_{}_{}", r.short_id, soil_str),
                    traj_type: "bipedal_locomotion".to_string(),
                    scenario: format!("{}_humanoid", soil_str),
                    steps: r.steps,
                    score: serde_json::json!({
                        "success": r.mission_success,
                        "max_compaction": (r.max_compaction * 1000.0).round() / 1000.0,
                    }),
                    proof_hash: r.proof_hash.clone(),
                    reasoning_context: serde_json::json!({
                        "anomaly": !r.mission_success,
                        "outcome": r.outcome,
                        "mass_kg": r.robot_mass,
                        "soil": soil_str,
                    }),
                    data: r.telemetry.clone(),
                }
            }).collect();

            let proof_hashes: Vec<String> = cond_results.iter().map(|r| r.proof_hash.clone()).collect();
            let run_proof = proof::seal_run(&proof_hashes);
            hash_list.push(run_proof);
            
            let dataset = Dataset {
                dataset_metadata: DatasetMetadata {
                    generator: "G^G Humanoid Monte Carlo".to_string(),
                    domain: "terran_bipedal".to_string(),
                    scenario: "locomotion_stability".to_string(),
                    trajectories: cond_results.len(),
                    physics_engine: "genesis_core::terran".to_string(),
                    version: "1.0.0".to_string(),
                    generated_at: output::now_iso(),
                },
                trajectories: trajectory_records,
            };
            
            let chunk_id = output::short_id(&mut rng);
            let file_path = format!("{}/dataset_{}_{}.json", dir_path, soil_str, chunk_id);

            output::write_dataset(&file_path, &dataset).expect("Failed to write dataset");
        }
        let master_proof = proof::seal_run(&hash_list);
        if !json_output {
            println!("  SHA-256 Run Proof: {}", master_proof);
        }
    }
}
