//! USV diesel-electric heat balance. Pump degradation vs thermal derate.
//! Gates: 115 °C interlock, 450 °C block melt. Runaway term on oil above 220 °C.
//! Thermal clock is 1 Hz — 1000 Hz was costume on a 4-hour plant.

use genesis_core::output;
use genesis_core::physics::marine;
use genesis_core::proof::{self, ProofChain};
use genesis_core::rng::Rng;
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{BooleanArray, Float64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;

const DT: f64 = 1.0;
const T_SHUT_C: f64 = 115.0;
const T_MELT_C: f64 = 450.0;
const T_SEA_C: f64 = marine::SEA_TEMP_C;
const T_RUNAWAY_C: f64 = 200.0;
const V0_KMH: f64 = 25.0;
const K_GEN: f64 = 2.0;
const K_PUMP: f64 = 1.50;
const K_HULL: f64 = 0.007;

#[derive(Debug, Serialize)]
struct DieselRun {
    id: u32,
    short_id: String,
    cooling_efficiency: f64,
    derate: f64,
    cruise_kmh: f64,
    waypoint_km: f64,
    k_runaway: f64,
    peak_temp_c: f64,
    final_temp_c: f64,
    t_overtemp_s: f64,
    t_melt_s: f64,
    made_waypoint: bool,
    is_overtemp: bool,
    is_melted: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> DieselRun {
    let short_id = output::short_id(rng);
    let eff = rng.range(0.15, 1.05);
    let derate = rng.range(0.0, 1.0);
    let cruise = rng.range(12.0, 34.0);
    let mut range_km = rng.range(20.0, 110.0);
    let k_run = rng.range(0.035, 0.11);

    let mut t = 85.0;
    let mut peak = t;
    let mut t_over = -1.0;
    let mut t_melt = -1.0;
    let mut over = false;
    let mut melted = false;
    let mut made = false;
    let mut elapsed = 0.0;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(eff);
    proof.feed_f64(derate);
    proof.feed_f64(cruise);
    proof.feed_f64(range_km);
    proof.feed_f64(k_run);

    let max_s = 2.0 * 3600.0;
    while elapsed < max_s {
        let v = if t > T_SHUT_C { cruise * derate } else { cruise };
        let q_gen = K_GEN * (v / V0_KMH).powi(2);
        let pump = K_PUMP * eff * ((t - T_SEA_C) / 67.0).clamp(0.0, 1.15);
        let hull = K_HULL * (t - T_SEA_C);
        let mut dtc = q_gen - pump - hull;
        if t > T_RUNAWAY_C {
            dtc += k_run * (t - T_RUNAWAY_C);
        }
        t += dtc * DT;
        range_km -= (v / 3600.0) * DT;
        elapsed += DT;
        if t > peak {
            peak = t;
        }
        if !over && t > T_SHUT_C {
            over = true;
            t_over = elapsed;
        }
        if t > T_MELT_C {
            melted = true;
            t_melt = elapsed;
            break;
        }
        if range_km <= 0.0 {
            made = true;
            break;
        }
        if elapsed as u64 % 300 == 0 {
            proof.feed_f64(t);
        }
    }

    proof.feed_f64(peak);
    proof.feed_str(if melted {
        "BLOCK_MELT"
    } else if over {
        "OVERTEMP_HELD"
    } else {
        "THERMAL_OK"
    });

    DieselRun {
        id,
        short_id,
        cooling_efficiency: eff,
        derate,
        cruise_kmh: cruise,
        waypoint_km: range_km.max(0.0),
        k_runaway: k_run,
        peak_temp_c: peak,
        final_temp_c: t,
        t_overtemp_s: t_over,
        t_melt_s: t_melt,
        made_waypoint: made,
        is_overtemp: over,
        is_melted: melted,
        proof_hash: proof.seal(),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(2500);
    let out = args
        .iter()
        .position(|a| a == "--parquet")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| {
            format!(
                "{}/../../grokd/data/usv_diesel_runaway.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: USV DIESEL THERMAL  (pump vs derate, oil runaway > {T_RUNAWAY_C} °C)");
    println!("  n={n}  dt={DT}s  shut {T_SHUT_C} °C  melt {T_MELT_C} °C");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x4449_4553_454c_5448);
    let t0 = Instant::now();
    let mut rows = Vec::with_capacity(n as usize);
    for i in 0..n {
        rows.push(run_one(i, &mut rng));
    }
    let proofs: Vec<_> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let run_proof = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::Utf8, false),
        Field::new("cooling_efficiency", DataType::Float64, false),
        Field::new("derate", DataType::Float64, false),
        Field::new("cruise_kmh", DataType::Float64, false),
        Field::new("waypoint_km", DataType::Float64, false),
        Field::new("k_runaway", DataType::Float64, false),
        Field::new("peak_temp_c", DataType::Float64, false),
        Field::new("final_temp_c", DataType::Float64, false),
        Field::new("t_overtemp_s", DataType::Float64, false),
        Field::new("t_melt_s", DataType::Float64, false),
        Field::new("made_waypoint", DataType::Boolean, false),
        Field::new("is_overtemp", DataType::Boolean, false),
        Field::new("is_melted", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(rows.iter().map(|r| Some(format!("dsl_{}", r.short_id))).collect::<StringArray>()),
            Arc::new(rows.iter().map(|r| Some(r.cooling_efficiency)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.derate)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.cruise_kmh)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.waypoint_km)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.k_runaway)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.peak_temp_c)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.final_temp_c)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.t_overtemp_s)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.t_melt_s)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.made_waypoint)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.is_overtemp)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.is_melted)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.proof_hash.clone())).collect::<StringArray>()),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.clone()),
            parquet::file::metadata::KeyValue::new(
                "generator".to_string(),
                "G^G USV diesel thermal dual-regime v1.0".to_string(),
            ),
        ]))
        .build();
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let over = rows.iter().filter(|r| r.is_overtemp).count();
    let melt = rows.iter().filter(|r| r.is_melted).count();
    let held = rows.iter().filter(|r| r.is_overtemp && !r.is_melted).count();
    let ok = rows.iter().filter(|r| !r.is_overtemp).count();
    println!(
        "  overtemp {over} ({:.1}%)  melted {melt} ({:.1}%)  overtemp_held {held} ({:.1}%)  ok {ok} ({:.1}%)",
        100.0 * over as f64 / n_f,
        100.0 * melt as f64 / n_f,
        100.0 * held as f64 / n_f,
        100.0 * ok as f64 / n_f
    );
    println!("  seal {run_proof}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
