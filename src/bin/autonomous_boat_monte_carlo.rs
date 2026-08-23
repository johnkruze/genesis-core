//! USV roll/capsize. Metacentric restoring vs encounter-driven roll.
//! Dual-regime: capsize (|φ| > 70°) is not the same column as wave-drag speed loss.
//! Capsize is integrated, not rng.chance.

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

const HZ: f64 = 50.0;
const DT: f64 = 1.0 / HZ;
const T_SIM: f64 = 60.0;
const CAPSIZE_RAD: f64 = 1.22; // ~70°

#[derive(Debug, Serialize)]
struct BoatRun {
    id: u32,
    short_id: String,
    wave_height_m: f64,
    wave_period_s: f64,
    gm_m: f64,
    beam_m: f64,
    target_speed_ms: f64,
    encounter_hz: f64,
    roll_fn_hz: f64,
    max_roll_rad: f64,
    final_speed_ms: f64,
    is_capsized: bool,
    is_speed_limited: bool,
    proof_hash: String,
}

fn sea_label(hs: f64) -> &'static str {
    if hs < 0.5 {
        "calm"
    } else if hs < 1.25 {
        "smooth"
    } else if hs < 2.5 {
        "moderate"
    } else if hs < 4.0 {
        "rough"
    } else {
        "very_rough"
    }
}

fn run_one(id: u32, rng: &mut Rng) -> BoatRun {
    let short_id = output::short_id(rng);
    let hs = rng.range(0.2, 5.5);
    let period = rng.range(3.5, 10.0);
    let gm = rng.range(0.25, 1.30);
    let beam = rng.range(2.2, 5.5);
    let k_gyr = beam * rng.range(0.28, 0.42);
    let target = rng.range(8.0, 22.0);
    let mass = rng.range(500.0, 1600.0);
    let area = rng.range(0.35, 0.80);

    let omega_n = marine::roll_natural_omega(gm, k_gyr);
    let zeta = rng.range(0.06, 0.22);
    let enc_hz = marine::head_on_encounter_hz(period, target);
    let omega_e = 2.0 * std::f64::consts::PI * enc_hz;

    let mut phi: f64 = 0.0;
    let mut phi_d: f64 = 0.0;
    let mut speed = 0.0;
    let mut max_phi = 0.0;
    let mut capsized = false;
    let thrust = mass * 7.0;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(hs);
    proof.feed_f64(gm);
    proof.feed_f64(beam);
    proof.feed_f64(target);
    proof.feed_str(sea_label(hs));

    let steps = (T_SIM * HZ) as usize;
    for tick in 0..steps {
        let t = tick as f64 * DT;
        let wave_drag = 1.0 + 0.35 * hs;
        let drag = 0.5 * marine::RHO_SEAWATER * speed * speed * 0.45 * area * wave_drag;
        let acc = (thrust - drag) / mass;
        speed += acc * DT;
        if speed > target {
            speed = target;
        }
        let omega_f = omega_e.min(4.0);
        let forcing = (hs / beam) * omega_f * omega_f * 0.40 * (omega_e * t).sin();
        let phi_dd = -omega_n * omega_n * phi.sin() - 2.0 * zeta * omega_n * phi_d + forcing;
        phi_d += phi_dd * DT;
        phi += phi_d * DT;
        let ap = phi.abs();
        if ap > max_phi {
            max_phi = ap;
        }
        if tick % 50 == 0 {
            proof.feed_f64(phi);
        }
        if ap > CAPSIZE_RAD {
            capsized = true;
            break;
        }
    }

    let speed_limited = !capsized && speed < 0.58 * target;
    proof.feed_f64(max_phi);
    proof.feed_str(if capsized {
        "CAPSIZED"
    } else if speed_limited {
        "SPEED_LIMITED"
    } else {
        "ROUTE_HELD"
    });

    BoatRun {
        id,
        short_id,
        wave_height_m: hs,
        wave_period_s: period,
        gm_m: gm,
        beam_m: beam,
        target_speed_ms: target,
        encounter_hz: enc_hz,
        roll_fn_hz: omega_n / (2.0 * std::f64::consts::PI),
        max_roll_rad: max_phi,
        final_speed_ms: speed,
        is_capsized: capsized,
        is_speed_limited: speed_limited,
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
                "{}/../../grokd/data/usv_boat_roll.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: USV BOAT ROLL  (GM restoring vs encounter roll)");
    println!("  n={n}  {HZ} Hz  capsize {CAPSIZE_RAD} rad");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x424f_4154_524f_4c4c);
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
        Field::new("gm_m", DataType::Float64, false),
        Field::new("beam_m", DataType::Float64, false),
        Field::new("target_speed_ms", DataType::Float64, false),
        Field::new("encounter_hz", DataType::Float64, false),
        Field::new("roll_fn_hz", DataType::Float64, false),
        Field::new("max_roll_rad", DataType::Float64, false),
        Field::new("final_speed_ms", DataType::Float64, false),
        Field::new("is_capsized", DataType::Boolean, false),
        Field::new("is_speed_limited", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(rows.iter().map(|r| Some(format!("bot_{}", r.short_id))).collect::<StringArray>()),
            Arc::new(rows.iter().map(|r| Some(r.wave_height_m)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.wave_period_s)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.gm_m)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.beam_m)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.target_speed_ms)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.encounter_hz)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.roll_fn_hz)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.max_roll_rad)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.final_speed_ms)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.is_capsized)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.is_speed_limited)).collect::<BooleanArray>()),
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
                "G^G USV boat roll dual-regime v1.0".to_string(),
            ),
        ]))
        .build();
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let cap = rows.iter().filter(|r| r.is_capsized).count();
    let slow = rows.iter().filter(|r| r.is_speed_limited).count();
    let held = rows
        .iter()
        .filter(|r| !r.is_capsized && !r.is_speed_limited)
        .count();
    println!(
        "  capsized {cap} ({:.1}%)  speed_limited {slow} ({:.1}%)  held {held} ({:.1}%)",
        100.0 * cap as f64 / n_f,
        100.0 * slow as f64 / n_f,
        100.0 * held as f64 / n_f
    );
    println!("  seal {run_proof}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
