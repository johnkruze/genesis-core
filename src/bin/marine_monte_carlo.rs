//! GPS-denied dead reckoning. IMU random-walk vs unmodeled current.
//! Dual-regime: 500 m lock-loss is not the same column as current-dominated drift.
//! Uses MarinePhysics + CurrentField + DeadReckoning — the ocean organ, not a JSON farm.

use genesis_core::output;
use genesis_core::physics::marine::{CurrentField, DeadReckoning, MarinePhysics};
use genesis_core::proof::{self, ProofChain};
use genesis_core::rng::Rng;
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{BooleanArray, Float64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;

const DT: f64 = 5.0;
const LOCK_LOSS_M: f64 = 500.0;

#[derive(Debug, Serialize)]
struct MarineRun {
    id: u32,
    short_id: String,
    drift_rate_ms: f64,
    mission_duration_hr: f64,
    current_speed_ms: f64,
    turbulence_std: f64,
    pressure_noise_std: f64,
    final_error_m: f64,
    current_disp_m: f64,
    imu_walk_m: f64,
    depth_error_m: f64,
    lost_nav_lock: bool,
    current_dominated: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> MarineRun {
    let short_id = output::short_id(rng);
    let drift = rng.range(0.002, 0.06);
    let hours = rng.range(0.5, 4.0);
    let current_speed = rng.range(0.0, 0.12);
    let turb = rng.range(0.005, 0.08);
    let p_noise = rng.range(50.0, 2000.0);

    let physics = MarinePhysics::default();
    let field = CurrentField {
        base_speed: current_speed,
        base_heading: rng.range(0.0, 2.0 * std::f64::consts::PI),
        shear_rate: rng.range(0.002, 0.02),
        tidal_amplitude: rng.range(0.05, 0.40),
        tidal_period: 12.4 * 3600.0,
        turbulence_std: turb,
    };
    let mut nav = DeadReckoning {
        believed_pos: [0.0, 0.0, -50.0],
        drift_rate: drift,
        total_drift: 0.0,
        pressure_noise_std: p_noise,
    };

    let true_velocity = [1.5, 0.0, 0.0];
    let mut true_pos = [0.0, 0.0, -50.0];
    let mut current_disp = [0.0, 0.0];
    // Constant IMU bias (bias instability), not a zero-mean random walk.
    // Random walk never reaches 500 m; a 0.02 m/s bias does in ~7 h.
    let bias_x = rng.gaussian(0.0, drift);
    let bias_y = rng.gaussian(0.0, drift);
    let steps = ((hours * 3600.0) / DT).round() as u64;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(drift);
    proof.feed_f64(hours);
    proof.feed_f64(current_speed);
    proof.feed_f64(bias_x);
    proof.feed_f64(bias_y);
    proof.feed_f64(turb);
    proof.feed_f64(p_noise);

    for step in 0..steps {
        let t = step as f64 * DT;
        let cur = field.sample(50.0, t, rng);
        true_pos[0] += (true_velocity[0] + cur[0]) * DT;
        true_pos[1] += (true_velocity[1] + cur[1]) * DT;
        true_pos[2] += (true_velocity[2] + cur[2]) * DT;
        current_disp[0] += cur[0] * DT;
        current_disp[1] += cur[1] * DT;
        nav.believed_pos[0] += (true_velocity[0] + bias_x) * DT;
        nav.believed_pos[1] += (true_velocity[1] + bias_y) * DT;
        nav.correct_from_pressure(-true_pos[2], &physics, rng);
        if step % 72 == 0 {
            proof.feed_f64(nav.horizontal_error(true_pos));
        }
    }

    let err = nav.horizontal_error(true_pos);
    let cur_m = (current_disp[0] * current_disp[0] + current_disp[1] * current_disp[1]).sqrt();
    let duration_s = hours * 3600.0;
    let imu_m = (bias_x * duration_s).hypot(bias_y * duration_s);
    let depth_err = (nav.believed_pos[2] - true_pos[2]).abs();
    let lost = err > LOCK_LOSS_M;
    let current_dom = cur_m > imu_m;
    proof.feed_f64(err);
    proof.feed_str(if lost && current_dom {
        "CURRENT_LOCK_LOSS"
    } else if lost {
        "IMU_LOCK_LOSS"
    } else if current_dom {
        "CURRENT_IN_BUDGET"
    } else {
        "IMU_IN_BUDGET"
    });

    MarineRun {
        id,
        short_id,
        drift_rate_ms: drift,
        mission_duration_hr: hours,
        current_speed_ms: current_speed,
        turbulence_std: turb,
        pressure_noise_std: p_noise,
        final_error_m: err,
        current_disp_m: cur_m,
        imu_walk_m: imu_m,
        depth_error_m: depth_err,
        lost_nav_lock: lost,
        current_dominated: current_dom,
        proof_hash: proof.seal(),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(2500);
    let out = args
        .iter()
        .position(|a| a == "--parquet")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| {
            format!(
                "{}/../../grokd/data/marine_dead_reckoning.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: MARINE DEAD RECKONING  (IMU walk vs unmodeled current)");
    println!("  n={n}  dt={DT}s  lock-loss {LOCK_LOSS_M} m");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x4d41_5249_4e45_4452);
    let t0 = Instant::now();
    let mut rows = Vec::with_capacity(n as usize);
    for i in 0..n {
        rows.push(run_one(i, &mut rng));
    }
    let proofs: Vec<_> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let run_proof = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::Utf8, false),
        Field::new("drift_rate_ms", DataType::Float64, false),
        Field::new("mission_duration_hr", DataType::Float64, false),
        Field::new("current_speed_ms", DataType::Float64, false),
        Field::new("turbulence_std", DataType::Float64, false),
        Field::new("pressure_noise_std", DataType::Float64, false),
        Field::new("final_error_m", DataType::Float64, false),
        Field::new("current_disp_m", DataType::Float64, false),
        Field::new("imu_walk_m", DataType::Float64, false),
        Field::new("depth_error_m", DataType::Float64, false),
        Field::new("lost_nav_lock", DataType::Boolean, false),
        Field::new("current_dominated", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(rows.iter().map(|r| Some(format!("nav_{}", r.short_id))).collect::<StringArray>()),
            Arc::new(rows.iter().map(|r| Some(r.drift_rate_ms)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.mission_duration_hr)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.current_speed_ms)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.turbulence_std)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.pressure_noise_std)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.final_error_m)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.current_disp_m)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.imu_walk_m)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.depth_error_m)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.lost_nav_lock)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.current_dominated)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.proof_hash.clone())).collect::<StringArray>()),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.clone()),
            parquet::file::metadata::KeyValue::new(
                "generator".to_string(),
                "G^G marine dead reckoning dual-regime v1.0".to_string(),
            ),
        ]))
        .build();
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let lost = rows.iter().filter(|r| r.lost_nav_lock).count();
    let cur = rows.iter().filter(|r| r.current_dominated).count();
    let both = rows
        .iter()
        .filter(|r| r.lost_nav_lock && r.current_dominated)
        .count();
    println!(
        "  lock-loss {lost} ({:.1}%)  current_dom {cur} ({:.1}%)  both {both} ({:.1}%)",
        100.0 * lost as f64 / n_f,
        100.0 * cur as f64 / n_f,
        100.0 * both as f64 / n_f
    );
    println!("  seal {run_proof}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
