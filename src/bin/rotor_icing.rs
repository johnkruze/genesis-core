//! Rotor ice accretion ṁ = LWC · V · A · E. De-ice once at t_delay (not RNG/tick).
//! T/W from iced_thrust_to_weight. Clock: 10 Hz (ice is slow), 40 s cloud.
//! Gates: iced (mass≥25 g) vs T/W < 1.

use genesis_core::output;
use genesis_core::physics::aero::{ice_accretion_kg_s, iced_thrust_to_weight, G};
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
const DT: f64 = 0.10;
const HORIZON_S: f64 = 40.0;
const MASS: f64 = 18.0;
const T0: f64 = MASS * G * 1.35; // hover margin
const AREA: f64 = 0.08;
const V: f64 = 35.0;
const ICE_REF_KG: f64 = 0.09;
const ICE_WARN_KG: f64 = 0.018;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    lwc_g_m3: f64,
    ice_kg: f64,
    thrust_to_weight: f64,
    is_iced: bool,
    is_tw_below_one: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let lwc_g = rng.range(0.08, 1.40);
    let e_coll = rng.range(0.25, 0.70);
    let t_deice = rng.range(8.0, 36.0);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(lwc_g);
    proof.feed_f64(e_coll);
    proof.feed_f64(t_deice);

    let lwc = lwc_g * 1e-3;
    let mut ice = 0.0;
    let mut deiced = false;
    let steps = (HORIZON_S / DT) as usize;
    for k in 0..steps {
        let t = k as f64 * DT;
        ice += ice_accretion_kg_s(lwc, V, AREA, e_coll) * DT;
        if !deiced && t >= t_deice {
            ice *= 0.18; // one thermal cycle, not a per-tick rng
            deiced = true;
        }
        if k % 20 == 0 {
            proof.feed_f64(ice);
        }
    }

    let tw = iced_thrust_to_weight(T0, MASS * G, ice, ICE_REF_KG);
    let iced = ice >= ICE_WARN_KG;
    let tw_fail = tw < 1.0;
    proof.feed_f64(ice);
    proof.feed_f64(tw);
    proof.feed_str(if tw_fail {
        "TW_FAIL"
    } else if iced {
        "ICED"
    } else {
        "CLEAR"
    });

    Run {
        id,
        short_id,
        lwc_g_m3: (lwc_g * 100.0).round() / 100.0,
        ice_kg: (ice * 1e4).round() / 1e4,
        thrust_to_weight: (tw * 1000.0).round() / 1000.0,
        // Exclusive three-way aligned with proof: ICED / TW_FAIL / CLEAR.
        is_iced: iced && !tw_fail,
        is_tw_below_one: tw_fail,
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
                "{}/../../data/exports/sovereign/rotor_icing.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: ROTOR ICE  (ṁ=LWC·V·A·E, 10 Hz)");
    println!("  n={n}  iced ≥{ICE_WARN_KG} kg  T/W<1");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0x2070_1C1E);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("lwc_g_m3", DataType::Float64, false),
        Field::new("ice_kg", DataType::Float64, false),
        Field::new("thrust_to_weight", DataType::Float64, false),
        Field::new("is_iced", DataType::Boolean, false),
        Field::new("is_tw_below_one", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.lwc_g_m3).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.ice_kg).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.thrust_to_weight).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_iced).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_tw_below_one).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G rotor ice accretion dual-regime v3.1");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let n_f = n as f64;
    let iced = rows.iter().filter(|r| r.is_iced).count();
    let tw = rows.iter().filter(|r| r.is_tw_below_one).count();
    let clear = n - iced - tw;
    println!(
        "  iced-held {iced} ({:.1}%)  tw<1 {tw} ({:.1}%)  clear {clear} ({:.1}%)",
        100.0 * iced as f64 / n_f,
        100.0 * tw as f64 / n_f,
        100.0 * clear as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
