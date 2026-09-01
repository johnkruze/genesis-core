//! Nitrile shaft-seal friction vs temperature. Constitutive (Tg = −25 °C).
//! Sub-Tg stiffening, above-Tg bore expansion. Organ: elastomeric_seal_friction_surge.
//! Gates: μ ≥ 0.18 surge (1.5×) vs μ ≥ 0.30 stall (2.5×). Envelope −50 °C to 95 °C.

use genesis_core::output;
use genesis_core::physics::thermal::elastomeric_seal_friction_surge;
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
const MU0: f64 = 0.12;
const TG_C: f64 = -25.0;
const SURGE: f64 = 0.18;
const STALL: f64 = 0.30;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    operating_temp_c: f64,
    effective_friction_mu: f64,
    is_friction_surge: bool,
    is_actuator_stalled: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let t_c = rng.range(-50.0, 95.0);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(t_c);

    let mu = elastomeric_seal_friction_surge(t_c, TG_C, MU0);
    let surge = mu >= SURGE;
    let stall = mu >= STALL;
    proof.feed_f64(mu);
    proof.feed_str(if stall {
        "STALLED"
    } else if surge {
        "SURGED"
    } else {
        "NOMINAL"
    });

    Run {
        id,
        short_id,
        operating_temp_c: (t_c * 10.0).round() / 10.0,
        effective_friction_mu: (mu * 1000.0).round() / 1000.0,
        is_friction_surge: surge,
        is_actuator_stalled: stall,
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
                "{}/../../data/exports/sovereign/thermal_seal_friction.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: NITRILE SEAL FRICTION  (Tg={TG_C} °C)");
    println!("  n={n}  surge μ≥{SURGE}  stall μ≥{STALL}");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0x9923_0008);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("operating_temp_c", DataType::Float64, false),
        Field::new("effective_friction_mu", DataType::Float64, false),
        Field::new("is_friction_surge", DataType::Boolean, false),
        Field::new("is_actuator_stalled", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.operating_temp_c).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.effective_friction_mu).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_friction_surge).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_actuator_stalled).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G nitrile seal μ(T) dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let s = rows.iter().filter(|r| r.is_friction_surge).count();
    let st = rows.iter().filter(|r| r.is_actuator_stalled).count();
    println!(
        "  surge {s} ({:.1}%)  stall {st} ({:.1}%)",
        100.0 * s as f64 / n_f,
        100.0 * st as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
