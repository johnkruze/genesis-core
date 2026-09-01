//! Planetary tooth Lewis bending + Basquin S-N. b = −0.09, σ_f' = 1200 MPa (4340-ish).
//! Constitutive (one impact). Gates: σ ≥ 850 MPa yield vs N_f < 1e5 mission fatigue.
//! Torque mix 10–55 N·m so a survive class exists. Organ: basquin_fatigue_life_cycles.

use genesis_core::output;
use genesis_core::physics::tribology::basquin_fatigue_life_cycles;
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
const YIELD_MPA: f64 = 850.0;
const SIGMA_F: f64 = 1200.0;
const B: f64 = -0.09;
const MISSION_CYCLES: f64 = 100_000.0;
const R_PITCH_M: f64 = 0.018;
const FACE_M: f64 = 0.008;
const MODULE_M: f64 = 0.00125;
const LEWIS_Y: f64 = 0.32;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    peak_impact_torque_nm: f64,
    tooth_shear_stress_mpa: f64,
    fatigue_life_cycles: f64,
    is_yield_stress_exceeded: bool,
    is_fatigue_life_depleted: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let torque = rng.range(10.0, 55.0);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(torque);

    let ft = torque / R_PITCH_M;
    let sigma = ft / (FACE_M * MODULE_M * LEWIS_Y) * 1e-6;
    let n_f = basquin_fatigue_life_cycles(sigma, SIGMA_F, B);
    let yld = sigma >= YIELD_MPA;
    let fat = n_f < MISSION_CYCLES;

    proof.feed_f64(sigma);
    proof.feed_f64(n_f.min(1e12));
    proof.feed_str(if yld {
        "YIELD"
    } else if fat {
        "FATIGUE"
    } else {
        "NOMINAL"
    });

    Run {
        id,
        short_id,
        peak_impact_torque_nm: (torque * 10.0).round() / 10.0,
        tooth_shear_stress_mpa: (sigma * 10.0).round() / 10.0,
        fatigue_life_cycles: n_f.min(1e9).round(),
        is_yield_stress_exceeded: yld,
        is_fatigue_life_depleted: fat,
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
                "{}/../../data/exports/sovereign/actuator_metallurgic_shear.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: LEWIS + BASQUIN  (b={B}, mission {MISSION_CYCLES:.0} cycles)");
    println!("  n={n}  yield {YIELD_MPA} MPa");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0x7181_0001);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("peak_impact_torque_nm", DataType::Float64, false),
        Field::new("tooth_shear_stress_mpa", DataType::Float64, false),
        Field::new("fatigue_life_cycles", DataType::Float64, false),
        Field::new("is_yield_stress_exceeded", DataType::Boolean, false),
        Field::new("is_fatigue_life_depleted", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.peak_impact_torque_nm).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.tooth_shear_stress_mpa).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.fatigue_life_cycles).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_yield_stress_exceeded).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_fatigue_life_depleted).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G Lewis-Basquin dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let y = rows.iter().filter(|r| r.is_yield_stress_exceeded).count();
    let f = rows.iter().filter(|r| r.is_fatigue_life_depleted).count();
    println!(
        "  yield {y} ({:.1}%)  fatigue {f} ({:.1}%)",
        100.0 * y as f64 / n_f,
        100.0 * f as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
