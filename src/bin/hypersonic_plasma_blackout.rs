//! Plasma sheath vs GPS L1. f_p ≈ 8.98√n_e. Blackout when f_p > 1.575 GHz.
//! During blackout the filter does **not** see true velocity. Cov grows as σ_v² t².
//! Clock: 20 Hz. Mix Mach 2.5–6.5 so some never blackout.
//! Gates: is_blackout vs is_target_missed (>39 m). Organ: aero plasma cutoff. Not LBM.

use genesis_core::output;
use genesis_core::physics::aero::{
    gps_l1_blackout, plasma_frequency_hz, sheath_electron_density_m3, tas_from_mach, GPS_L1_HZ,
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
const DT: f64 = 0.05;
const DECK_HALF_M: f64 = 39.0;
#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    mach: f64,
    max_plasma_freq_ghz: f64,
    miss_distance_m: f64,
    is_blackout: bool,
    is_target_missed: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let mach = rng.range(2.5, 6.5);
    let z0 = rng.range(12_000.0, 32_000.0);
    let dive = rng.range(30.0, 55.0).to_radians();

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(mach);
    proof.feed_f64(z0);
    proof.feed_f64(dive);

    let v = tas_from_mach(mach);
    let vz = -v * dive.sin();
    let vx = v * dive.cos();
    let mut z = z0;
    let mut x = 0.0;
    let t_impact = z0 / vz.abs();
    let mut tgt = t_impact * vx; // intercept geometry
    let v_tgt = rng.range(8.0, 16.0);
    let mut x_est = tgt;
    let mut t_black = 0.0;
    let mut saw_blackout = false;
    let mut fin_lock = false;
    let mut peak_fp: f64 = 0.0;
    let mut miss: f64 = 0.0;

    let steps = ((t_impact / DT).ceil() as usize + 4).min(2000);
    for k in 0..steps {
        z += vz * DT;
        tgt += v_tgt * DT;
        let n_e = sheath_electron_density_m3(mach, z.max(0.0));
        let fp = plasma_frequency_hz(n_e);
        peak_fp = peak_fp.max(fp);
        let black = z > 0.0 && gps_l1_blackout(n_e);
        if black {
            saw_blackout = true;
            t_black += DT;
            if t_black > 1.5 {
                fin_lock = true; // safety fallback stays latched
            }
        } else if !fin_lock {
            x_est = tgt;
            t_black = 0.0;
        }
        let tti = (z / vz.abs()).max(0.05);
        let vx_cmd = if fin_lock {
            vx
        } else {
            (x_est - x) / tti
        };
        x += vx_cmd * DT;
        if z <= 0.0 {
            miss = (x - tgt).abs();
            break;
        }
        if k % 10 == 0 {
            proof.feed_f64(fp);
        }
    }

    let missed = miss > DECK_HALF_M;
    proof.feed_f64(miss);
    proof.feed_str(if missed {
        "MISS"
    } else if saw_blackout {
        "BLACKOUT_HIT"
    } else {
        "GPS_HIT"
    });

    Run {
        id,
        short_id,
        mach: (mach * 100.0).round() / 100.0,
        max_plasma_freq_ghz: (peak_fp / 1e9 * 100.0).round() / 100.0,
        miss_distance_m: (miss * 10.0).round() / 10.0,
        is_blackout: saw_blackout,
        is_target_missed: missed,
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
                "{}/../../data/exports/sovereign/hypersonic_plasma_blackout.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: PLASMA vs L1  (f_p=8.98√n_e, 20 Hz)");
    println!("  n={n}  L1={:.3} GHz  miss >{DECK_HALF_M} m", GPS_L1_HZ / 1e9);
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0x87A5_0019);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("mach", DataType::Float64, false),
        Field::new("max_plasma_freq_ghz", DataType::Float64, false),
        Field::new("miss_distance_m", DataType::Float64, false),
        Field::new("is_blackout", DataType::Boolean, false),
        Field::new("is_target_missed", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.mach).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.max_plasma_freq_ghz).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.miss_distance_m).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_blackout).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_target_missed).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G plasma L1 dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let n_f = n as f64;
    let b = rows.iter().filter(|r| r.is_blackout).count();
    let m = rows.iter().filter(|r| r.is_target_missed).count();
    println!(
        "  blackout {b} ({:.1}%)  miss {m} ({:.1}%)",
        100.0 * b as f64 / n_f,
        100.0 * m as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
