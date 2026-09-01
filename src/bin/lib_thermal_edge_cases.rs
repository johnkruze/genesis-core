//! Stuck actuator I²R. Cluster A joule + lumped node. Sovereign path, not commercial/.
//! Mix current. Clock: 1 Hz, 8 min. Gates: T≥130 °C insulation vs T≥180 °C bus collapse.

use genesis_core::output;
use genesis_core::physics::thermal::{joule_heating_watts, LumpedThermalNode};
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
const HORIZON_S: f64 = 480.0;
const INSUL_C: f64 = 130.0;
const BUS_C: f64 = 180.0;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    current_a: f64,
    final_winding_temp_c: f64,
    is_insulation_hot: bool,
    is_bus_collapse: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let stuck = rng.chance(0.38);
    let i_a = if stuck {
        rng.range(90.0, 180.0)
    } else {
        rng.range(25.0, 75.0)
    };
    let r0 = rng.range(0.04, 0.12);
    let amb = rng.range(18.0, 48.0);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(i_a);
    proof.feed_f64(r0);

    let mut node = LumpedThermalNode::new(amb, rng.range(400.0, 1200.0), rng.range(0.08, 0.22));
    let steps = (HORIZON_S / DT) as usize;
    for k in 0..steps {
        let r = r0 * (1.0 + 0.004 * (node.temperature_c - 20.0).max(0.0));
        let p = joule_heating_watts(i_a, r);
        node.step(p, amb, DT);
        if k % 60 == 0 {
            proof.feed_f64(node.temperature_c);
        }
    }
    let t = node.temperature_c;
    let ins = t >= INSUL_C;
    let bus = t >= BUS_C;

    proof.feed_f64(t);
    proof.feed_str(if bus {
        "BUS"
    } else if ins {
        "INSUL"
    } else {
        "OK"
    });

    Run {
        id,
        short_id,
        current_a: (i_a * 10.0).round() / 10.0,
        final_winding_temp_c: (t * 10.0).round() / 10.0,
        is_insulation_hot: ins,
        is_bus_collapse: bus,
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
                "{}/../../data/exports/sovereign/lib_thermal_edge_cases.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    println!("====================================================================");
    println!("  G^G: I2R WINDING  (joule + lumped, 1 Hz, sovereign)");
    println!("  n={n}");
    println!("====================================================================\n");
    let t0 = Instant::now();
    let mut rng = Rng::new(0x1928_4499);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("current_a", DataType::Float64, false),
        Field::new("final_winding_temp_c", DataType::Float64, false),
        Field::new("is_insulation_hot", DataType::Boolean, false),
        Field::new("is_bus_collapse", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.current_a).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.final_winding_temp_c).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_insulation_hot).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_bus_collapse).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G I2R winding dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let nf = n as f64;
    let a = rows.iter().filter(|r| r.is_insulation_hot).count();
    let b = rows.iter().filter(|r| r.is_bus_collapse).count();
    println!(
        "  insul {a} ({:.1}%)  bus {b} ({:.1}%)",
        100.0 * a as f64 / nf,
        100.0 * b as f64 / nf
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
