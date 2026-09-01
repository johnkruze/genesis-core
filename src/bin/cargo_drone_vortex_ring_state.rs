//! Cargo-drone VRS. v_i = √(T/(2ρA)). Mix 0.2 v_i (outside) and 1.0 v_i (inside).
//! Derate is a sampled policy (diesel twin). Clock: 50 Hz, 20 s from 80 m.
//! Gates: in vortex ring vs ground impact. Organ: aero momentum theory. Not LBM.

use genesis_core::output;
use genesis_core::physics::aero::{
    hover_induced_velocity_ms, in_vortex_ring, vrs_descent_ratio, vrs_efficiency, G, RHO_SL,
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
const DT: f64 = 0.02;
const HORIZON_S: f64 = 20.0;
const MASS: f64 = 225.0;
const AREA: f64 = 4.0;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    target_descent_ratio: f64,
    max_descent_ratio: f64,
    vrs_efficiency: f64,
    is_vrs: bool,
    is_ground_impact: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    // Mix: 55% outside the ring, 45% inside.
    let ratio_cmd = if rng.chance(0.55) {
        rng.range(0.12, 0.42)
    } else {
        rng.range(0.70, 1.25)
    };
    let derate = rng.range(0.0, 1.0);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(ratio_cmd);
    proof.feed_f64(derate);

    let hover_t = MASS * G;
    let vi = hover_induced_velocity_ms(hover_t, RHO_SL, AREA);
    let mut v_cmd = ratio_cmd * vi; // positive descent
    let mut z = 80.0;
    let mut vz = 0.0; // positive down
    let mut peak_ratio: f64 = 0.0;
    let mut min_eff: f64 = 1.0;
    let mut saw_vrs = false;
    let mut impact = false;
    let mut integ = 0.0;
    let steps = (HORIZON_S / DT) as usize;
    for k in 0..steps {
        let ratio = vrs_descent_ratio(vz, vi);
        peak_ratio = peak_ratio.max(ratio);
        if in_vortex_ring(ratio) {
            saw_vrs = true;
            // Derate: cut commanded descent once in the ring (sampled policy).
            v_cmd = ratio_cmd * vi * (0.25 + 0.75 * (1.0 - derate));
        }
        let eff = vrs_efficiency(ratio);
        min_eff = min_eff.min(eff);
        let err = v_cmd - vz; // want more descent → less thrust
        integ += err * DT;
        let t_cmd = (hover_t - err * 90.0 - integ * 14.0).clamp(0.15 * hover_t, hover_t * 1.6);
        let t_act = t_cmd * eff;
        let acc = G - t_act / MASS; // positive down
        vz += acc * DT;
        z -= vz * DT;
        if z <= 0.0 {
            impact = vz > 8.0; // crater, not a 3 m/s arrival
            break;
        }
        if k % 25 == 0 {
            proof.feed_f64(ratio);
        }
    }

    proof.feed_f64(peak_ratio);
    proof.feed_str(if impact {
        "IMPACT"
    } else if saw_vrs {
        "VRS_HELD"
    } else {
        "OUTSIDE"
    });

    Run {
        id,
        short_id,
        target_descent_ratio: (ratio_cmd * 100.0).round() / 100.0,
        max_descent_ratio: (peak_ratio * 100.0).round() / 100.0,
        vrs_efficiency: (min_eff * 1000.0).round() / 1000.0,
        // Exclusive three-way aligned with proof: VRS_HELD / IMPACT / OUTSIDE.
        is_vrs: saw_vrs && !impact,
        is_ground_impact: impact,
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
                "{}/../../data/exports/sovereign/cargo_drone_vortex_ring_state.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: VRS  (v_i=√(T/2ρA), mix 0.2 v_i / 1.0 v_i, 50 Hz)");
    println!("  n={n}  ring ratio>0.5  impact z=0");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0x5E77_1100);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("target_descent_ratio", DataType::Float64, false),
        Field::new("max_descent_ratio", DataType::Float64, false),
        Field::new("vrs_efficiency", DataType::Float64, false),
        Field::new("is_vrs", DataType::Boolean, false),
        Field::new("is_ground_impact", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.target_descent_ratio).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.max_descent_ratio).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.vrs_efficiency).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_vrs).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_ground_impact).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G VRS momentum dual-regime v3.1");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let n_f = n as f64;
    let v = rows.iter().filter(|r| r.is_vrs).count();
    let i = rows.iter().filter(|r| r.is_ground_impact).count();
    let outside = n - v - i;
    println!(
        "  vrs-held {v} ({:.1}%)  crater {i} ({:.1}%)  outside {outside} ({:.1}%)",
        100.0 * v as f64 / n_f,
        100.0 * i as f64 / n_f,
        100.0 * outside as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
