//! CMOS die thermal SNR. Dark current doubles every 7 °C — that is the cliff.
//! Johnson–Nyquist is ~0.7 dB at 70 °C and is not the product.
//! Clock: 1 Hz, 180 s soak. Gates: SNR ≤ 15 dB degraded vs SNR ≤ 8 dB starved.
//! Throttle at 80 °C cuts power (helps dark current) and shortens integration (−3 dB).

use genesis_core::output;
use genesis_core::physics::thermal::{cmos_dark_current_snr_db, LumpedThermalNode};
use genesis_core::proof::{self, ProofChain};
use genesis_core::rng::Rng;
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{BooleanArray, Float64Array, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::ArrowWriter;

const DEFAULT_N: usize = 2500;
const DT: f64 = 1.0;
const HORIZON_S: f64 = 180.0;
const SNR0: f64 = 38.0;
const T_REF_C: f64 = 20.0;
const T_DOUBLE_C: f64 = 7.0;
const SNR_DEGRADE: f64 = 15.0;
const SNR_STARVE: f64 = 8.0;
const T_THROTTLE_C: f64 = 80.0;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    sensor_temp_c: f64,
    final_snr_db: f64,
    is_snr_degraded: bool,
    is_sensor_starved: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let amb = rng.range(20.0, 55.0);
    let p0 = rng.range(6.0, 22.0);
    let r_th = rng.range(1.8, 7.0);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(amb);
    proof.feed_f64(p0);
    proof.feed_f64(r_th);

    let mut node = LumpedThermalNode::new(amb, 15.0, r_th);
    let mut throttled = false;
    let mut snr = SNR0;
    let mut peak = amb;
    let mut t = 0.0;
    while t < HORIZON_S {
        let p = if throttled { p0 * 0.40 } else { p0 };
        let die = node.step(p, amb, DT);
        if die > peak {
            peak = die;
        }
        if die >= T_THROTTLE_C {
            throttled = true;
        }
        snr = cmos_dark_current_snr_db(SNR0, die, T_REF_C, T_DOUBLE_C);
        if throttled {
            snr -= 3.0; // shorter integration after clock cut
        }
        t += DT;
        if snr <= SNR_STARVE {
            break;
        }
    }

    let degraded = snr <= SNR_DEGRADE;
    let starved = snr <= SNR_STARVE;
    proof.feed_f64(peak);
    proof.feed_f64(snr);
    proof.feed_str(if starved {
        "STARVED"
    } else if degraded {
        "DEGRADED"
    } else {
        "NOMINAL"
    });

    Run {
        id,
        short_id,
        sensor_temp_c: (peak * 10.0).round() / 10.0,
        final_snr_db: (snr * 10.0).round() / 10.0,
        is_snr_degraded: degraded,
        is_sensor_starved: starved,
        proof_hash: proof.seal(),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(DEFAULT_N);
    let out = args
        .iter()
        .position(|a| a == "--parquet")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| {
            format!(
                "{}/../../data/exports/sovereign/thermal_sensor_starvation.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: CMOS DARK-CURRENT SNR  (doubling {T_DOUBLE_C} °C, 1 Hz)");
    println!("  n={n}  degrade ≤{SNR_DEGRADE} dB  starve ≤{SNR_STARVE} dB");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0x8891_0006);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("sensor_temp_c", DataType::Float64, false),
        Field::new("final_snr_db", DataType::Float64, false),
        Field::new("is_snr_degraded", DataType::Boolean, false),
        Field::new("is_sensor_starved", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.sensor_temp_c).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.final_snr_db).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_snr_degraded).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_sensor_starved).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G CMOS dark-current SNR dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let d = rows.iter().filter(|r| r.is_snr_degraded).count();
    let s = rows.iter().filter(|r| r.is_sensor_starved).count();
    println!(
        "  degraded {d} ({:.1}%)  starved {s} ({:.1}%)",
        100.0 * d as f64 / n_f,
        100.0 * s as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
