//! Vacuum outgassing of polymeric adhesives. Arrhenius Ea = 0.5 eV (water/organic).
//! Lumped soak against a −20 °C radiative sink, then flux ratio vs 20 °C baseline.
//! Clock: one exact 2 h step. Gates: 5× desorption surge vs 20× optical fogging.

use genesis_core::output;
use genesis_core::physics::thermal::{arrhenius_outgassing_rate, LumpedThermalNode};
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
const EA_EV: f64 = 0.5;
const SOAK_S: f64 = 7200.0;
const SURGE_X: f64 = 5.0;
const FOG_X: f64 = 20.0;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    final_payload_temp_c: f64,
    outgassing_flux_multiplier: f64,
    is_desorption_surged: bool,
    is_optical_fogging_failed: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let t0 = rng.range(10.0, 30.0);
    let q_w = rng.range(10.0, 90.0); // eclipse through moderate sun-pointing load

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(t0);
    proof.feed_f64(q_w);

    let mut node = LumpedThermalNode::new(t0, 1200.0, 1.2);
    let t_final = node.step(q_w, -20.0, SOAK_S);
    let baseline = arrhenius_outgassing_rate(EA_EV, 20.0, 1.0);
    let rate = arrhenius_outgassing_rate(EA_EV, t_final, 1.0);
    let x = rate / baseline.max(1e-30);
    let surged = x >= SURGE_X;
    let fogged = x >= FOG_X;

    proof.feed_f64(t_final);
    proof.feed_f64(x);
    proof.feed_str(if fogged {
        "FOGGED"
    } else if surged {
        "SURGED"
    } else {
        "NOMINAL"
    });

    Run {
        id,
        short_id,
        final_payload_temp_c: (t_final * 10.0).round() / 10.0,
        outgassing_flux_multiplier: (x * 100.0).round() / 100.0,
        is_desorption_surged: surged,
        is_optical_fogging_failed: fogged,
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
                "{}/../../data/exports/sovereign/thermal_outgassing.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: ARRHENIUS OUTGASSING  (Ea={EA_EV} eV, 2 h soak)");
    println!("  n={n}  surge {SURGE_X}×  fog {FOG_X}×");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0x4421_0007);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("final_payload_temp_c", DataType::Float64, false),
        Field::new("outgassing_flux_multiplier", DataType::Float64, false),
        Field::new("is_desorption_surged", DataType::Boolean, false),
        Field::new("is_optical_fogging_failed", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.final_payload_temp_c).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.outgassing_flux_multiplier).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_desorption_surged).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_optical_fogging_failed).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G Arrhenius 0.5 eV dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let s = rows.iter().filter(|r| r.is_desorption_surged).count();
    let f = rows.iter().filter(|r| r.is_optical_fogging_failed).count();
    println!(
        "  surged {s} ({:.1}%)  fogged {f} ({:.1}%)",
        100.0 * s as f64 / n_f,
        100.0 * f as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
