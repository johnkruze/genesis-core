//! Custom Physical Forge shape: C-C 1.7 g/cc vs Supercarbon ≥2.0 g/cc.
//! Reduced-order reentry: density-dependent recession → CoG migration → controllability.
//! Named gates, not CFD. Survival corridor is the product.

use genesis_core::output;
use genesis_core::physics::materials::{CC_DENSITY_GCC, SUPERCARBON_DENSITY_GCC};
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

const COG_SPIN_M: f64 = 0.95;
const NOSE_MIN_MM: f64 = 5.0;

#[derive(Debug, Serialize)]
struct CarbonRow {
    id: u32,
    short_id: String,
    density_gcc: f64,
    stack: String,
    is_cc_baseline: bool,
    is_supercarbon: bool,
    t_controllable_s: f64,
    mass_loss_kg: f64,
    nose_thickness_mm: f64,
    cog_aft_m: f64,
    survived_terminal: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> CarbonRow {
    let short_id = output::short_id(rng);
    let density = rng.range(1.55, 2.35);
    let is_cc = density < CC_DENSITY_GCC;
    let is_sc = density >= SUPERCARBON_DENSITY_GCC;
    let stack = if is_cc {
        "C-C"
    } else if is_sc {
        "supercarbon"
    } else {
        "transitional"
    };
    // Porosity proxy: lower density ablates faster (named, reduced-order).
    let porosity = (2.20 / density).powi(2);
    let heat = rng.range(0.85, 1.25); // trajectory-to-trajectory heating
    let mass0 = rng.range(1100.0, 1400.0);
    let nose0 = rng.range(18.0, 28.0); // mm
    // Calibrated so ~1.7 g/cc loses CoG near 66 s, ~2.1 near 97 s.
    let rec_mm_s = 0.12 * porosity * heat; // nose lasts; CoG is the spin clock
    let mass_rate = mass0 * 0.00475 * porosity * heat;

    let dt = 0.10;
    let mut t = 0.0;
    let mut nose = nose0;
    let mut mass_loss = 0.0;
    let mut cog = 0.50;
    let mut survived = false;
    let mut t_ctrl = 0.0;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(density);
    proof.feed_f64(heat);

    while t < 140.0 {
        nose -= rec_mm_s * dt;
        mass_loss += mass_rate * dt;
        cog = 0.50 + (mass_loss / mass0) * 0.80;
        t += dt;
        t_ctrl = t;
        if t % 10.0 < dt {
            proof.feed_f64(cog);
            proof.feed_f64(nose);
        }
        if nose <= NOSE_MIN_MM || cog >= COG_SPIN_M {
            break;
        }
        if t >= 90.0 && nose > NOSE_MIN_MM && cog < COG_SPIN_M {
            survived = true;
        }
    }
    if cog >= COG_SPIN_M || nose <= NOSE_MIN_MM {
        survived = false;
    }

    proof.feed_str(stack);
    proof.feed_str(if survived { "TERMINAL_SURVIVED" } else { "DEPARTURE_SPIN" });

    CarbonRow {
        id,
        short_id,
        density_gcc: density,
        stack: stack.to_string(),
        is_cc_baseline: is_cc,
        is_supercarbon: is_sc,
        t_controllable_s: t_ctrl,
        mass_loss_kg: mass_loss,
        nose_thickness_mm: nose.max(0.0),
        cog_aft_m: cog,
        survived_terminal: survived,
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
        .unwrap_or_else(|| "../../grokd/data/forge_denser_carbon.parquet".to_string());

    println!("====================================================================");
    println!("  G^G CUSTOM PHYSICAL FORGE  ·  C-C vs Supercarbon");
    println!("  n={n}  gates: CoG {COG_SPIN_M} m  ·  nose {NOSE_MIN_MM} mm  ·  2.0 g/cc corridor");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x504f_5045_5f43432d);
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
        Field::new("density_gcc", DataType::Float64, false),
        Field::new("stack", DataType::Utf8, false),
        Field::new("is_cc_baseline", DataType::Boolean, false),
        Field::new("is_supercarbon", DataType::Boolean, false),
        Field::new("t_controllable_s", DataType::Float64, false),
        Field::new("mass_loss_kg", DataType::Float64, false),
        Field::new("nose_thickness_mm", DataType::Float64, false),
        Field::new("cog_aft_m", DataType::Float64, false),
        Field::new("survived_terminal", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(rows.iter().map(|r| Some(format!("pope_{}", r.short_id))).collect::<StringArray>()),
            Arc::new(rows.iter().map(|r| Some(r.density_gcc)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.stack.clone())).collect::<StringArray>()),
            Arc::new(rows.iter().map(|r| Some(r.is_cc_baseline)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.is_supercarbon)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.t_controllable_s)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.mass_loss_kg)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.nose_thickness_mm)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.cog_aft_m)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.survived_terminal)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.proof_hash.clone())).collect::<StringArray>()),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.clone()),
            parquet::file::metadata::KeyValue::new("generator".to_string(), "G^G C-C vs denser carbon stack v1.0".to_string()),
        ]))
        .build();
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let cc: Vec<_> = rows.iter().filter(|r| r.is_cc_baseline).collect();
    let sc: Vec<_> = rows.iter().filter(|r| r.is_supercarbon).collect();
    let cc_s = cc.iter().filter(|r| r.survived_terminal).count();
    let sc_s = sc.iter().filter(|r| r.survived_terminal).count();
    let cc_t: f64 = cc.iter().map(|r| r.t_controllable_s).sum::<f64>() / cc.len().max(1) as f64;
    let sc_t: f64 = sc.iter().map(|r| r.t_controllable_s).sum::<f64>() / sc.len().max(1) as f64;
    println!("  C-C n={} survive {:.1}%  t_ctrl={:.1} s", cc.len(), 100.0 * cc_s as f64 / cc.len().max(1) as f64, cc_t);
    println!("  Supercarbon n={} survive {:.1}%  t_ctrl={:.1} s  Δt={:.1} s", sc.len(), 100.0 * sc_s as f64 / sc.len().max(1) as f64, sc_t, sc_t - cc_t);
    println!("  seal {run_proof}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
