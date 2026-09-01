//! Li-ion cell Joule heating with optional BMS current clamp and high-T self-heat.
//! Clock: 1 Hz. Gates: current-limited (I_cmd > 60 A and clamp applied) vs
//! thermal runaway (T ≥ 150 °C). 65 °C is not runaway — that is a BMS trip.
//! Organ: LumpedThermalNode, joule_heating_watts, battery_dynamic_resistance_ohms.

use genesis_core::output;
use genesis_core::physics::thermal::{
    battery_dynamic_resistance_ohms, joule_heating_watts, LumpedThermalNode,
};
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
const HORIZON_S: f64 = 600.0;
const I_LIMIT_A: f64 = 60.0;
const T_FOLD_C: f64 = 80.0;
const T_CHEM_C: f64 = 120.0;
const T_RUNAWAY_C: f64 = 150.0;
const CELL_AH: f64 = 5.0;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    i_cmd_a: f64,
    derate: f64,
    k_runaway: f64,
    peak_temp_c: f64,
    is_current_limited: bool,
    is_thermal_runaway: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let amb_c = rng.range(20.0, 45.0);
    let i_cmd = rng.range(25.0, 95.0);
    let r0 = rng.range(0.012, 0.028);
    let derate = rng.range(0.0, 1.0);
    let k_run = rng.range(0.020, 0.090);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(amb_c);
    proof.feed_f64(i_cmd);
    proof.feed_f64(r0);
    proof.feed_f64(derate);
    proof.feed_f64(k_run);

    let mut node = LumpedThermalNode::new(amb_c, 300.0, 0.45);
    let mut soc = 90.0;
    let mut peak = amb_c;
    let mut applied_clamp = false;
    let mut runaway = false;
    let mut elapsed = 0.0;

    while elapsed < HORIZON_S {
        let mut i = i_cmd;
        if i > I_LIMIT_A {
            i = I_LIMIT_A + (i_cmd - I_LIMIT_A) * (1.0 - derate);
            applied_clamp = derate > 0.05;
        }
        if node.temperature_c > T_FOLD_C {
            i *= 0.15 + 0.85 * (1.0 - derate);
        }

        let r_int = battery_dynamic_resistance_ohms(r0, soc);
        let mut q = joule_heating_watts(i, r_int);
        if node.temperature_c > T_CHEM_C {
            q += 300.0 * k_run * (node.temperature_c - T_CHEM_C);
        }

        let t = node.step(q, amb_c, DT);
        soc = (soc - (i * DT / 3600.0) / CELL_AH * 100.0).max(5.0);
        elapsed += DT;
        if t > peak {
            peak = t;
        }
        if t >= T_RUNAWAY_C {
            runaway = true;
            break;
        }
        if elapsed as u64 % 60 == 0 {
            proof.feed_f64(t);
        }
    }

    proof.feed_f64(peak);
    proof.feed_str(if runaway {
        "RUNAWAY"
    } else if applied_clamp {
        "CLAMPED"
    } else {
        "JOULE_OK"
    });

    Run {
        id,
        short_id,
        i_cmd_a: (i_cmd * 10.0).round() / 10.0,
        derate: (derate * 1000.0).round() / 1000.0,
        k_runaway: (k_run * 1000.0).round() / 1000.0,
        peak_temp_c: (peak * 10.0).round() / 10.0,
        is_current_limited: applied_clamp,
        is_thermal_runaway: runaway,
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
                "{}/../../data/exports/sovereign/battery_thermal_runaway.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: LI-ION JOULE + SELF-HEAT  (clamp vs T≥{T_RUNAWAY_C} °C)");
    println!("  n={n}  dt={DT}s  I_lim {I_LIMIT_A} A  fold {T_FOLD_C} °C  chem {T_CHEM_C} °C");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0xBA77_0001);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("i_cmd_a", DataType::Float64, false),
        Field::new("derate", DataType::Float64, false),
        Field::new("k_runaway", DataType::Float64, false),
        Field::new("peak_temp_c", DataType::Float64, false),
        Field::new("is_current_limited", DataType::Boolean, false),
        Field::new("is_thermal_runaway", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.i_cmd_a).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.derate).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.k_runaway).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.peak_temp_c).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_current_limited).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_thermal_runaway).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(
        &seal,
        "G^G Li-ion Joule + self-heat dual-regime v3.0",
    );
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let clamp = rows.iter().filter(|r| r.is_current_limited).count();
    let run = rows.iter().filter(|r| r.is_thermal_runaway).count();
    let held = rows
        .iter()
        .filter(|r| r.is_current_limited && !r.is_thermal_runaway)
        .count();
    println!(
        "  current_limited {clamp} ({:.1}%)  runaway {run} ({:.1}%)  clamped-held {held} ({:.1}%)",
        100.0 * clamp as f64 / n_f,
        100.0 * run as f64 / n_f,
        100.0 * held as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
