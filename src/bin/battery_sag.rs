//! 12S Li-ion pack voltage sag on a climb current profile.
//! Clock: 1 Hz, 45 s climb. Gates: V < 42 V (3.5 V/cell warning) vs V ≤ 36 V (3.0 V/cell cutoff).
//! Organ: battery_voltage_sag. Linear OCV vs SoC, linear ESR vs SoC — named reduced-order.

use genesis_core::output;
use genesis_core::physics::thermal::battery_voltage_sag;
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
const CLIMB_S: f64 = 45.0;
const PACK_AH: f64 = 22.0;
const V_FULL: f64 = 50.4; // 12S × 4.2 V
const V_EMPTY: f64 = 36.0; // 12S × 3.0 V
const V_WARN: f64 = 42.0;  // 12S × 3.5 V
const V_CUT: f64 = 36.0;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    initial_soc_pct: f64,
    climb_current_a: f64,
    min_terminal_v: f64,
    is_voltage_sag_warning: bool,
    is_bms_cutoff_failed: bool,
    proof_hash: String,
}

fn ocv(soc_pct: f64) -> f64 {
    V_EMPTY + (V_FULL - V_EMPTY) * (soc_pct / 100.0).clamp(0.0, 1.0)
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let soc0 = rng.range(30.0, 95.0);
    let i_climb = rng.range(18.0, 95.0); // hover 18 A through heavy climb 95 A
    let r0 = rng.range(0.040, 0.085);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(soc0);
    proof.feed_f64(i_climb);
    proof.feed_f64(r0);

    let mut soc = soc0;
    let mut min_v = ocv(soc);
    let mut t = 0.0;
    while t < CLIMB_S {
        let (v_term, _) = battery_voltage_sag(ocv(soc), i_climb, soc, r0);
        if v_term < min_v {
            min_v = v_term;
        }
        soc = (soc - (i_climb * DT / 3600.0) / PACK_AH * 100.0).max(1.0);
        t += DT;
        if t as u64 % 15 == 0 {
            proof.feed_f64(v_term);
        }
        if v_term <= V_CUT {
            break;
        }
    }

    let warn = min_v < V_WARN;
    let cut = min_v <= V_CUT;
    proof.feed_f64(min_v);
    proof.feed_str(if cut {
        "CUTOFF"
    } else if warn {
        "WARNING"
    } else {
        "NOMINAL"
    });

    Run {
        id,
        short_id,
        initial_soc_pct: (soc0 * 10.0).round() / 10.0,
        climb_current_a: (i_climb * 10.0).round() / 10.0,
        min_terminal_v: (min_v * 100.0).round() / 100.0,
        is_voltage_sag_warning: warn,
        is_bms_cutoff_failed: cut,
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
                "{}/../../data/exports/sovereign/battery_sag.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: 12S PACK SAG  (one climb current, 1 Hz)");
    println!("  n={n}  dt={DT}s  climb {CLIMB_S}s  warn {V_WARN} V  cut {V_CUT} V");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0x5A66_0002);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("initial_soc_pct", DataType::Float64, false),
        Field::new("climb_current_a", DataType::Float64, false),
        Field::new("min_terminal_v", DataType::Float64, false),
        Field::new("is_voltage_sag_warning", DataType::Boolean, false),
        Field::new("is_bms_cutoff_failed", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.initial_soc_pct).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.climb_current_a).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.min_terminal_v).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_voltage_sag_warning).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_bms_cutoff_failed).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G 12S climb sag dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let warn = rows.iter().filter(|r| r.is_voltage_sag_warning).count();
    let cut = rows.iter().filter(|r| r.is_bms_cutoff_failed).count();
    println!(
        "  warn {warn} ({:.1}%)  cutoff {cut} ({:.1}%)",
        100.0 * warn as f64 / n_f,
        100.0 * cut as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
