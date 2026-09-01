//! Liquid payload on a balancer. k_p > m g h, then slosh as the perturbation.
//! Clock: 200 Hz, 4 s. Gates: slosh coupled vs flip |θ|≥0.35 rad.
//! Organ: InvertedPendulum, DynamicOscillator. Flip gate is a balancer's, not 57°.

use genesis_core::output;
use genesis_core::physics::resonance::{
    pd_ankle_torque_nm, DynamicOscillator, InvertedPendulum,
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
const DT: f64 = 0.005;
const HORIZON_S: f64 = 4.0;
const FLIP: f64 = 0.35;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    slosh_frac: f64,
    max_pitch_rad: f64,
    is_slosh_coupled: bool,
    is_robot_flipped: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let m_bot = rng.range(22.0, 32.0);
    let m_pay = rng.range(4.0, 18.0);
    let h = rng.range(0.28, 0.42);
    let slosh_f = rng.range(0.8, 2.4);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(m_pay);
    proof.feed_f64(slosh_f);

    let mut plant = InvertedPendulum::new(0.03, h, m_bot + m_pay);
    let kp = plant.mgh_nm_per_rad() * rng.range(1.25, 1.85);
    let kd = 2.0 * (kp * plant.inertia_kg_m2()).sqrt() * rng.range(0.4, 0.85);
    let mut slosh = DynamicOscillator::new(slosh_f, 0.08);
    let coupled = m_pay / (m_bot + m_pay) > 0.28;
    let mut peak: f64 = 0.03;
    let steps = (HORIZON_S / DT) as usize;
    for k in 0..steps {
        let t = k as f64 * DT;
        let (x, _) = slosh.step(4.0 * (slosh.natural_frequency_rad_s * t).sin(), DT);
        let tau_pd = pd_ankle_torque_nm(plant.theta_rad, plant.omega_rad_s, kp, kd, 80.0);
        let tau_slosh = m_pay * 9.81 * x;
        plant.step(tau_pd - tau_slosh, DT);
        peak = peak.max(plant.theta_rad.abs());
        if peak >= FLIP {
            break;
        }
        if k % 40 == 0 {
            proof.feed_f64(plant.theta_rad);
        }
    }
    let flip = peak >= FLIP;
    proof.feed_f64(peak);
    proof.feed_str(if flip {
        "FLIP"
    } else if coupled {
        "SLOSH"
    } else {
        "HELD"
    });

    Run {
        id,
        short_id,
        slosh_frac: (m_pay / (m_bot + m_pay) * 1000.0).round() / 1000.0,
        max_pitch_rad: (peak * 1000.0).round() / 1000.0,
        is_slosh_coupled: coupled,
        is_robot_flipped: flip,
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
                "{}/../../data/exports/sovereign/cg_shift_resonance.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    println!("====================================================================");
    println!("  G^G: SLOSH LIP  (k_p > m g h, 200 Hz)");
    println!("  n={n}  flip {FLIP} rad");
    println!("====================================================================\n");
    let t0 = Instant::now();
    let mut rng = Rng::new(0x2804_00CC);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("slosh_frac", DataType::Float64, false),
        Field::new("max_pitch_rad", DataType::Float64, false),
        Field::new("is_slosh_coupled", DataType::Boolean, false),
        Field::new("is_robot_flipped", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.slosh_frac).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.max_pitch_rad).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_slosh_coupled).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_robot_flipped).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G slosh LIP dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let nf = n as f64;
    let a = rows.iter().filter(|r| r.is_slosh_coupled).count();
    let b = rows.iter().filter(|r| r.is_robot_flipped).count();
    println!(
        "  slosh {a} ({:.1}%)  flip {b} ({:.1}%)",
        100.0 * a as f64 / nf,
        100.0 * b as f64 / nf
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
