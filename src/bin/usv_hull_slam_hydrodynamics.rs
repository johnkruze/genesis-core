//! USV hull slam vs hydrofoil phase lag. Sea State 5 reduced-order pitch.
//! Gates: pitch < −0.4 rad at speed (submarine into trough); slew-limited foil still
//! bow-down as the crest passes. 1000 Hz clock. Not an AI-blame envelope.

use genesis_core::output;
use genesis_core::physics::marine;
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

const HZ: f64 = 1000.0;
const DT: f64 = 1.0 / HZ;
const T_SIM: f64 = 10.0;
const CRITICAL_PITCH_RAD: f64 = -0.4;
const DAMPING_C: f64 = 2.0;
const RESTORING_K: f64 = 10.0;

#[derive(Debug, Serialize)]
struct SlamRun {
    id: u32,
    short_id: String,
    wave_height_m: f64,
    wave_period_s: f64,
    usv_speed_ms: f64,
    encounter_hz: f64,
    foil_slew_rate: f64,
    kp: f64,
    ki: f64,
    min_pitch_rad: f64,
    foil_moment_at_crest: f64,
    is_submarined: bool,
    is_foil_phase_lagged: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> SlamRun {
    let short_id = output::short_id(rng);
    let hs = rng.range(2.0, 5.0);
    let period = rng.range(4.0, 9.0);
    let speed = rng.range(12.0, 24.0);
    let slew = rng.range(12.0, 70.0);
    let kp = rng.range(30.0, 160.0);
    let ki = rng.range(8.0, 90.0);
    let encounter_hz = marine::head_on_encounter_hz(period, speed);
    let omega = 2.0 * std::f64::consts::PI * encounter_hz;

    let mut pitch = 0.0;
    let mut pitch_vel = 0.0;
    let mut integral = 0.0;
    let mut foil = 0.0;
    let mut min_pitch = 0.0;
    let mut foil_at_crest = 0.0;
    let mut saw_crest = false;
    let mut submarined = false;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(hs);
    proof.feed_f64(period);
    proof.feed_f64(speed);
    proof.feed_f64(slew);
    proof.feed_f64(kp);
    proof.feed_f64(ki);

    let steps = (T_SIM * HZ) as usize;
    for tick in 0..steps {
        let t = tick as f64 * DT;
        let phase = omega * t;
        let elevation = phase.sin() * (hs / 2.0);
        let approaching_crest = phase.cos() > 0.8 && elevation > 0.0;
        let slam = if approaching_crest {
            speed * speed * 0.015
        } else {
            0.0
        };
        let hydro = slam - DAMPING_C * pitch_vel - RESTORING_K * pitch;
        let err = -pitch;
        integral += err * DT;
        let cmd = kp * err + ki * integral - 2.0 * pitch_vel;
        let max_d = slew * DT;
        let delta = (cmd - foil).clamp(-max_d, max_d);
        foil += delta;

        pitch_vel += (hydro + foil) * DT;
        pitch += pitch_vel * DT;

        if approaching_crest {
            foil_at_crest = foil;
            saw_crest = true;
        }
        if pitch < min_pitch {
            min_pitch = pitch;
        }
        if tick % 1000 == 0 {
            proof.feed_f64(pitch);
        }
        if pitch < CRITICAL_PITCH_RAD {
            submarined = true;
            break;
        }
    }

    // Bow-down foil as the crest is ridden: phase lag of the hydraulic ram.
    let phase_lagged = saw_crest && foil_at_crest < -8.0;
    proof.feed_f64(min_pitch);
    proof.feed_str(if submarined {
        "SUBMARINED"
    } else if phase_lagged {
        "FOIL_PHASE_LAG"
    } else {
        "DECK_HELD"
    });

    SlamRun {
        id,
        short_id,
        wave_height_m: hs,
        wave_period_s: period,
        usv_speed_ms: speed,
        encounter_hz,
        foil_slew_rate: slew,
        kp,
        ki,
        min_pitch_rad: min_pitch,
        foil_moment_at_crest: foil_at_crest,
        is_submarined: submarined,
        is_foil_phase_lagged: phase_lagged,
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
                "{}/../../grokd/data/usv_hull_slam.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: USV HULL SLAM  (encounter freq vs hydrofoil slew)");
    println!("  n={n}  1000 Hz  gate pitch < {CRITICAL_PITCH_RAD} rad");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x5553_565f_534c_414d);
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
        Field::new("wave_height_m", DataType::Float64, false),
        Field::new("wave_period_s", DataType::Float64, false),
        Field::new("usv_speed_ms", DataType::Float64, false),
        Field::new("encounter_hz", DataType::Float64, false),
        Field::new("foil_slew_rate", DataType::Float64, false),
        Field::new("kp", DataType::Float64, false),
        Field::new("ki", DataType::Float64, false),
        Field::new("min_pitch_rad", DataType::Float64, false),
        Field::new("foil_moment_at_crest", DataType::Float64, false),
        Field::new("is_submarined", DataType::Boolean, false),
        Field::new("is_foil_phase_lagged", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(rows.iter().map(|r| Some(format!("slam_{}", r.short_id))).collect::<StringArray>()),
            Arc::new(rows.iter().map(|r| Some(r.wave_height_m)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.wave_period_s)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.usv_speed_ms)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.encounter_hz)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.foil_slew_rate)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.kp)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.ki)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.min_pitch_rad)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.foil_moment_at_crest)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.is_submarined)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.is_foil_phase_lagged)).collect::<BooleanArray>()),
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
                "G^G USV hull slam dual-regime v1.0".to_string(),
            ),
        ]))
        .build();
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let sub = rows.iter().filter(|r| r.is_submarined).count();
    let lag = rows.iter().filter(|r| r.is_foil_phase_lagged).count();
    let both = rows
        .iter()
        .filter(|r| r.is_submarined && r.is_foil_phase_lagged)
        .count();
    let held = rows
        .iter()
        .filter(|r| !r.is_submarined && !r.is_foil_phase_lagged)
        .count();
    println!(
        "  submarined {sub} ({:.1}%)  foil_lag {lag} ({:.1}%)  both {both} ({:.1}%)  held {held} ({:.1}%)",
        100.0 * sub as f64 / n_f,
        100.0 * lag as f64 / n_f,
        100.0 * both as f64 / n_f,
        100.0 * held as f64 / n_f
    );
    println!("  seal {run_proof}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
