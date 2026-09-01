//! Sealed IP67 enclosure. Solar G·A·α plus compute, lumped RC to ambient.
//! Clock: 1 Hz thermal, 10 s exact exponential chunks, 4 h soak.
//! Gates: throttle T > 70 °C (compute × 0.5) vs shutdown T > 85 °C.
//! Envelope mixes night/shade (G=0) with desert midday so a survive class exists.

use genesis_core::output;
use genesis_core::physics::thermal::LumpedThermalNode;
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
const CHUNK: f64 = 10.0;
const HORIZON_S: f64 = 4.0 * 3600.0;
const T_THROTTLE: f64 = 70.0;
const T_SHUT: f64 = 85.0;
const AREA_M2: f64 = 0.05;
const ALPHA: f64 = 0.85;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    solar_flux_w_m2: f64,
    compute_w: f64,
    peak_temp_c: f64,
    is_thermal_throttled: bool,
    is_overtemp_shutdown: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let amb = rng.range(18.0, 50.0);
    let solar = rng.range(0.0, 1150.0); // night through desert
    let compute = rng.range(12.0, 95.0);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(amb);
    proof.feed_f64(solar);
    proof.feed_f64(compute);

    let q_solar = solar * AREA_M2 * ALPHA;
    let mut node = LumpedThermalNode::new(amb, 2250.0, 0.55);
    let mut throttled = false;
    let mut shutdown = false;
    let mut peak = amb;
    let mut elapsed = 0.0;

    while elapsed < HORIZON_S {
        let p = if throttled { compute * 0.50 } else { compute };
        let t = node.step(p + q_solar, amb, CHUNK);
        elapsed += CHUNK;
        if t > peak {
            peak = t;
        }
        if t >= T_THROTTLE {
            throttled = true;
        }
        if t >= T_SHUT {
            shutdown = true;
            break;
        }
        if (elapsed as u64) % 600 == 0 {
            proof.feed_f64(t);
        }
    }

    proof.feed_f64(peak);
    proof.feed_str(if shutdown {
        "SHUTDOWN"
    } else if throttled {
        "THROTTLED"
    } else {
        "NOMINAL"
    });

    Run {
        id,
        short_id,
        solar_flux_w_m2: (solar * 10.0).round() / 10.0,
        compute_w: (compute * 10.0).round() / 10.0,
        peak_temp_c: (peak * 10.0).round() / 10.0,
        is_thermal_throttled: throttled,
        is_overtemp_shutdown: shutdown,
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
                "{}/../../data/exports/sovereign/ip67_heat_soak.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: IP67 HEAT SOAK  (G·A·α + compute, 1 Hz / 10 s chunks)");
    println!("  n={n}  horizon {HORIZON_S}s  throttle {T_THROTTLE} °C  shut {T_SHUT} °C");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0x1967_0003);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("solar_flux_w_m2", DataType::Float64, false),
        Field::new("compute_w", DataType::Float64, false),
        Field::new("peak_temp_c", DataType::Float64, false),
        Field::new("is_thermal_throttled", DataType::Boolean, false),
        Field::new("is_overtemp_shutdown", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.solar_flux_w_m2).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.compute_w).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.peak_temp_c).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_thermal_throttled).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_overtemp_shutdown).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G IP67 GAα soak dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let th = rows.iter().filter(|r| r.is_thermal_throttled).count();
    let sh = rows.iter().filter(|r| r.is_overtemp_shutdown).count();
    println!(
        "  throttled {th} ({:.1}%)  shutdown {sh} ({:.1}%)",
        100.0 * th as f64 / n_f,
        100.0 * sh as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
