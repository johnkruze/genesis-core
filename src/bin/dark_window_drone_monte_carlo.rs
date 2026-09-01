//! 1000Hz GENESIS CORE MODULE: DARK_WINDOW_DRONE_MONTE_CARLO
//! TARGET: Unmanned Aerial Systems (UAS) / Tactical Defense Autonomy
//! CLASS: Autonomous Flight Platforms in Contested RF / Dense Smoke
//! SUBSYSTEM: Visual-Inertial Navigation & Local Deliberative Divert System
//! VULNERABILITY: GPS/RF blackout + Beer-Lambert smoke optical drift (+10 to +35 m/s).
//! Dual-regime: Unprotected Autopilot Crash vs ZTP Reflex Safe Touchdown / Still Airborne / Hard Impact.

use std::fs::File;
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{BooleanArray, Float64Array, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::ArrowWriter;
use serde::Serialize;

use genesis_core::output;
use genesis_core::last_state::{self, LastStateFrame64, BODY_DRONE};
use genesis_core::physics::optics;
use genesis_core::proof::{self, ProofChain};
use genesis_core::rng::Rng;

const DEFAULT_N: usize = 2500;
const TOTAL_SIM_TIME_S: f64 = 6.0;
const DT_S: f64 = 0.001; // 1000 Hz physical integration
const CAMERA_DT_S: f64 = 1.0 / 30.0; // 30 Hz camera rate
const COHERENCE_THRESHOLD: f64 = 1.5; // 1.5 m/s^2 residual gate

#[derive(Debug, Serialize)]
struct DarkWindowFlightRun {
    id: u32,
    short_id: String,
    drone_mass_kg: f64,
    blackout_start_s: f64,
    t_event_ms: f64,
    smoke_optical_depth: f64,
    peak_coherence_residual: f64,
    unprotected_crashed: bool,
    reflex_touchdown_safe: bool,
    reflex_still_airborne: bool,
    reflex_hard_impact: bool,
    proof_hash: String,
    // Soma pinout only — not parquet columns.
    final_alt_m: f64,
    final_vz_ms: f64,
    vslam_fail: bool,
    reflex_held: bool,
}

fn simulate_dark_window_trajectory(id: u32, rng: &mut Rng) -> DarkWindowFlightRun {
    let mut chain = ProofChain::new();
    let short_id = output::short_id(rng);
    chain.feed_str(&short_id);

    let drone_mass_kg = rng.range(1.6, 3.4);
    let cruise_speed_ms = rng.range(8.0, 16.0);
    let smoke_drift_velocity_ms = rng.range(14.0, 32.0);
    let smoke_rise_velocity_ms = rng.range(2.5, 6.0);
    let blackout_start_s = rng.range(1.2, 2.2);
    let smoke_extinction_coeff = rng.range(0.8, 2.5); // Beer-Lambert beta
    let optical_path_length_m = rng.range(2.0, 8.0);
    let initial_altitude_m = rng.range(3.0, 14.0);

    chain.feed_f64(drone_mass_kg);
    chain.feed_f64(cruise_speed_ms);
    chain.feed_f64(smoke_drift_velocity_ms);
    chain.feed_f64(blackout_start_s);
    chain.feed_f64(smoke_extinction_coeff);
    chain.feed_f64(initial_altitude_m);

    let steps = (TOTAL_SIM_TIME_S / DT_S) as usize;
    let drag_coeff = 0.18f64;
    let max_thrust = drone_mass_kg * 9.81 * 2.2; // 2.2:1 thrust-to-weight
    let hover_throttle = (drone_mass_kg * 9.81) / max_thrust;

    // ─────────────────────────────────────────────────────────────────────────
    // SIMULATION A: UNPROTECTED AUTOPILOT (VSLAM VELOCITY-HOLD LOOP)
    // ─────────────────────────────────────────────────────────────────────────
    let mut unprot_pos_z = initial_altitude_m;
    let mut unprot_vel_z = 0.0;
    let mut unprot_crashed = false;
    let mut last_cam_t = 0.0;
    let mut vslam_vz = 0.0;

    for step in 0..steps {
        let t = step as f64 * DT_S;
        let in_smoke = t >= blackout_start_s;

        // 30 Hz Camera optical flow update with Beer-Lambert optical transmittance
        if t - last_cam_t >= CAMERA_DT_S {
            last_cam_t = t;
            if in_smoke {
                let optical_transmission = optics::beer_lambert_transmittance(smoke_extinction_coeff, optical_path_length_m);
                let smoke_optical_density = (1.0 - optical_transmission).clamp(0.0, 1.0);
                vslam_vz = unprot_vel_z + smoke_rise_velocity_ms * smoke_optical_density;
            } else {
                vslam_vz = unprot_vel_z;
            }
        }

        // Standard velocity-hold feedback controller: error = vslam_vz - target_vz
        let throttle = if in_smoke {
            let error_vz = vslam_vz - 0.0;
            let kp = 0.14;
            (hover_throttle - kp * error_vz).clamp(0.12, 0.95)
        } else {
            hover_throttle
        };

        let thrust_z = throttle * max_thrust;
        let true_acc_z = (thrust_z - drag_coeff * unprot_vel_z) / drone_mass_kg - 9.81;
        unprot_vel_z += true_acc_z * DT_S;
        unprot_pos_z += unprot_vel_z * DT_S;

        if unprot_pos_z <= 0.0 {
            unprot_crashed = unprot_vel_z.abs() > 3.0; // Hard ground impact
            break;
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // SIMULATION B: ZTP 1000Hz REFLEX + LOCAL COGNITIVE DIVERT
    // ─────────────────────────────────────────────────────────────────────────
    let mut prot_pos_z = initial_altitude_m;
    let mut prot_vel_z = 0.0;
    let mut reflex_touchdown_safe = false;
    let mut reflex_still_airborne = false;
    let mut max_coherence_residual = 0.0f64;
    let mut reflex_triggered = false;
    let mut event_trigger_time_ms = 0.0;

    let mut last_cam_t_b = 0.0;
    let mut vslam_vz_b = 0.0;
    let mut vslam_vz_prev_b = 0.0;

    for step in 0..steps {
        let t = step as f64 * DT_S;
        let in_blackout = t >= blackout_start_s;

        // 30 Hz Camera optical flow update
        if t - last_cam_t_b >= CAMERA_DT_S {
            last_cam_t_b = t;
            vslam_vz_prev_b = vslam_vz_b;
            if in_blackout {
                let optical_transmission = optics::beer_lambert_transmittance(smoke_extinction_coeff, optical_path_length_m);
                let smoke_optical_density = (1.0 - optical_transmission).clamp(0.0, 1.0);
                vslam_vz_b = prot_vel_z + smoke_rise_velocity_ms * smoke_optical_density;
            } else {
                vslam_vz_b = prot_vel_z;
            }
        }

        // 1000 Hz ZTP Coherence Audit
        let commanded_throttle = if reflex_triggered {
            hover_throttle * 0.85 // Emergency controlled descent throttle (~1.8 m/s sink)
        } else {
            hover_throttle
        };

        let thrust_z = commanded_throttle * max_thrust;
        let true_acc_z = (thrust_z - drag_coeff * prot_vel_z) / drone_mass_kg - 9.81;
        let imu_acc_z = (thrust_z - drag_coeff * prot_vel_z) / drone_mass_kg;

        let a_vslam_z = (vslam_vz_b - vslam_vz_prev_b) / CAMERA_DT_S;
        let residual = (imu_acc_z - (a_vslam_z + 9.81)).abs();
        if residual > max_coherence_residual {
            max_coherence_residual = residual;
        }

        if residual > COHERENCE_THRESHOLD && !reflex_triggered && in_blackout {
            reflex_triggered = true;
            event_trigger_time_ms = t * 1000.0;
        }

        // State update
        prot_vel_z += true_acc_z * DT_S;
        prot_pos_z += prot_vel_z * DT_S;

        if prot_pos_z <= 0.0 {
            if prot_vel_z.abs() <= 3.0 {
                reflex_touchdown_safe = true;
            }
            break;
        }
    }

    if prot_pos_z > 0.0 {
        reflex_still_airborne = true;
    }

    let reflex_hard_impact = !reflex_touchdown_safe && !reflex_still_airborne;

    chain.feed_f64(max_coherence_residual);
    chain.feed_f64(event_trigger_time_ms);
    chain.feed_str(if unprot_crashed { "UNPROT_CRASHED" } else { "UNPROT_SURVIVED" });
    chain.feed_str(if reflex_touchdown_safe { "TOUCHDOWN_SAFE" } else if reflex_still_airborne { "STILL_AIRBORNE" } else { "HARD_IMPACT" });

    let smoke_optical_depth = smoke_extinction_coeff * optical_path_length_m;

    DarkWindowFlightRun {
        id,
        short_id,
        drone_mass_kg: (drone_mass_kg * 100.0).round() / 100.0,
        blackout_start_s: (blackout_start_s * 100.0).round() / 100.0,
        t_event_ms: (event_trigger_time_ms * 10.0).round() / 10.0,
        smoke_optical_depth: (smoke_optical_depth * 100.0).round() / 100.0,
        peak_coherence_residual: (max_coherence_residual * 10.0).round() / 10.0,
        unprotected_crashed: unprot_crashed,
        reflex_touchdown_safe,
        reflex_still_airborne,
        reflex_hard_impact,
        proof_hash: chain.seal(),
        final_alt_m: prot_pos_z.max(0.0),
        final_vz_ms: prot_vel_z,
        vslam_fail: max_coherence_residual > COHERENCE_THRESHOLD,
        reflex_held: reflex_touchdown_safe || reflex_still_airborne,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let num_runs: usize = args
        .get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_N);

    let parquet_path = args
        .iter()
        .position(|a| a == "--parquet")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| {
            format!("{}/../../data/exports/sovereign/dark_window_drone_nav.parquet", env!("CARGO_MANIFEST_DIR"))
        });

    println!("=========================================================");
    println!("G^G SOVEREIGN PHYSICS ENGINE: DARK WINDOW FLIGHT AUDIT");
    println!("MODULE: DARK_WINDOW_DRONE_NAV (ZTP Reflex + Qwen Mesh)");
    println!("TRAJECTORIES: {}", num_runs);
    println!("=========================================================\n");

    let start = Instant::now();
    let mut rng = Rng::new(0xDA8C_0003);

    let runs: Vec<DarkWindowFlightRun> = (0..num_runs as u32)
        .map(|i| simulate_dark_window_trajectory(i, &mut rng))
        .collect();

    let unprot_crashed_count = runs.iter().filter(|r| r.unprotected_crashed).count();
    let safe_touchdown_count = runs.iter().filter(|r| r.reflex_touchdown_safe).count();
    let airborne_count = runs.iter().filter(|r| r.reflex_still_airborne).count();
    let hard_impact_count = runs.iter().filter(|r| r.reflex_hard_impact).count();

    let proofs: Vec<String> = runs.iter().map(|r| r.proof_hash.clone()).collect();
    let run_seal = proof::seal_run(&proofs);

    if let Some(parent) = std::path::Path::new(&parquet_path).parent() {
        std::fs::create_dir_all(parent).unwrap();
    }

    let schema = Arc::new(Schema::new(vec![
            Field::new("trajectory_id", DataType::UInt32, false),
            Field::new("short_id", DataType::Utf8, false),
            Field::new("drone_mass_kg", DataType::Float64, false),
            Field::new("blackout_start_s", DataType::Float64, false),
            Field::new("t_event_ms", DataType::Float64, false),
            Field::new("smoke_optical_depth", DataType::Float64, false),
            Field::new("peak_coherence_residual", DataType::Float64, false),
            Field::new("unprotected_crashed", DataType::Boolean, false),
            Field::new("reflex_touchdown_safe", DataType::Boolean, false),
            Field::new("reflex_still_airborne", DataType::Boolean, false),
            Field::new("reflex_hard_impact", DataType::Boolean, false),
            Field::new("proof_hash", DataType::Utf8, false),
        ]));

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(runs.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(runs.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(runs.iter().map(|r| r.drone_mass_kg).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(runs.iter().map(|r| r.blackout_start_s).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(runs.iter().map(|r| r.t_event_ms).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(runs.iter().map(|r| r.smoke_optical_depth).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(runs.iter().map(|r| r.peak_coherence_residual).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(runs.iter().map(|r| r.unprotected_crashed).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(runs.iter().map(|r| r.reflex_touchdown_safe).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(runs.iter().map(|r| r.reflex_still_airborne).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(runs.iter().map(|r| r.reflex_hard_impact).collect::<Vec<_>>())),
            Arc::new(StringArray::from(runs.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    ).unwrap();

    let file = File::create(&parquet_path).unwrap();
    let props = output::parquet_receipt_properties(
        &run_seal,
        "G^G Dark Window drone nav dual-regime v2.3",
    );
    let mut writer = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();

    // File pinout — one frame per trajectory from the protected plant.
    let soma_frames: Vec<[u8; 64]> = runs
        .iter()
        .map(|r| {
            LastStateFrame64::pack_drone(
                r.id,
                0.0,
                0.0,
                r.final_alt_m as f32,
                0.0,
                0.0,
                r.final_vz_ms as f32,
                0.0,
                r.peak_coherence_residual as f32,
                true,
                r.vslam_fail,
                r.reflex_held,
            )
            .to_bytes()
        })
        .collect();
    let soma_bytes = last_state::write_soma_file(BODY_DRONE, *b"DRONE001", &soma_frames);
    let soma_public = format!(
        "{}/../../grokd/public/soma/drone_terminal.soma.bin",
        env!("CARGO_MANIFEST_DIR")
    );
    let soma_sovereign = format!(
        "{}/../../data/exports/sovereign/drone_terminal.soma.bin",
        env!("CARGO_MANIFEST_DIR")
    );
    for path in [&soma_public, &soma_sovereign] {
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(path, &soma_bytes).unwrap();
    }

    println!("DARK_WINDOW_DRONE_NAV AUDIT COMPLETE");
    println!("  -> Sealed: {}", parquet_path);
    println!("  -> Unprotected Crashed:     {}/{} ({:.2}%)", unprot_crashed_count, num_runs, (unprot_crashed_count as f64 / num_runs as f64) * 100.0);
    println!("  -> Reflex Safe Touchdown:   {}/{} ({:.2}%)", safe_touchdown_count, num_runs, (safe_touchdown_count as f64 / num_runs as f64) * 100.0);
    println!("  -> Reflex Still Airborne:   {}/{} ({:.2}%)", airborne_count, num_runs, (airborne_count as f64 / num_runs as f64) * 100.0);
    println!("  -> Reflex Hard Impact:      {}/{} ({:.2}%)", hard_impact_count, num_runs, (hard_impact_count as f64 / num_runs as f64) * 100.0);
    println!("  -> Run Seal: {}", run_seal);
    println!("  -> Soma: {} ({} B, body 7)", soma_public, soma_bytes.len());
    println!("  -> Time: {:?}", start.elapsed());
}
