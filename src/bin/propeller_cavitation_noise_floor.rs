//! Propeller-tip Bernoulli vs hydrophone mask. Dual-regime: cavitation inception
//! (P_min ≤ P_vapor) is not the same column as self-noise saturating a 110 dB floor.

use genesis_core::output;
use genesis_core::physics::marine::{self, MarinePhysics, SEAWATER_VAPOR_PA};
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

const HZ: f64 = 100.0;
const DT: f64 = 1.0 / HZ;
const T_SIM: f64 = 8.0;
const HYDROPHONE_SAT_DB: f64 = 110.0;
const CONTACT_SL_DB: f64 = 95.0;

#[derive(Debug, Serialize)]
struct CavRun {
    id: u32,
    short_id: String,
    depth_m: f64,
    sprint_rpm: f64,
    prop_diameter_m: f64,
    blade_cl: f64,
    hydrostatic_pa: f64,
    tip_speed_ms: f64,
    p_min_pa: f64,
    cavitation_sigma: f64,
    peak_self_noise_db: f64,
    t_deafen_s: f64,
    is_cavitating: bool,
    is_deafened: bool,
    proof_hash: String,
}

fn flow_noise_db(v_tip: f64) -> f64 {
    70.0 + 20.0 * (v_tip.max(1.0) / 10.0).log10()
}

fn cav_noise_db(p_min: f64) -> f64 {
    // Inception sits near 100 dB; only deep exceedance of vapor crosses 110.
    let severity = ((SEAWATER_VAPOR_PA - p_min).max(0.0)) / 2.0e4;
    100.0 + 6.0 * (1.0 + severity).log10()
}

fn run_one(id: u32, rng: &mut Rng) -> CavRun {
    let short_id = output::short_id(rng);
    let depth = rng.range(8.0, 80.0);
    let sprint = rng.range(300.0, 1600.0);
    let diam = rng.range(0.8, 2.0);
    let cl = rng.range(0.35, 0.90);
    let physics = MarinePhysics::default();
    let p_hydro = physics.hydrostatic_pressure(depth);

    let mut rpm = 200.0;
    let mut peak_db = flow_noise_db((rpm / 60.0) * std::f64::consts::PI * diam);
    let mut cavitating = false;
    let mut deafened = false;
    let mut t_deafen = -1.0;
    let mut p_min_end = p_hydro;
    let mut v_tip_end = 0.0;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(depth);
    proof.feed_f64(sprint);
    proof.feed_f64(diam);
    proof.feed_f64(cl);

    let steps = (T_SIM * HZ) as usize;
    for tick in 0..steps {
        rpm += (sprint - rpm) * 0.5 * DT;
        let v_tip = (rpm / 60.0) * std::f64::consts::PI * diam;
        let drop = 0.5 * marine::RHO_SEAWATER * v_tip * v_tip * cl;
        let p_min = p_hydro - drop;
        let cavitating_now = p_min <= SEAWATER_VAPOR_PA;
        if cavitating_now {
            cavitating = true;
        }
        let noise = if cavitating_now {
            flow_noise_db(v_tip).max(cav_noise_db(p_min))
        } else {
            flow_noise_db(v_tip)
        };
        if noise > peak_db {
            peak_db = noise;
        }
        if !deafened && noise > HYDROPHONE_SAT_DB {
            deafened = true;
            t_deafen = tick as f64 * DT;
        }
        p_min_end = p_min;
        v_tip_end = v_tip;
        if tick % 50 == 0 {
            proof.feed_f64(noise);
        }
    }

    let dyn_q = 0.5 * marine::RHO_SEAWATER * v_tip_end * v_tip_end;
    let sigma = if dyn_q > 1.0 {
        (p_hydro - SEAWATER_VAPOR_PA) / dyn_q
    } else {
        f64::INFINITY
    };
    proof.feed_f64(p_min_end);
    proof.feed_str(if cavitating && deafened {
        "CAVITATION_MASK"
    } else if cavitating {
        "CAVITATION_QUIET"
    } else if deafened {
        "FLOW_NOISE_MASK"
    } else {
        "CONTACT_HELD"
    });

    CavRun {
        id,
        short_id,
        depth_m: depth,
        sprint_rpm: sprint,
        prop_diameter_m: diam,
        blade_cl: cl,
        hydrostatic_pa: p_hydro,
        tip_speed_ms: v_tip_end,
        p_min_pa: p_min_end,
        cavitation_sigma: sigma,
        peak_self_noise_db: peak_db,
        t_deafen_s: t_deafen,
        is_cavitating: cavitating,
        is_deafened: deafened,
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
                "{}/../../grokd/data/propeller_cavitation.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: PROPELLER CAVITATION  (Bernoulli tip vs {HYDROPHONE_SAT_DB} dB floor)");
    println!("  n={n}  100 Hz ramp  contact {CONTACT_SL_DB} dB named, not a column");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x4341_565f_4e4f_4953);
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
        Field::new("depth_m", DataType::Float64, false),
        Field::new("sprint_rpm", DataType::Float64, false),
        Field::new("prop_diameter_m", DataType::Float64, false),
        Field::new("blade_cl", DataType::Float64, false),
        Field::new("hydrostatic_pa", DataType::Float64, false),
        Field::new("tip_speed_ms", DataType::Float64, false),
        Field::new("p_min_pa", DataType::Float64, false),
        Field::new("cavitation_sigma", DataType::Float64, false),
        Field::new("peak_self_noise_db", DataType::Float64, false),
        Field::new("t_deafen_s", DataType::Float64, false),
        Field::new("is_cavitating", DataType::Boolean, false),
        Field::new("is_deafened", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(rows.iter().map(|r| Some(format!("cav_{}", r.short_id))).collect::<StringArray>()),
            Arc::new(rows.iter().map(|r| Some(r.depth_m)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.sprint_rpm)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.prop_diameter_m)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.blade_cl)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.hydrostatic_pa)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.tip_speed_ms)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.p_min_pa)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.cavitation_sigma)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.peak_self_noise_db)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.t_deafen_s)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.is_cavitating)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.is_deafened)).collect::<BooleanArray>()),
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
                "G^G propeller cavitation dual-regime v1.0".to_string(),
            ),
        ]))
        .build();
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let cav = rows.iter().filter(|r| r.is_cavitating).count();
    let deaf = rows.iter().filter(|r| r.is_deafened).count();
    let quiet_cav = rows
        .iter()
        .filter(|r| r.is_cavitating && !r.is_deafened)
        .count();
    let flow_mask = rows
        .iter()
        .filter(|r| !r.is_cavitating && r.is_deafened)
        .count();
    println!(
        "  cavitating {cav} ({:.1}%)  deafened {deaf} ({:.1}%)  cav_quiet {quiet_cav} ({:.1}%)  flow_mask {flow_mask} ({:.1}%)",
        100.0 * cav as f64 / n_f,
        100.0 * deaf as f64 / n_f,
        100.0 * quiet_cav as f64 / n_f,
        100.0 * flow_mask as f64 / n_f
    );
    println!("  seal {run_proof}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
