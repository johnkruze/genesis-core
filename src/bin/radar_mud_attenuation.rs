//! X-band APS radome mud. R ∝ P^{1/4}. 2 dB/cm named (not 8.5 dB/cm costume).
//! Mix thin film vs caked. Clock: constitutive. Gates: range < 90 m vs < 22 m (can't arm).

use genesis_core::output;
use genesis_core::physics::optics::{radar_attenuated_range_m, XBAND_WET_MUD_DB_PER_CM};
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
const R0: f64 = 350.0;
const DEGRADED_M: f64 = 90.0;
const ARM_M: f64 = 22.0;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    mud_thickness_cm: f64,
    max_effective_range_m: f64,
    is_range_degraded: bool,
    is_intercept_failed: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let caked = rng.chance(0.38);
    let th = if caked {
        rng.range(7.0, 18.0)
    } else {
        rng.range(0.4, 6.0)
    };

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(th);

    let r = radar_attenuated_range_m(R0, th, XBAND_WET_MUD_DB_PER_CM);
    let degraded = r < DEGRADED_M;
    let fail = r < ARM_M;

    proof.feed_f64(r);
    proof.feed_str(if fail {
        "NO_ARM"
    } else if degraded {
        "DEGRADED"
    } else {
        "LIVE"
    });

    Run {
        id,
        short_id,
        mud_thickness_cm: (th * 10.0).round() / 10.0,
        max_effective_range_m: (r * 10.0).round() / 10.0,
        is_range_degraded: degraded,
        is_intercept_failed: fail,
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
                "{}/../../data/exports/sovereign/radar_mud_attenuation.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    println!("====================================================================");
    println!("  G^G: X-BAND MUD  (2 dB/cm, R~P^1/4)");
    println!("  n={n}");
    println!("====================================================================\n");
    let t0 = Instant::now();
    let mut rng = Rng::new(0x9180_00F3);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("mud_thickness_cm", DataType::Float64, false),
        Field::new("max_effective_range_m", DataType::Float64, false),
        Field::new("is_range_degraded", DataType::Boolean, false),
        Field::new("is_intercept_failed", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.mud_thickness_cm).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.max_effective_range_m).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_range_degraded).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_intercept_failed).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G X-band mud dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let nf = n as f64;
    let a = rows.iter().filter(|r| r.is_range_degraded).count();
    let b = rows.iter().filter(|r| r.is_intercept_failed).count();
    println!(
        "  degraded {a} ({:.1}%)  no_arm {b} ({:.1}%)",
        100.0 * a as f64 / nf,
        100.0 * b as f64 / nf
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
