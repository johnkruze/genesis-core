//! Class-8 casing hysteresis. P = F δ rps tanδ into a lumped node. Not 100 RNG hours.
//! Mix speed/load. Clock: constitutive exact exponential (dt = 2 h). Gates: T≥95 vs blowout ≥150.

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
const EDGE_C: f64 = 95.0;
const BLOW_C: f64 = 150.0;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    target_speed_mph: f64,
    final_casing_temp_c: f64,
    is_belt_hot: bool,
    is_tread_separated: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let hot = rng.chance(0.40);
    let v_ms = if hot {
        rng.range(28.0, 36.0)
    } else {
        rng.range(16.0, 26.0)
    };
    let load = rng.range(1800.0, 3200.0);
    let amb = rng.range(28.0, 44.0);
    let tan_d = rng.range(0.10, 0.22);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(v_ms);
    proof.feed_f64(load);

    let rps = v_ms / 3.3;
    let p_w = (0.018 * load * 9.81) * rps * tan_d;
    let r_th = rng.range(0.07, 0.20);
    let mut node = LumpedThermalNode::new(amb + 12.0, 14_000.0, r_th);
    node.step(p_w, amb, 7200.0);
    let t = node.temperature_c;
    let edge = t >= EDGE_C;
    let blow = t >= BLOW_C;

    proof.feed_f64(t);
    proof.feed_str(if blow {
        "BLOWOUT"
    } else if edge {
        "HOT"
    } else {
        "OK"
    });

    Run {
        id,
        short_id,
        target_speed_mph: (v_ms / 0.44704 * 10.0).round() / 10.0,
        final_casing_temp_c: (t * 10.0).round() / 10.0,
        is_belt_hot: edge,
        is_tread_separated: blow,
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
                "{}/../../data/exports/sovereign/tire_casing_hysteresis.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    println!("====================================================================");
    println!("  G^G: TIRE HYST  (lumped node, 2 h exact)");
    println!("  n={n}");
    println!("====================================================================\n");
    let t0 = Instant::now();
    let mut rng = Rng::new(0x71A0_00C8);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("target_speed_mph", DataType::Float64, false),
        Field::new("final_casing_temp_c", DataType::Float64, false),
        Field::new("is_belt_hot", DataType::Boolean, false),
        Field::new("is_tread_separated", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.target_speed_mph).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.final_casing_temp_c).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_belt_hot).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_tread_separated).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G tire hysteresis dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let nf = n as f64;
    let a = rows.iter().filter(|r| r.is_belt_hot).count();
    let b = rows.iter().filter(|r| r.is_tread_separated).count();
    println!(
        "  hot {a} ({:.1}%)  blow {b} ({:.1}%)",
        100.0 * a as f64 / nf,
        100.0 * b as f64 / nf
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
