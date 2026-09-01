//! Arctic turret. Walther visc, then organ viscous drag. Not 14·exp((20−T)·0.08).
//! Mix arctic vs temperate. Clock: 50 Hz, 5 s. Gates: lag ≥ 2° vs sling ≥ 8°.

use genesis_core::output;
use genesis_core::physics::hydraulics::hydraulic_actuator_viscous_drag_n;
use genesis_core::physics::thermal::walther_lubricant_viscosity_cst;
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
const HORIZON_S: f64 = 5.0;
const WALTHER_A: f64 = 9.0;
const WALTHER_B: f64 = 3.55;
const LAG_DEG: f64 = 4.5;
const SLING_DEG: f64 = 8.0;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    ambient_temp_c: f64,
    max_tracking_error_deg: f64,
    is_lagging: bool,
    is_radar_lock_lost: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let arctic = rng.chance(0.42);
    let t_c = if arctic {
        rng.range(-42.0, -18.0)
    } else {
        rng.range(0.0, 28.0)
    };

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(t_c);

    let nu = walther_lubricant_viscosity_cst(WALTHER_A, WALTHER_B, t_c);
    let mut th = 0.0;
    let mut w = 0.0;
    let mut peak: f64 = 0.0;
    let tgt_rate = 0.12; // rad/s
    let steps = (HORIZON_S / DT) as usize;
    for k in 0..steps {
        let tgt = tgt_rate * k as f64 * DT;
        let err = tgt - th;
        let tau_cmd = (280.0 * err - 35.0 * w).clamp(-400.0, 400.0);
        let v_piston = (w * 0.30).abs();
        let drag = hydraulic_actuator_viscous_drag_n(nu, 870.0, v_piston.max(0.005), 0.008, 2e-5);
        let tau_drag = drag * 0.30;
        let coulomb = 8.0 + 0.006 * nu;
        let stuck = w.abs() < 1e-4 && tau_cmd.abs() < coulomb;
        let tau_net = if stuck {
            0.0
        } else {
            tau_cmd - tau_drag.copysign(w) - coulomb.copysign(tau_cmd)
        };
        w += (tau_net / 40.0) * DT;
        th += w * DT;
        peak = peak.max((tgt - th).abs());
        if k % 10 == 0 {
            proof.feed_f64(th);
        }
    }
    let lag = peak.to_degrees() >= LAG_DEG;
    let lost = peak.to_degrees() >= SLING_DEG;

    proof.feed_f64(peak);
    proof.feed_str(if lost {
        "SLING"
    } else if lag {
        "LAG"
    } else {
        "LOCK"
    });

    Run {
        id,
        short_id,
        ambient_temp_c: (t_c * 10.0).round() / 10.0,
        max_tracking_error_deg: (peak.to_degrees() * 10.0).round() / 10.0,
        is_lagging: lag,
        is_radar_lock_lost: lost,
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
                "{}/../../data/exports/sovereign/hydraulic_shear_stiction.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    println!("====================================================================");
    println!("  G^G: WALTHER STICK  (87257 named, 50 Hz)");
    println!("  n={n}");
    println!("====================================================================\n");
    let t0 = Instant::now();
    let mut rng = Rng::new(0x5E11_00AA);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("ambient_temp_c", DataType::Float64, false),
        Field::new("max_tracking_error_deg", DataType::Float64, false),
        Field::new("is_lagging", DataType::Boolean, false),
        Field::new("is_radar_lock_lost", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.ambient_temp_c).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.max_tracking_error_deg).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_lagging).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_radar_lock_lost).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G Walther stiction dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let nf = n as f64;
    let a = rows.iter().filter(|r| r.is_lagging).count();
    let b = rows.iter().filter(|r| r.is_radar_lock_lost).count();
    println!(
        "  lag {a} ({:.1}%)  sling {b} ({:.1}%)",
        100.0 * a as f64 / nf,
        100.0 * b as f64 / nf
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
