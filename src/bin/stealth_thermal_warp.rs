//! Supersonic skin recovery temperature. T_aw = T_∞ (1 + r (γ−1)/2 M²), r = 0.89.
//! One exact exponential step of 600 s to T_aw. Tropopause T_∞ = 216.65 K (11–18 km).
//! Mach 1.2–3.2 so bloom (120 °C) and RAM structural (180 °C) both bind.
//! Organ: adiabatic_wall_temperature_k, LumpedThermalNode.

use genesis_core::output;
use genesis_core::physics::thermal::{adiabatic_wall_temperature_k, LumpedThermalNode};
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
const DASH_S: f64 = 600.0;
const T_INF_K: f64 = 216.65;
const T_BLOOM_C: f64 = 120.0;
const T_RAM_C: f64 = 180.0;
const R_RECOVERY: f64 = 0.89;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    flight_mach: f64,
    t_aw_c: f64,
    final_skin_temp_c: f64,
    is_thermal_bloom: bool,
    is_structural_limit_exceeded: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let mach = rng.range(1.2, 3.2);
    let alt_km = rng.range(11.0, 18.0); // tropopause; T_∞ held

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(mach);
    proof.feed_f64(alt_km);

    let t_aw_k = adiabatic_wall_temperature_k(T_INF_K, mach, R_RECOVERY);
    let t_aw_c = t_aw_k - 273.15;
    let amb_c = T_INF_K - 273.15;
    let mut node = LumpedThermalNode::new(amb_c, 800.0, 0.004);
    // Driving potential is recovery temperature: T_eq = T_aw (Q̇ = 0, ambient = T_aw).
    let t_skin = node.step(0.0, t_aw_c, DASH_S);
    let bloom = t_skin >= T_BLOOM_C;
    let ram = t_skin >= T_RAM_C;

    proof.feed_f64(t_aw_c);
    proof.feed_f64(t_skin);
    proof.feed_str(if ram {
        "STRUCTURAL_LIMIT"
    } else if bloom {
        "BLOOM"
    } else {
        "NOMINAL"
    });

    Run {
        id,
        short_id,
        flight_mach: (mach * 100.0).round() / 100.0,
        t_aw_c: (t_aw_c * 10.0).round() / 10.0,
        final_skin_temp_c: (t_skin * 10.0).round() / 10.0,
        is_thermal_bloom: bloom,
        is_structural_limit_exceeded: ram,
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
                "{}/../../data/exports/sovereign/stealth_thermal_warp.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: AEROTHERMAL T_aw  (r={R_RECOVERY}, 600 s exact lag)");
    println!("  n={n}  bloom {T_BLOOM_C} °C  RAM {T_RAM_C} °C  M=1.2–3.2");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0x5512_0009);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("flight_mach", DataType::Float64, false),
        Field::new("t_aw_c", DataType::Float64, false),
        Field::new("final_skin_temp_c", DataType::Float64, false),
        Field::new("is_thermal_bloom", DataType::Boolean, false),
        Field::new("is_structural_limit_exceeded", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.flight_mach).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.t_aw_c).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.final_skin_temp_c).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_thermal_bloom).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_structural_limit_exceeded).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G T_aw recovery dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let b = rows.iter().filter(|r| r.is_thermal_bloom).count();
    let ram = rows.iter().filter(|r| r.is_structural_limit_exceeded).count();
    println!(
        "  bloom {b} ({:.1}%)  structural {ram} ({:.1}%)",
        100.0 * b as f64 / n_f,
        100.0 * ram as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
