//! Reactor Xe-135 pit. Hours-scale iodine/xenon; prompt watch after rod pull.
//! Exclusive two-way: prompt-critical · pit-survived.
//! Remainder of “not prompt” is pit-survived — named, re-sealed. No FFI.

use genesis_core::output;
use genesis_core::physics::reactor::ReactorState;
use genesis_core::proof;
use genesis_core::rng::Rng;
use rayon::prelude::*;
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{BooleanArray, Float64Array, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::ArrowWriter;

const DEFAULT_N: usize = 2500;
const DT_MACRO: f64 = 10.0; // s — xenon pit hours
const DT_MICRO: f64 = 1.0; // s — prompt watch
const STABILIZE_H: f64 = 12.0;
const WATCH_H: f64 = 10.0;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    core_age_days: f64,
    delta_rho: f64,
    initial_flux: f64,
    base_rho: f64,
    pit_duration_hours: f64,
    xenon_worth_at_pull: f64,
    time_to_criticality_s: f64,
    beta_eff_at_end: f64,
    is_prompt_critical: bool,
    is_pit_survived: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let core_age_days = rng.range(0.0, 400.0);
    let delta_rho = rng.range(0.054, 0.057);
    let initial_flux = rng.range(5e12, 2e13);
    let base_rho = rng.range(0.18, 0.216);
    let pit_duration_hours = rng.range(10.0, 28.0);

    let mut reactor = ReactorState::new();
    reactor.flux = initial_flux;
    reactor.base_rho = base_rho;
    reactor.control_rods_rho = -base_rho;
    reactor.days_burned = core_age_days;
    reactor.proof.seed(&id.to_le_bytes());
    reactor.proof.feed_f64(core_age_days);
    reactor.proof.feed_f64(delta_rho);
    reactor.proof.feed_f64(pit_duration_hours);

    let n_stab = ((STABILIZE_H * 3600.0) / DT_MACRO) as u64;
    for _ in 0..n_stab {
        reactor.step(DT_MACRO);
    }

    reactor.control_rods_rho -= 0.05;
    let n_pit = ((pit_duration_hours * 3600.0) / DT_MACRO) as u64;
    for _ in 0..n_pit {
        reactor.step(DT_MACRO);
    }

    let xenon_worth_at_pull = reactor.xenon_reactivity_worth();
    reactor.pull_control_rods(delta_rho);

    let n_watch = ((WATCH_H * 3600.0) / DT_MICRO) as u64;
    for _ in 0..n_watch {
        if reactor.prompt_critical {
            break;
        }
        reactor.step(DT_MICRO);
    }

    let is_prompt_critical = reactor.prompt_critical;
    let is_pit_survived = !is_prompt_critical;
    let class = if is_prompt_critical {
        "PROMPT_CRITICAL"
    } else {
        "PIT_SURVIVED"
    };
    reactor.proof.feed_str(class);

    let time_to_criticality_s = if is_prompt_critical { reactor.time_s } else { 0.0 };
    let beta_eff_at_end = reactor.effective_beta();
    let proof_hash = reactor.get_sealed_hash();

    Run {
        id,
        short_id,
        core_age_days,
        delta_rho,
        initial_flux,
        base_rho,
        pit_duration_hours,
        xenon_worth_at_pull,
        time_to_criticality_s,
        beta_eff_at_end,
        is_prompt_critical,
        is_pit_survived,
        proof_hash,
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
                "{}/../../data/exports/sovereign/reactor_xenon_pit.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: REACTOR XE-135 PIT  (hours iodine/xenon · prompt watch)");
    println!("  n={n}  exclusive PROMPT_CRITICAL / PIT_SURVIVED");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let rows: Vec<Run> = (0..n as u32)
        .into_par_iter()
        .map(|i| {
            let mut rng = Rng::new(0x135E_0002u64.wrapping_add(i as u64).wrapping_mul(0x9E3779B97F4A7C15));
            run_one(i, &mut rng)
        })
        .collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let unique: std::collections::HashSet<&str> = proofs.iter().map(|s| s.as_str()).collect();
    assert_eq!(unique.len(), n, "proof_hash must be unique");
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("core_age_days", DataType::Float64, false),
        Field::new("delta_rho", DataType::Float64, false),
        Field::new("initial_flux", DataType::Float64, false),
        Field::new("base_rho", DataType::Float64, false),
        Field::new("pit_duration_hours", DataType::Float64, false),
        Field::new("xenon_worth_at_pull", DataType::Float64, false),
        Field::new("time_to_criticality_s", DataType::Float64, false),
        Field::new("beta_eff_at_end", DataType::Float64, false),
        Field::new("is_prompt_critical", DataType::Boolean, false),
        Field::new("is_pit_survived", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.core_age_days).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.delta_rho).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.initial_flux).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.base_rho).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.pit_duration_hours).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.xenon_worth_at_pull).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.time_to_criticality_s).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.beta_eff_at_end).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_prompt_critical).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_pit_survived).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G reactor xenon pit dual-regime v1.1");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let nf = n as f64;
    let prompt = rows.iter().filter(|r| r.is_prompt_critical).count();
    let survived = rows.iter().filter(|r| r.is_pit_survived).count();
    let both = rows
        .iter()
        .filter(|r| r.is_prompt_critical == r.is_pit_survived)
        .count();
    println!(
        "  exclusive: prompt-critical {prompt} ({:.1}%)  pit-survived {survived} ({:.1}%)  sum {}  overlap {both}",
        100.0 * prompt as f64 / nf,
        100.0 * survived as f64 / nf,
        prompt + survived
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
