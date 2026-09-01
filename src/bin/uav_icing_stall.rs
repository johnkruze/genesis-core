//! Fixed-wing icing. Velocity state. Iced CL_max and α_stall from the organ.
//! Elevator holds altitude; ice drops the stall gate onto the trajectory.
//! Clock: 50 Hz, 25 s. Gates: ice_contaminated vs stalled (α > α_stall_iced).

use genesis_core::output;
use genesis_core::physics::aero::{
    cl_linear, dynamic_pressure_pa, iced_cl_max, iced_stall_alpha_rad, G,
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
const HORIZON_S: f64 = 25.0;
const MASS: f64 = 36.0;
const S: f64 = 1.2;
const CL0: f64 = 0.30;
const A_CL: f64 = 4.5;
const CL_MAX_CLEAN: f64 = 1.35;
const A_STALL_CLEAN: f64 = 0.26;
const RHO: f64 = 1.05;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    ice_factor: f64,
    max_aoa_rad: f64,
    stall_alpha_rad: f64,
    is_ice_contaminated: bool,
    is_uas_stalled: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let ice = rng.range(0.05, 0.95);
    let v = rng.range(22.0, 32.0);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(ice);
    proof.feed_f64(v);

    let cl_max = iced_cl_max(CL_MAX_CLEAN, ice);
    let a_stall = iced_stall_alpha_rad(A_STALL_CLEAN, ice);
    let q = dynamic_pressure_pa(RHO, v);
    let contaminated = ice >= 0.35;

    let mut alpha = 0.06;
    let mut vz = 0.0;
    let mut z = 0.0;
    let mut peak_a = alpha;
    let mut stalled = false;
    let steps = (HORIZON_S / DT) as usize;
    for k in 0..steps {
        let cl = cl_linear(alpha, CL0, A_CL, cl_max);
        let lift = q * S * cl;
        let az = (lift - MASS * G) / MASS;
        vz += az * DT;
        z += vz * DT;
        // Elevator: hold z≈0 by increasing α when sinking. Ice forces more α.
        let alpha_cmd = (0.08 - 0.15 * z - 0.08 * vz + 0.12 * ice).clamp(0.02, 0.32);
        alpha += (alpha_cmd - alpha) * 0.15;
        peak_a = peak_a.max(alpha);
        if alpha >= a_stall {
            stalled = true;
            break;
        }
        if k % 20 == 0 {
            proof.feed_f64(alpha);
        }
    }

    proof.feed_f64(peak_a);
    proof.feed_str(if stalled {
        "STALLED"
    } else if contaminated {
        "ICED"
    } else {
        "CLEAN"
    });

    Run {
        id,
        short_id,
        ice_factor: (ice * 100.0).round() / 100.0,
        max_aoa_rad: (peak_a * 1000.0).round() / 1000.0,
        stall_alpha_rad: (a_stall * 1000.0).round() / 1000.0,
        // Exclusive three-way aligned with proof: ICED / STALLED / CLEAN.
        is_ice_contaminated: contaminated && !stalled,
        is_uas_stalled: stalled,
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
                "{}/../../data/exports/sovereign/uav_icing_stall.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: ICED STALL  (velocity state, α_stall(ice), 50 Hz)");
    println!("  n={n}");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0x1C19_00A1);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("ice_factor", DataType::Float64, false),
        Field::new("max_aoa_rad", DataType::Float64, false),
        Field::new("stall_alpha_rad", DataType::Float64, false),
        Field::new("is_ice_contaminated", DataType::Boolean, false),
        Field::new("is_uas_stalled", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.ice_factor).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.max_aoa_rad).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.stall_alpha_rad).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_ice_contaminated).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_uas_stalled).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G iced stall dual-regime v3.1");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let n_f = n as f64;
    let c = rows.iter().filter(|r| r.is_ice_contaminated).count();
    let s = rows.iter().filter(|r| r.is_uas_stalled).count();
    let clean = n - c - s;
    println!(
        "  iced-held {c} ({:.1}%)  stalled {s} ({:.1}%)  clean {clean} ({:.1}%)",
        100.0 * c as f64 / n_f,
        100.0 * s as f64 / n_f,
        100.0 * clean as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
