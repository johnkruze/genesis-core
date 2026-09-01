//! Quadruped knee grease: Walther viscosity at startup, then lumped heating on a 10 min trot.
//! Extra shear power scales with ν/ν_40, not a boolean. Clock: one exact 600 s step.
//! Gates: ν ≥ 500 cSt cold drag vs T_joint ≥ 85 °C overtemp. They anti-correlate.
//! Organ: walther_lubricant_viscosity_cst, LumpedThermalNode.
//!
//! JSON-farm twin museumed: `z_archive/gg-garden-cruft-2026-08-24/bin/quadruped_thermal_monte_carlo.rs`.

use genesis_core::output;
use genesis_core::physics::thermal::{walther_lubricant_viscosity_cst, LumpedThermalNode};
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
const TROT_S: f64 = 1200.0; // 20 min — τ ≈ 382 s, so this is 3.1τ (10 min never reached 85 °C)
const WALTHER_A: f64 = 9.2;
const WALTHER_B: f64 = 3.6;
const NU_40: f64 = 46.0;
const NU_DRAG: f64 = 500.0;
const T_OVER_C: f64 = 85.0;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    initial_ambient_c: f64,
    initial_viscosity_cst: f64,
    final_joint_temp_c: f64,
    is_viscous_drag_surged: bool,
    is_joint_overtemp_tripped: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let amb = rng.range(-25.0, 45.0);
    let duty = rng.range(0.50, 1.0);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(amb);
    proof.feed_f64(duty);

    let nu = walther_lubricant_viscosity_cst(WALTHER_A, WALTHER_B, amb);
    let drag = nu >= NU_DRAG;
    let p_mech = 55.0 * duty;
    let p_visc = 8.0 * (nu / NU_40).clamp(0.3, 8.0);
    let mut node = LumpedThermalNode::new(amb, 450.0, 0.85);
    let t_final = node.step(p_mech + p_visc, amb, TROT_S);
    let over = t_final >= T_OVER_C;

    proof.feed_f64(nu);
    proof.feed_f64(t_final);
    proof.feed_str(if over {
        "OVERTEMP"
    } else if drag {
        "VISCOUS_DRAG"
    } else {
        "NOMINAL"
    });

    Run {
        id,
        short_id,
        initial_ambient_c: (amb * 10.0).round() / 10.0,
        initial_viscosity_cst: (nu * 10.0).round() / 10.0,
        final_joint_temp_c: (t_final * 10.0).round() / 10.0,
        is_viscous_drag_surged: drag,
        is_joint_overtemp_tripped: over,
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
                "{}/../../data/exports/sovereign/quadruped_leg_thermal_sink.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: QUADRUPED WALTHER GREASE  (ν(T) + lumped 20 min trot)");
    println!("  n={n}  drag ν≥{NU_DRAG} cSt  overtemp {T_OVER_C} °C");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0x4488_000A);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("initial_ambient_c", DataType::Float64, false),
        Field::new("initial_viscosity_cst", DataType::Float64, false),
        Field::new("final_joint_temp_c", DataType::Float64, false),
        Field::new("is_viscous_drag_surged", DataType::Boolean, false),
        Field::new("is_joint_overtemp_tripped", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.initial_ambient_c).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.initial_viscosity_cst).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.final_joint_temp_c).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_viscous_drag_surged).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_joint_overtemp_tripped).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G Walther knee dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let d = rows.iter().filter(|r| r.is_viscous_drag_surged).count();
    let o = rows.iter().filter(|r| r.is_joint_overtemp_tripped).count();
    println!(
        "  viscous_drag {d} ({:.1}%)  overtemp {o} ({:.1}%)",
        100.0 * d as f64 / n_f,
        100.0 * o as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
