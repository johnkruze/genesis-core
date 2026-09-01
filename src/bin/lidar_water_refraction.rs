//! Dome droplet. Snell n1 sinθ1 = n2 sinθ2, not θ·0.33. Mix dry vs wet, grazing vs face-on.
//! Clock: constitutive one scan. Gates: warp ≥ 0.28 m vs freeze ≥ 1.15 m.

use genesis_core::output;
use genesis_core::physics::optics::{droplet_refraction_projection_error_m, N_WATER};
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
const WARP_M: f64 = 0.22;
const FREEZE_M: f64 = 1.55;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    incidence_deg: f64,
    max_depth_error_m: f64,
    is_cloud_warped: bool,
    is_vslam_frozen: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let wet = rng.chance(0.58);
    let theta = if wet {
        rng.range(10.0, 48.0).to_radians()
    } else {
        rng.range(3.0, 12.0).to_radians()
    };
    let dist = rng.range(3.5, 11.0);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(theta);
    proof.feed_f64(dist);

    let err = if wet {
        droplet_refraction_projection_error_m(dist, theta, N_WATER)
    } else {
        0.0
    };
    let warp = err >= WARP_M;
    let freeze = err >= FREEZE_M;

    proof.feed_f64(err);
    proof.feed_str(if freeze {
        "FROZEN"
    } else if warp {
        "WARP"
    } else {
        "CLEAR"
    });

    Run {
        id,
        short_id,
        incidence_deg: (theta.to_degrees() * 10.0).round() / 10.0,
        max_depth_error_m: (err * 1000.0).round() / 1000.0,
        is_cloud_warped: warp,
        is_vslam_frozen: freeze,
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
                "{}/../../data/exports/sovereign/lidar_water_refraction.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    println!("====================================================================");
    println!("  G^G: SNELL DOME  (n1 sinθ1 = n2 sinθ2)");
    println!("  n={n}");
    println!("====================================================================\n");
    let t0 = Instant::now();
    let mut rng = Rng::new(0x8110_77D0);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("incidence_deg", DataType::Float64, false),
        Field::new("max_depth_error_m", DataType::Float64, false),
        Field::new("is_cloud_warped", DataType::Boolean, false),
        Field::new("is_vslam_frozen", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.incidence_deg).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.max_depth_error_m).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_cloud_warped).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_vslam_frozen).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G Snell dome dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let nf = n as f64;
    let a = rows.iter().filter(|r| r.is_cloud_warped).count();
    let b = rows.iter().filter(|r| r.is_vslam_frozen).count();
    println!(
        "  warp {a} ({:.1}%)  freeze {b} ({:.1}%)",
        100.0 * a as f64 / nf,
        100.0 * b as f64 / nf
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
