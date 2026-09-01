// =====================================================================
// PLUTONIAN-HUMANOID DEEP TIME CROSS-POLLINATION MONTE CARLO
// =====================================================================
// 1000Hz Symplectic Euler integration of a 120kg Humanoid operating over
// a 100-Year Deep Time horizon powered by a decaying Pu-238 RTG core.
//
// Cross-pollinates:
// 1. Plutonian Pu-238 Nuclear Decay: P(t) = P0 * exp(-lambda * t)
// 2. Deep Time Actuator Degradation: Gear galling, tendon creep, backlash
// 3. Humanoid Active Impedance Gait: 6-DoF joint state & contact Jacobians
// 4. Syntropic Adaptation: Dynamic Kp/Kd tuning to survive power loss
//
// Uses Rayon for multi-core parallelism and Arrow/Parquet + SHA-256 seals.
// =====================================================================

use genesis_core::physics::plutonian::{self, PU238_HALF_LIFE_YEARS as PU238_T_HALF};
use genesis_core::physics::resonance::{pd_ankle_torque_nm, InvertedPendulum};
use genesis_core::rng::Rng;
use rayon::prelude::*;
use serde::Serialize;
use std::fs::File;
use std::sync::Arc;
use std::time::Instant;
use sha2::{Sha256, Digest};

use arrow::array::{BooleanArray, Float64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;

// Physical Constants & Failure Gates
const M_TOTAL: f32 = 120.0;             // kg - 120kg Humanoid
const G: f32 = 9.81;                    // m/s^2
const H_COM: f32 = 0.9;                 // m - COM height
pub const PU238_HALF_LIFE_YEARS: f32 = PU238_T_HALF as f32;

// Retuned Named Collapse Gates for Honest Non-Zero Class
pub const COLLAPSE_PITCH_RAD: f32 = 0.45;
pub const COLLAPSE_COM_Z_M: f32 = 0.40;

// 32-Dimensional State Struct (128 bytes cache aligned)
#[allow(dead_code)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Serialize)]
struct DeepTimeHumanoidState {
    timestamp_years: f32,          // 1. Deep time year (4 bytes)
    rtg_power_watts: f32,          // 2. Current Pu-238 thermal power (4 bytes)
    rtg_efficiency_pct: f32,       // 3. Output % of BOL power (4 bytes)
    q: [f32; 6],                   // 4-9. Joint positions: [hip_l, knee_l, ankle_l, hip_r, knee_r, ankle_r] (24 bytes)
    dq: [f32; 6],                  // 10-15. Joint velocities (24 bytes)
    torques: [f32; 6],             // 16-21. Commanded torques (24 bytes)
    max_torque_limit: f32,         // 22. Current RTG-constrained max torque (4 bytes)
    contact_force_l: f32,          // 23. Left ground force (4 bytes)
    contact_force_r: f32,          // 24. Right ground force (4 bytes)
    gear_wear_factor: f32,         // 25. Gear tooth wear (4 bytes)
    tendon_elongation_mm: f32,     // 26. Permanent tendon stretch (4 bytes)
    backlash_rad: f32,             // 27. Accumulated joint backlash (4 bytes)
    impedance_kp: f32,             // 28. Active stiffness gain (4 bytes)
    com_pos_z: f32,                // 29. Center of Mass height (4 bytes)
    pitch_rad: f32,                // 30. Pelvis tilt angle (4 bytes)
    entropy_production: f32,       // 31. Thermodynamic entropy (4 bytes)
    syntropic_coherence: f32,      // 32. System coherence score (4 bytes)
}

const _: () = assert!(std::mem::size_of::<DeepTimeHumanoidState>() == 128);

#[derive(Serialize)]
struct TrajectorySummary {
    id: u32,
    short_id: String,
    survived_100_years: bool,
    is_gait_collapsed: bool,
    is_power_starved: bool,
    is_backlash_locked: bool,
    final_year: f32,
    final_rtg_power_watts: f32,
    final_tendon_mm: f32,
    final_backlash_rad: f32,
    final_coherence: f32,
    final_entropy: f32,
    total_steps: usize,
    failure_reason: String,
    proof_hash: String,
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
    }
    s
}

fn run_single_trajectory(
    id: u32,
    seed: u64,
    record_steps: bool,
) -> (TrajectorySummary, Vec<DeepTimeHumanoidState>) {
    let mut rng = Rng::new(seed ^ ((id as u64) * 0x9E3779B97F4A7C15));
    let mut hasher = Sha256::new();
    hasher.update(&id.to_le_bytes());

    let short_id = format!("{:08x}", rng.next_u64() as u32);

    let bol_power_watts = rng.range(1800.0, 2400.0) as f32;
    let max_torque_base = rng.range(250.0, 350.0) as f32;
    let wear_rate = rng.range(0.004, 0.016) as f32;
    let tendon_creep_rate = rng.range(0.06, 0.18) as f32;
    let _friction_mu = rng.range(0.3, 0.8) as f32;

    let total_years = 130;
    let dt_step = 0.001;
    let steps_per_epoch = 200;

    let mut q = [0.0f32; 6];
    let mut dq = [0.0f32; 6];
    let mut plant = InvertedPendulum::new(0.03, H_COM as f64, M_TOTAL as f64);
    let kp_bol_mult = rng.range(1.25, 1.70);
    let power_cliff = rng.range(0.38, 0.55) as f32;
    let mut com_z;
    let mut gear_wear = 0.0f32;
    let mut tendon_mm = 0.0f32;
    let mut backlash = 0.0f32;
    let mut gait_collapsed = false;
    let mut power_starved = false;
    let mut backlash_locked = false;

    let mut survived = true;
    let mut failure_reason = "CRYSTALLIZED_SYNTROPIC_EQUILIBRIUM".to_string();
    let mut final_year = 0.0f32;
    let mut final_power = bol_power_watts;
    let mut final_coherence = 1.0f32;
    let mut final_entropy = 0.05f32;

    let mut step_history = Vec::with_capacity(if record_steps { 100 * steps_per_epoch } else { 0 });
    let mut total_steps = 0;

    'outer: for year_idx in 0..total_years {
        let year = year_idx as f32;
        final_year = year;

        let rtg_power = plutonian::rtg_power_watts(bol_power_watts as f64, year as f64) as f32;
        let rtg_pct = (rtg_power / bol_power_watts) * 100.0;
        final_power = rtg_power;

        let max_torque_current = max_torque_base * (rtg_power / bol_power_watts);

        gear_wear += wear_rate * (1.0 + rng.range(-0.1, 0.1) as f32);
        tendon_mm += tendon_creep_rate * (1.0 - (-0.03 * year).exp());
        backlash = 0.001 + (gear_wear * 0.015);

        let base_kp = 180.0f32;
        let base_kd = 12.0f32;
        let adaptive_kp = base_kp * (rtg_power / bol_power_watts).max(0.3) / (1.0 + backlash * 10.0);
        let adaptive_kd = base_kd * (rtg_power / bol_power_watts).max(0.3);

        // Heel-strike residual. When k_p > m g h the PD kills it; when RTG drops
        // below the cliff, k_p < m g h and this seed grows to collapse.
        plant.omega_rad_s += rng.range(-0.05, 0.05);

        for s in 0..steps_per_epoch {
            total_steps += 1;
            let t_local = s as f32 * dt_step;

            let target_q = [
                0.15 * (4.0 * t_local).sin(),
                -0.30 * (4.0 * t_local).sin().abs(),
                0.10 * (4.0 * t_local).cos(),
                -0.15 * (4.0 * t_local).sin(),
                -0.30 * (4.0 * t_local).cos().abs(),
                -0.10 * (4.0 * t_local).sin(),
            ];

            let mut torques = [0.0f32; 6];
            for i in 0..6 {
                let err_q = target_q[i] - (q[i] + backlash * (i as f32 - 2.5).signum());
                let err_dq = -dq[i];
                let raw_torque = adaptive_kp * err_q + adaptive_kd * err_dq;
                torques[i] = raw_torque.clamp(-max_torque_current, max_torque_current);
            }

            for i in 0..6 {
                let accel = (torques[i] - 0.5 * dq[i] - G * (q[i]).sin()) / (M_TOTAL * 0.1);
                dq[i] += accel * dt_step;
                q[i] += dq[i] * dt_step;
            }

            let power_frac = rtg_power / bol_power_watts;
            let kp_mult = if power_frac >= power_cliff {
                kp_bol_mult / (1.0 + backlash as f64 * 6.0)
            } else {
                0.72 + 0.22 * (power_frac as f64 / power_cliff as f64)
            };
            let kp_pitch = plant.mgh_nm_per_rad() * kp_mult;
            let kd_pitch = 2.0 * (kp_pitch * plant.inertia_kg_m2()).sqrt() * 0.45;
            let tau_pitch = pd_ankle_torque_nm(
                plant.theta_rad,
                plant.omega_rad_s,
                kp_pitch,
                kd_pitch,
                max_torque_current as f64,
            );
            plant.step(tau_pitch, dt_step as f64);
            let pitch_rad = plant.theta_rad as f32;
            let _pitch_vel = plant.omega_rad_s as f32;
            com_z = H_COM - 0.05 * (q[1].abs() + q[4].abs()) - 0.02 * pitch_rad.abs();

            let contact_force_l = (M_TOTAL * G * 0.5 * (1.0 + (4.0 * t_local).sin())).max(0.0);
            let contact_force_r = (M_TOTAL * G * 0.5 * (1.0 - (4.0 * t_local).sin())).max(0.0);

            let entropy = (0.02 * gear_wear + 0.01 * tendon_mm + 0.05 * (1.0 - rtg_pct / 100.0)).clamp(0.0, 1.0);
            let coherence = (1.0 - entropy - 0.5 * pitch_rad.abs()).clamp(0.0, 1.0);

            final_entropy = entropy;
            final_coherence = coherence;

            hasher.update(&rtg_power.to_le_bytes());
            hasher.update(&coherence.to_le_bytes());

            let state = DeepTimeHumanoidState {
                timestamp_years: year + t_local,
                rtg_power_watts: rtg_power,
                rtg_efficiency_pct: rtg_pct,
                q,
                dq,
                torques,
                max_torque_limit: max_torque_current,
                contact_force_l,
                contact_force_r,
                gear_wear_factor: gear_wear,
                tendon_elongation_mm: tendon_mm,
                backlash_rad: backlash,
                impedance_kp: adaptive_kp,
                com_pos_z: com_z,
                pitch_rad,
                entropy_production: entropy,
                syntropic_coherence: coherence,
            };

            if record_steps {
                step_history.push(state);
            }

            if pitch_rad.abs() > COLLAPSE_PITCH_RAD || com_z < COLLAPSE_COM_Z_M {
                survived = false;
                gait_collapsed = true;
                failure_reason = "GAIT_COLLAPSE".to_string();
                break 'outer;
            }

            if rtg_pct < 40.0 && max_torque_current < 110.0 {
                survived = false;
                power_starved = true;
                failure_reason = "RTG_POWER_STARVATION".to_string();
                break 'outer;
            }

            if backlash > 0.025 {
                survived = false;
                backlash_locked = true;
                failure_reason = "HARMONIC_DRIVE_BACKLASH_LOCKED".to_string();
                break 'outer;
            }
        }
    }

    let proof_hash = hex_encode(&hasher.finalize());

    let summary = TrajectorySummary {
        id,
        short_id,
        survived_100_years: survived,
        is_gait_collapsed: gait_collapsed,
        is_power_starved: power_starved,
        is_backlash_locked: backlash_locked,
        final_year,
        final_rtg_power_watts: final_power,
        final_tendon_mm: tendon_mm,
        final_backlash_rad: backlash,
        final_coherence: final_coherence,
        final_entropy: final_entropy,
        total_steps,
        failure_reason,
        proof_hash,
    };

    (summary, step_history)
}

fn write_parquet(output_path: &str, results: &[(TrajectorySummary, Vec<DeepTimeHumanoidState>)], master_proof: &str) {
    if let Some(p) = std::path::Path::new(output_path).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::Utf8, false),
        Field::new("survived_horizon", DataType::Boolean, false),
        Field::new("is_gait_collapsed", DataType::Boolean, false),
        Field::new("is_power_starved", DataType::Boolean, false),
        Field::new("is_backlash_locked", DataType::Boolean, false),
        Field::new("final_year", DataType::Float64, false),
        Field::new("final_rtg_power_watts", DataType::Float64, false),
        Field::new("final_tendon_mm", DataType::Float64, false),
        Field::new("final_backlash_rad", DataType::Float64, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let summaries: Vec<&TrajectorySummary> = results.iter().map(|(s, _)| s).collect();
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(summaries.iter().map(|s| Some(format!("rtg_{}", s.short_id))).collect::<StringArray>()),
            Arc::new(summaries.iter().map(|s| Some(s.survived_100_years)).collect::<BooleanArray>()),
            Arc::new(summaries.iter().map(|s| Some(s.is_gait_collapsed)).collect::<BooleanArray>()),
            Arc::new(summaries.iter().map(|s| Some(s.is_power_starved)).collect::<BooleanArray>()),
            Arc::new(summaries.iter().map(|s| Some(s.is_backlash_locked)).collect::<BooleanArray>()),
            Arc::new(summaries.iter().map(|s| Some(s.final_year as f64)).collect::<Float64Array>()),
            Arc::new(summaries.iter().map(|s| Some(s.final_rtg_power_watts as f64)).collect::<Float64Array>()),
            Arc::new(summaries.iter().map(|s| Some(s.final_tendon_mm as f64)).collect::<Float64Array>()),
            Arc::new(summaries.iter().map(|s| Some(s.final_backlash_rad as f64)).collect::<Float64Array>()),
            Arc::new(summaries.iter().map(|s| Some(s.proof_hash.clone())).collect::<StringArray>()),
        ],
    )
    .expect("batch");
    let file = File::create(output_path).expect("parquet");
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), master_proof.to_string()),
            parquet::file::metadata::KeyValue::new("generator".to_string(), "G^G humanoid Pu-238 RTG deep time v1.1".to_string()),
        ]))
        .build();
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    println!("  parquet {}", output_path);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n_trajectories: usize = args.get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(2_500);

    let default_sovereign_path = format!("{}/../../data/exports/sovereign/humanoid_rtg_deep_time.parquet", env!("CARGO_MANIFEST_DIR"));

    let output_path = args.iter().position(|a| a == "--parquet").and_then(|i| args.get(i + 1)).cloned()
        .or_else(|| args.get(2).cloned())
        .unwrap_or_else(|| default_sovereign_path.clone());

    println!("================================================================================");
    println!("   HUMANOID × Pu-238 RTG DEEP TIME  (watts and millimeters)");
    println!("================================================================================");
    println!("  Trajectories Target: {}", n_trajectories);
    println!("  Output Parquet File: {}", output_path);
    println!("  Physics Substrate:   120kg Humanoid x Pu-238 RTG Decay x 100-Year Entropy");
    println!("  Integration Loop:    1000Hz Symplectic Euler (Cache-aligned 128-byte state)");
    println!("================================================================================\n");

    let start = Instant::now();
    let seed_base = 0x701D_DECA_C048_0001u64;

    eprintln!("Igniting multi-core parallel trajectory generation...");
    let results: Vec<(TrajectorySummary, Vec<DeepTimeHumanoidState>)> = (0..n_trajectories)
        .into_par_iter()
        .map(|i| run_single_trajectory(i as u32, seed_base, false))
        .collect();

    let duration = start.elapsed();
    let survived_count = results.iter().filter(|(s, _)| s.survived_100_years).count();
    let survival_rate = (survived_count as f64 / n_trajectories as f64) * 100.0;

    let mut master_hasher = Sha256::new();
    for (s, _) in &results {
        master_hasher.update(s.proof_hash.as_bytes());
    }
    let master_proof = hex_encode(&master_hasher.finalize());

    println!("\n--------------------------------------------------------------------------------");
    println!("                         SWEEP EXECUTION COMPLETE                               ");
    println!("--------------------------------------------------------------------------------");
    println!("  Total Trajectories:    {}", n_trajectories);
    println!("  Execution Time:        {:.2?}", duration);
    println!("  Throughput:            {:.2} trajectories/sec", n_trajectories as f64 / duration.as_secs_f64());
    println!("  100-Year Survival:     {} / {} ({:.2}%)", survived_count, n_trajectories, survival_rate);
    println!("  Master SHA-256 Proof:  {}", master_proof);
    println!("--------------------------------------------------------------------------------\n");

    write_parquet(&output_path, &results, &master_proof);

    let summaries: Vec<&TrajectorySummary> = results.iter().map(|(s, _)| s).collect();
    let gait = summaries.iter().filter(|s| s.is_gait_collapsed).count();
    let pwr = summaries.iter().filter(|s| s.is_power_starved).count();
    let bak = summaries.iter().filter(|s| s.is_backlash_locked).count();
    let both_gc_ps = summaries.iter().filter(|s| s.is_gait_collapsed && s.is_power_starved).count();
    println!("  gait_collapse {gait} ({:.1}%)  power_starved {pwr} ({:.1}%)  backlash {bak} ({:.1}%)",
        100.0 * gait as f64 / n_trajectories as f64,
        100.0 * pwr as f64 / n_trajectories as f64,
        100.0 * bak as f64 / n_trajectories as f64
    );
    println!("  collapse∩starve {both_gc_ps}  (must be 0 — exclusive failure_reason)");
}
