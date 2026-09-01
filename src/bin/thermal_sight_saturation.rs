//! MWIR well fill under AGC. Background held at 40 % well; a hot source of fill-factor f
//! in the same pixel fills (1−f)a + f a (T_s/T_bg)⁴.
//! Distance sets angular fill: f = clamp((D/r) / IFOV, 0.02, 1). Organ: agc_blackbody_well_fill.
//! Gates: well ≥ 0.80 saturated vs well ≥ 0.99 blinded.

use genesis_core::output;
use genesis_core::physics::thermal::agc_blackbody_well_fill;
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
const IFOV_RAD: f64 = 0.001;
const AGC_WELL: f64 = 0.40;
const SAT: f64 = 0.80;
const BLIND: f64 = 0.99;
const T_BG_K: f64 = 300.0;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    target_emitter_temp_c: f64,
    range_m: f64,
    source_fill_factor: f64,
    fpa_well_fill_ratio: f64,
    is_well_saturated: bool,
    is_contrast_blinded: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    // Mix: unresolved exhaust/spark (typical) vs resolved furnace/flare (tail).
    let t_c = if rng.chance(0.18) {
        rng.range(450.0, 900.0)
    } else {
        rng.range(40.0, 280.0)
    };
    let range_m = rng.range(120.0, 500.0);
    let source_m = rng.range(0.03, 0.12);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(t_c);
    proof.feed_f64(range_m);
    proof.feed_f64(source_m);

    let angular = source_m / range_m.max(1.0);
    let fill = (angular / IFOV_RAD).clamp(0.02, 1.0);
    let well = agc_blackbody_well_fill(t_c + 273.15, T_BG_K, fill, AGC_WELL);
    let sat = well >= SAT;
    let blind = well >= BLIND;

    proof.feed_f64(fill);
    proof.feed_f64(well);
    proof.feed_str(if blind {
        "BLINDED"
    } else if sat {
        "SATURATED"
    } else {
        "NOMINAL"
    });

    Run {
        id,
        short_id,
        target_emitter_temp_c: (t_c * 10.0).round() / 10.0,
        range_m: (range_m * 10.0).round() / 10.0,
        source_fill_factor: (fill * 1000.0).round() / 1000.0,
        fpa_well_fill_ratio: (well * 1000.0).round() / 1000.0,
        is_well_saturated: sat,
        is_contrast_blinded: blind,
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
                "{}/../../data/exports/sovereign/thermal_sight_saturation.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: MWIR AGC WELL FILL  (T⁴ contrast, fill from D/r)");
    println!("  n={n}  sat ≥{SAT}  blind ≥{BLIND}");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0x3819_000B);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("target_emitter_temp_c", DataType::Float64, false),
        Field::new("range_m", DataType::Float64, false),
        Field::new("source_fill_factor", DataType::Float64, false),
        Field::new("fpa_well_fill_ratio", DataType::Float64, false),
        Field::new("is_well_saturated", DataType::Boolean, false),
        Field::new("is_contrast_blinded", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.target_emitter_temp_c).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.range_m).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.source_fill_factor).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.fpa_well_fill_ratio).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_well_saturated).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_contrast_blinded).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G MWIR AGC T^4 well dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let sat = rows.iter().filter(|r| r.is_well_saturated).count();
    let blind = rows.iter().filter(|r| r.is_contrast_blinded).count();
    println!(
        "  saturated {sat} ({:.1}%)  blinded {blind} ({:.1}%)",
        100.0 * sat as f64 / n_f,
        100.0 * blind as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
