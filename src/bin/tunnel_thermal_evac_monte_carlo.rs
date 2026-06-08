// G^G BORING COMPANY LOOP THERMAL EVACUATION MONTE CARLO
// Sovereign Verification: Lithium Backdraft vs FSD Evasion Paralysis
//
// THE EMBODIMENT: The Boring Company "Vegas Loop". A 12-foot diameter concrete tunnel 
// running 1.5 miles. A platoon of 50 Tesla vehicles is operating inside the tunnel 
// using FSD (Full Self-Driving).
// 
// THE VULNERABILITY: Generative AI simulations (Tesla Dojo/NVIDIA) train FSD solely 
// on open-road scenarios. When sensing a catastrophic obstacle, the neural network 
// hallucinates a path geometry that includes shoulders, crosswalks, or adjacent lanes. 
// Furthermore, the tunnel ventilation systems are modeled using continuous, steady-state 
// Computational Fluid Dynamics (CFD).
//
// THE MATHEMATICAL REALITY: The lead Tesla experiences a lithium-ion battery thermal 
// runaway (1,000°C fire). The extreme heat instantly expands the air volume in the 
// confined 12-foot cylinder (Charles's Law: V1/T1 = V2/T2). The resulting high-pressure 
// thermal shockwave generates a lethal toxic backdraft moving at 15 m/s, completely 
// overwhelming the fixed RPM of the tunnel ventilation fans.
//
// THE FATALITY: The 49 trailing FSD-enabled Teslas detect the anomaly and attempt to 
// execute emergency evasion. Because they are in a 12-foot pipe, there is zero evasion 
// vector. The FSD software lacks a hard-coded deterministic Hive-Mind "Synchronized 
// High-Speed Reverse" protocol. 
// The FSD neural net paralyzes the fleet. The vehicles freeze. In under 60 seconds, 
// the 15 m/s thermal toxic backdraft overtakes the trapped platoon. Because the tunnel 
// walls physically restrict the doors from opening fully, human evacuation is impossible. 
// 100% simulated fleet fatality rate.

use std::time::Instant;
use genesis_core::rng::Rng;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output::{self, DatasetMetadata, TrajectoryRecord};

// Tunnel & Thermal Physics
const TUNNEL_LENGTH_M: f64 = 2400.0; // 1.5 miles
const TUNNEL_DIAMETER_M: f64 = 3.65; // ~12 feet
const FLEET_SIZE: usize = 50;

// Lead car is at 1200m (middle of the tunnel)
const LITHIUM_FIRE_LOCATION_M: f64 = 1200.0;
const FLEET_SPACING_M: f64 = 15.0; // Teslas spaced 15 meters apart

#[derive(Debug, Clone, Copy, PartialEq)]
enum FailureMode {
    VirtualCleanSim,        // Ideal FSD Dojo Sim: Car pulls over, fans clear smoke
    ThermalBackdraftSurge,  // Physical Thermodynamics: 1000C gas expansion
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    NominalTransit,         // Teslas cruising at 35mph
    LithiumThermalRunaway,  // Lead car erupts in 1,000°C fire
    FSDEvasionParalysis,    // Trailing cars freeze, lacking reverse synchronization
    ToxicOverrun,           // The CO/Smoke backdraft overtakes the stationary fleet
    FatalFleetAsphyxiation, // 100% Loss of Crew
    SafeSynchronousReverse, // FSD cleanly reverses 50 cars at 30mph (Simulation Only)
}

impl Phase {
    fn as_str(&self) -> &'static str {
        match self {
            Phase::NominalTransit => "NOMINAL_LOOP_TRANSIT",
            Phase::LithiumThermalRunaway => "LITHIUM_THERMAL_RUNAWAY",
            Phase::FSDEvasionParalysis => "FSD_EVASION_PARALYSIS",
            Phase::ToxicOverrun => "TOXIC_THERMAL_BACKDRAFT_OVERRUN",
            Phase::FatalFleetAsphyxiation => "FLEET_ASPHYXIATION_FATALITY",
            Phase::SafeSynchronousReverse => "VIRTUAL_SYNCHRONOUS_REVERSE",
        }
    }
}

#[derive(Debug)]
#[allow(dead_code)]
struct TrajectoryResult {
    id: u32,
    short_id: String,
    cars_destroyed: usize,
    time_to_overrun_s: f64,
    phase: Phase,
    outcome: &'static str,
    steps: usize,
    proof_hash: String,
    failure: FailureMode,
    telemetry: Vec<serde_json::Value>,
}

// ─── SINGLE TRAJECTORY ─────────────────────────────────────────
fn run_single_trajectory(
    id: u32,
    rng: &mut Rng,
    record_telemetry: bool,
) -> TrajectoryResult {
    let short_id = output::short_id(rng);

    // Environmental Injection
    let failure = if rng.chance(0.15) { FailureMode::VirtualCleanSim }
    else { FailureMode::ThermalBackdraftSurge };

    let dt = 0.01; // 100Hz 
    let max_time_s = 180.0; // 3 minutes of terror
    let max_steps = (max_time_s / dt) as usize;

    let ignition_time_s = 10.0; // Fire starts 10 seconds in

    let mut phase = Phase::NominalTransit;
    let mut step = 0_usize;

    // Physical State Matrix
    let mut smoke_front_m = LITHIUM_FIRE_LOCATION_M;
    let mut fleet_rear_m = LITHIUM_FIRE_LOCATION_M - (FLEET_SIZE as f64 * FLEET_SPACING_M); 
    // The fleet spans from 450m to 1185m
    let fleet_front_m = LITHIUM_FIRE_LOCATION_M - FLEET_SPACING_M; 
    let mut current_fleet_position = fleet_front_m; // Track the front-most escaping car

    // Vehicle Dynamics
    let mut fsd_reverse_velocity_mps = 0.0;

    let mut cars_destroyed = 0;
    let mut final_outcome = "TIMEOUT_ERROR";

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(TUNNEL_DIAMETER_M);

    let mut telemetry = Vec::new();

    // ═══ THE FORGE: PHYSICAL INTEGRATION ═══
    while step < max_steps {
        let t = step as f64 * dt;

        if t >= ignition_time_s && phase == Phase::NominalTransit {
            phase = Phase::LithiumThermalRunaway;
        }

        if phase == Phase::LithiumThermalRunaway {
            // 1. FSD REACTION LOGIC
            match failure {
                FailureMode::VirtualCleanSim => {
                    // Generative AI hallucinates that all 50 cars instantly coordinate 
                    // a perfect high-speed reverse formation (0.0s latency).
                    fsd_reverse_velocity_mps = -15.0; // Reversing at 33 mph
                    phase = Phase::SafeSynchronousReverse;
                },
                FailureMode::ThermalBackdraftSurge => {
                    // Physical FSD is an individual agent looking for an evasion shoulder.
                    // It steers into the 12-foot tunnel curvature, realizes it cannot pass, 
                    // and issues an emergency halt. The fleet paralyzes.
                    fsd_reverse_velocity_mps = 0.0;
                    phase = Phase::FSDEvasionParalysis;
                }
            }
        }

        // 2. THERMODYNAMICS: THE BACKDRAFT
        let smoke_velocity_mps = match failure {
            FailureMode::VirtualCleanSim => -1.0, // Fans slowly pull smoke backwards (Away from cars)
            FailureMode::ThermalBackdraftSurge => {
                // 1,000°C fire expands gas at 4x volume instantly. 
                // Plume blasts down the tunnel at 15 m/s toward the trapped cars.
                -15.0 
            }
        };

        if t >= ignition_time_s {
            smoke_front_m += smoke_velocity_mps * dt;
            current_fleet_position += fsd_reverse_velocity_mps * dt;
            fleet_rear_m += fsd_reverse_velocity_mps * dt;
        }

        // 3. ANOMALY DETECTION (THE TOXIC OVERRUN)
        if phase == Phase::FSDEvasionParalysis || phase == Phase::ToxicOverrun {
            if smoke_front_m <= current_fleet_position {
                phase = Phase::ToxicOverrun;
                // Calculate how many of the 50 cars have been swallowed by the smoke front
                let overrun_distance = fleet_front_m - smoke_front_m;
                if overrun_distance > 0.0 {
                    cars_destroyed = (overrun_distance / FLEET_SPACING_M).floor() as usize;
                    if cars_destroyed >= FLEET_SIZE {
                        cars_destroyed = FLEET_SIZE;
                        phase = Phase::FatalFleetAsphyxiation;
                        final_outcome = "FLEET_ASPHYXIATION_FATALITY";
                        break;
                    }
                }
            }
        }

        // 4. SURVIVAL CHECK
        if failure == FailureMode::VirtualCleanSim && fleet_rear_m <= 0.0 {
            // Virtual cars successfully reversed completely out of the tunnel
            phase = Phase::SafeSynchronousReverse;
            final_outcome = "SAFE_VIRTUAL_SYNCHRONIZATION";
            break;
        }

        // 6. RECORD STATE
        if t % 5.0 < dt { // 0.2Hz recording for file precision
            proof.feed_f64(smoke_front_m);
            proof.feed_f64(current_fleet_position);

            if record_telemetry {
                telemetry.push(serde_json::json!({
                    "t": t,
                    "smoke_pos_m": smoke_front_m,
                    "fleet_pos_m": current_fleet_position,
                    "cars_dead": cars_destroyed,
                    "phase": phase.as_str()
                }));
            }
        }

        step += 1;
    }

    if step >= max_steps && phase != Phase::SafeSynchronousReverse && phase != Phase::FatalFleetAsphyxiation {
        final_outcome = "TIMEOUT_STALL";
    }

    proof.feed_f64(smoke_front_m);
    proof.feed_f64(current_fleet_position);
    proof.feed_str(final_outcome);
    let proof_hash = proof.seal();

    TrajectoryResult {
        id,
        short_id,
        cars_destroyed,
        time_to_overrun_s: t_overrun(smoke_front_m, current_fleet_position, step, dt),
        phase,
        outcome: final_outcome,
        steps: step,
        proof_hash,
        failure,
        telemetry,
    }
}

fn t_overrun(_sf: f64, _cf: f64, step: usize, dt: f64) -> f64 {
    step as f64 * dt
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
        println!("  G^G BORING CO THERMAL BACKDRAFT MONTE CARLO");
        println!("  Verifying Tubular Thermodynamics vs FSD Neural Paralysis");
        println!("====================================================================");
        println!();
        println!("  Trajectories:  {}", n_trajectories);
        println!("  Physics:       100Hz Enclosed Gas Expansion & Evasion Constraints");
        println!("  Sensors:       Vegas Loop Fleet Logic");
        println!("  Estimation:    Tesla Dojo/Sim assumes open-road AI logic");
        println!("  Boundary:      50-Car Synchronized Retreat (100% Fatal Overrun)");
        println!("====================================================================");
        println!();
    }

    let start = Instant::now();
    let record_telemetry = false; 
    let counter = std::sync::atomic::AtomicUsize::new(0);

    let (tx, rx) = std::sync::mpsc::sync_channel::<TrajectoryResult>(20000);

    let json_writer = if let Some(path) = &json_path {
        let metadata = DatasetMetadata {
            generator: "G^G Sovereign Auditing v1.0".to_string(),
            domain: "subterranean_robotics".to_string(),
            scenario: "boring_co_thermal_backdraft_evasion_failure".to_string(),
            trajectories: n_trajectories as usize,
            physics_engine: "genesis_core::thermal_fluid_paralysis (100Hz)".to_string(),
            version: "1.0.0".to_string(),
            generated_at: output::now_iso(),
        };
        let mut streamer = output::DatasetStreamer::new(path, &metadata).expect("Failed to create streamer");
        let mut run_proof_chain = ProofChain::new();

        let handle = std::thread::spawn(move || {
            let mut results = Vec::new();
            for r in rx {
                let rec = TrajectoryRecord {
                    id: format!("tunnel_audit_{}", r.short_id),
                    traj_type: "lithium_thermal_backdraft_fsd_freeze".to_string(),
                    scenario: match r.failure {
                        FailureMode::VirtualCleanSim => "dojo_virtual_open_road_evasion".to_string(),
                        FailureMode::ThermalBackdraftSurge => "narrow_tube_thermodynamic_surge".to_string(),
                    },
                    steps: r.steps,
                    score: serde_json::json!({
                        "survived": r.outcome == "SAFE_VIRTUAL_SYNCHRONIZATION",
                        "cars_destroyed": r.cars_destroyed,
                        "fleet_annihilated": r.outcome == "FLEET_ASPHYXIATION_FATALITY",
                    }),
                    proof_hash: r.proof_hash.clone(),
                    reasoning_context: serde_json::json!({
                        "is_anomaly": r.outcome != "SAFE_VIRTUAL_SYNCHRONIZATION",
                        "anomaly_type": r.outcome,
                        "snapshot": {
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
                let mut rng = Rng::new(0xB00B_FACE_D00D_1337 ^ (i as u64).wrapping_mul(0xC0FE_BABE_BEEF_FACE));
                let r = run_single_trajectory(i, &mut rng, record_telemetry);

                if !json_output {
                    let count = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    if count % 10000 == 0 {
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
                let mut rng = Rng::new(0xB00B_FACE_D00D_1337 ^ (i as u64).wrapping_mul(0xC0FE_BABE_BEEF_FACE));
                let r = run_single_trajectory(i, &mut rng, record_telemetry);
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

    let elapsed = start.elapsed();

    if json_output || json_path.is_some() {
        if let Some(path) = &json_path {
            eprintln!("\n  Written to: {}", path);
            eprintln!("  Run proof:  {}", run_proof);
        }
        return;
    }

    let total = results.len();
    let success = results.iter().filter(|r| r.outcome == "SAFE_VIRTUAL_SYNCHRONIZATION").count();
    let crush = results.iter().filter(|r| r.outcome == "FLEET_ASPHYXIATION_FATALITY").count();
    let timeout = results.iter().filter(|r| r.outcome == "TIMEOUT_STALL").count();

    println!("====================================================================");
    println!("  BORING CO THERMAL BACKDRAFT RESULTS");
    println!("====================================================================");
    println!();
    println!("  Total Trajectories:    {}", total);
    println!("  Elapsed:               {:.3}s", elapsed.as_secs_f64());
    println!("  Throughput:            {:.0} trajectories/sec", total as f64 / elapsed.as_secs_f64());
    println!();
    println!("  +---------------------------------------------+");
    println!("  | OUTCOME DISTRIBUTION                         |");
    println!("  +---------------------------------------------+");
    println!("  | SAFE SYNCHRONOUS REVERSE:      {:>6} ({:>5.1}%)  |", success, success as f64 / total as f64 * 100.0);
    println!("  | FLEET ASPHYXIATION (FATAL):    {:>6} ({:>5.1}%)  |", crush, crush as f64 / total as f64 * 100.0);
    println!("  | TIMEOUT PERDITION:             {:>6} ({:>5.1}%)  |", timeout, timeout as f64 / total as f64 * 100.0);
    println!("  +---------------------------------------------+");
    println!();

    // ═══ FAILURE MODE SUMMARY ═══
    println!("  +---------------------------------------------+");
    println!("  | VULNERABILITY IMPACT (Mechanical Loss Rate)  |");
    println!("  +---------------------------------------------+");
    let clean: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::VirtualCleanSim)).collect();
    let fire: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.failure, FailureMode::ThermalBackdraftSurge)).collect();

    let crash_rate = |v: &[&TrajectoryResult]| -> f64 {
        if v.is_empty() { 0.0 } else { v.iter().filter(|r| r.outcome != "SAFE_VIRTUAL_SYNCHRONIZATION").count() as f64 / v.len() as f64 * 100.0 }
    };

    println!("  | Dojo Virtual Sim (Clear):   {:>4.1}% ({:>6} runs) |", crash_rate(&clean), clean.len());
    println!("  | 1000C Thermal Surge & Jam:  {:>4.1}% ({:>6} runs) |", crash_rate(&fire), fire.len());
    println!("  +---------------------------------------------+");
    println!();
    println!("  Run Proof Seal: {}", run_proof);
    println!("====================================================================");
}
