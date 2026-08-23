//! Micro-assembly release: capillary stiction · ESD · piezo shake · safe retract.

use genesis_core::output;
use genesis_core::physics::dexterous::{
    evaluate_micro_release_dynamics, C_MicroReleaseAuditor,
};
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

#[derive(Debug, Serialize)]
struct MicroRun {
    id: u32,
    short_id: String,
    part_mass_ug: f64,
    final_pull_off_un: f64,
    final_jaw_separation_um: f64,
    final_charge_v: f64,
    release_stiction_active: bool,
    electrostatic_charge_violation: bool,
    piezo_shake_trigger: bool,
    safe_to_retract: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> MicroRun {
    let short_id = output::short_id(rng);
    let mass = rng.range(2.0, 400.0) as f32;
    let humidity = rng.range(0.15, 0.95) as f32;
    let tribo = rng.range(0.8, 6.5) as f32;
    let dt = 0.001f32;
    let mut jaw = 0.0f32;
    let mut last_jaw = 0.0f32;
    let mut pull = 0.0f32;
    let mut charge = rng.range(5.0, 40.0) as f32;
    let mut stiction = false;
    let mut esd = false;
    let mut piezo = false;
    let mut safe = false;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(mass as f64);
    proof.feed_f64(humidity as f64);

    for step in 0..80 {
        last_jaw = jaw;
        jaw += rng.range(0.3, 0.7) as f32; // µm / ms opening
        // Capillary pull-off decays as jaws open, worse in humidity.
        let bridge = (18.0 * humidity) / (1.0 + 0.08 * jaw);
        pull = bridge * (mass / 80.0).sqrt();
        charge += tribo * (jaw - last_jaw).abs() * rng.range(0.8, 1.6) as f32;

        let auditor = C_MicroReleaseAuditor {
            part_mass_micrograms: mass,
            pull_off_force_un: pull,
            jaw_separation_um: jaw,
            dynamic_electrostatic_charge_v: charge,
            last_jaw_separation_um: last_jaw,
        };
        let res = evaluate_micro_release_dynamics(&auditor, dt);
        stiction = res.release_stiction_active;
        esd = res.electrostatic_charge_violation;
        piezo = res.piezo_shake_trigger;
        safe = res.safe_to_retract;
        if step % 20 == 0 {
            proof.feed_f64(pull as f64);
            proof.feed_f64(charge as f64);
        }
    }

    proof.feed_str(if safe {
        "SAFE_RETRACT"
    } else if esd {
        "ESD_VIOLATION"
    } else {
        "STICTION_BOUND"
    });

    MicroRun {
        id,
        short_id,
        part_mass_ug: mass as f64,
        final_pull_off_un: pull as f64,
        final_jaw_separation_um: jaw as f64,
        final_charge_v: charge as f64,
        release_stiction_active: stiction,
        electrostatic_charge_violation: esd,
        piezo_shake_trigger: piezo,
        safe_to_retract: safe,
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
        .unwrap_or_else(|| "../../grokd/data/micro_assembly_release.parquet".to_string());

    println!("====================================================================");
    println!("  G^G: MICRO-ASSEMBLY RELEASE  (stiction / ESD / piezo)");
    println!("  n={n}  gates: jaw>10µm & pull>5µN · charge>150 V");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x4d49_4352_4f5f524c);
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
        Field::new("part_mass_ug", DataType::Float64, false),
        Field::new("final_pull_off_un", DataType::Float64, false),
        Field::new("final_jaw_separation_um", DataType::Float64, false),
        Field::new("final_charge_v", DataType::Float64, false),
        Field::new("release_stiction_active", DataType::Boolean, false),
        Field::new("electrostatic_charge_violation", DataType::Boolean, false),
        Field::new("piezo_shake_trigger", DataType::Boolean, false),
        Field::new("safe_to_retract", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(rows.iter().map(|r| Some(format!("micro_{}", r.short_id))).collect::<StringArray>()),
            Arc::new(rows.iter().map(|r| Some(r.part_mass_ug)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.final_pull_off_un)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.final_jaw_separation_um)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.final_charge_v)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.release_stiction_active)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.electrostatic_charge_violation)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.piezo_shake_trigger)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.safe_to_retract)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.proof_hash.clone())).collect::<StringArray>()),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.clone()),
            parquet::file::metadata::KeyValue::new("generator".to_string(), "G^G micro-assembly release v1.0".to_string()),
        ]))
        .build();
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let safe = rows.iter().filter(|r| r.safe_to_retract).count();
    let st = rows.iter().filter(|r| r.release_stiction_active).count();
    let esd = rows.iter().filter(|r| r.electrostatic_charge_violation).count();
    println!("  safe_retract {safe} ({:.1}%)  stiction {st}  esd {esd}", 100.0 * safe as f64 / n as f64);
    println!("  seal {run_proof}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
