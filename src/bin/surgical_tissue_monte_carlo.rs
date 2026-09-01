//! Tissue-limited clamp: overstress · viscoelastic rupture · cable slip · held.
//! Dual-regime exclusive: rupture is terminal (tissue destroyed). Cable only if
//! tissue lives. Overstress only if the jaw is still on living tissue.
//! Policy numbers (1.2 / 2.5 / 40 N) are named gates, not constitutive tissue models.

use genesis_core::output;
use genesis_core::physics::dexterous::{
    evaluate_surgical_grasp_dynamics, C_SurgicalTissueAuditor,
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
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;

#[derive(Debug, Serialize)]
struct SurgicalRun {
    id: u32,
    short_id: String,
    tissue_type_id: u32,
    tissue_name: String,
    tissue_limit_n: f64,
    max_tearing_force_n: f64,
    policy_command_n: f64,
    final_force_n: f64,
    clamped_force_n: f64,
    final_displacement_m: f64,
    tissue_overstress: bool,
    viscoelastic_rupture: bool,
    cable_slip_fault: bool,
    sample_held: bool,
    proof_hash: String,
}

fn tissue_name(id: u32) -> &'static str {
    match id {
        0 => "liver_spleen",
        1 => "bowel_vessel",
        2 => "bone_tendon",
        _ => "safe_default",
    }
}

fn tissue_limit(id: u32) -> f32 {
    match id {
        0 => 1.2,
        1 => 2.5,
        2 => 40.0,
        _ => 1.0,
    }
}

fn run_one(id: u32, rng: &mut Rng) -> SurgicalRun {
    let short_id = output::short_id(rng);
    let tissue_type_id = rng.index(3) as u32;
    let limit = tissue_limit(tissue_type_id);
    let max_tearing = limit * rng.range(0.8, 1.35) as f32;
    let policy_command = limit * rng.range(0.35, 2.15) as f32;
    let cable_failed = rng.range(0.0, 1.0) < 0.12;
    let yield_force = limit * rng.range(0.80, 1.45) as f32;
    let disp_rate = rng.range(0.04, 0.18) as f32; // m/s of jaw close

    let dt = 0.001f32;
    let mut disp = 0.0f32;
    let mut force = rng.range(0.05, 0.35) as f32 * limit;
    let mut last_disp = disp;
    let mut last_force = force;
    let mut over = false;
    let mut rupture = false;
    let mut cable = false;
    let mut clamped = 0.0f32;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(limit as f64);
    proof.feed_f64(policy_command as f64);

    for step in 0..100 {
        last_disp = disp;
        last_force = force;
        disp += disp_rate * dt;

        if cable_failed {
            force *= 0.92;
            if disp > 0.012 {
                force = rng.range(0.0, 0.04) as f32;
            }
        } else {
            force += (policy_command - force) * 0.10;
            if force > yield_force && disp > 0.0015 {
                force *= 0.42; // stiffness drop → rupture detector
            }
        }

        let auditor = C_SurgicalTissueAuditor {
            tissue_type_id,
            max_tearing_force_n: max_tearing,
            measured_displacement_m: disp,
            measured_force_n: force,
            relaxation_tau: 0.05,
            last_displacement_m: last_disp,
            last_force_n: last_force,
            accumulated_energy_j: (force * disp).max(0.0),
        };
        let res = evaluate_surgical_grasp_dynamics(&auditor, dt);
        clamped = res.clamped_force;
        if res.tissue_overstress_detected {
            over = true;
        }
        if res.viscoelastic_rupture_detected {
            rupture = true;
        }
        if res.cable_slip_fault {
            cable = true;
        }
        if step % 25 == 0 {
            proof.feed_f64(force as f64);
            proof.feed_f64(disp as f64);
        }
    }

    // Rupture is the terminal event. Cable / overstress only seal if tissue lives.
    let rupture_flag = rupture;
    let cable_flag = cable && !rupture;
    let over_flag = over && !rupture && !cable;
    let held = !over_flag && !rupture_flag && !cable_flag;
    proof.feed_str(if rupture_flag {
        "VISCOELASTIC_RUPTURE"
    } else if cable_flag {
        "CABLE_SLIP"
    } else if over_flag {
        "TISSUE_OVERSTRESS"
    } else {
        "SAMPLE_HELD"
    });

    SurgicalRun {
        id,
        short_id,
        tissue_type_id,
        tissue_name: tissue_name(tissue_type_id).to_string(),
        tissue_limit_n: limit as f64,
        max_tearing_force_n: max_tearing as f64,
        policy_command_n: policy_command as f64,
        final_force_n: force as f64,
        clamped_force_n: clamped as f64,
        final_displacement_m: disp as f64,
        tissue_overstress: over_flag,
        viscoelastic_rupture: rupture_flag,
        cable_slip_fault: cable_flag,
        sample_held: held,
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
        .unwrap_or_else(|| "../../grokd/data/surgical_tissue_clamp.parquet".to_string());

    println!("====================================================================");
    println!("  G^G: SURGICAL TISSUE CLAMP  (do not destroy the sample)");
    println!("  n={n}  gates: liver 1.2 N · bowel 2.5 N · bone 40 N");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x5355_5247_5f544953);
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
        Field::new("tissue_type_id", DataType::UInt32, false),
        Field::new("tissue_name", DataType::Utf8, false),
        Field::new("tissue_limit_n", DataType::Float64, false),
        Field::new("max_tearing_force_n", DataType::Float64, false),
        Field::new("policy_command_n", DataType::Float64, false),
        Field::new("final_force_n", DataType::Float64, false),
        Field::new("clamped_force_n", DataType::Float64, false),
        Field::new("final_displacement_m", DataType::Float64, false),
        Field::new("tissue_overstress", DataType::Boolean, false),
        Field::new("viscoelastic_rupture", DataType::Boolean, false),
        Field::new("cable_slip_fault", DataType::Boolean, false),
        Field::new("sample_held", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(rows.iter().map(|r| Some(format!("surg_{}", r.short_id))).collect::<StringArray>()),
            Arc::new(rows.iter().map(|r| Some(r.tissue_type_id)).collect::<UInt32Array>()),
            Arc::new(rows.iter().map(|r| Some(r.tissue_name.clone())).collect::<StringArray>()),
            Arc::new(rows.iter().map(|r| Some(r.tissue_limit_n)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.max_tearing_force_n)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.policy_command_n)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.final_force_n)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.clamped_force_n)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.final_displacement_m)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.tissue_overstress)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.viscoelastic_rupture)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.cable_slip_fault)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.sample_held)).collect::<BooleanArray>()),
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
                "G^G surgical tissue clamp dual-regime v1.1".to_string(),
            ),
        ]))
        .build();
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let held = rows.iter().filter(|r| r.sample_held).count();
    let over = rows.iter().filter(|r| r.tissue_overstress).count();
    let rup = rows.iter().filter(|r| r.viscoelastic_rupture).count();
    let cab = rows.iter().filter(|r| r.cable_slip_fault).count();
    assert_eq!(held + over + rup + cab, n as usize);
    let n_f = n as f64;
    println!(
        "  held {held} ({:.1}%)  overstress {over} ({:.1}%)  rupture {rup} ({:.1}%)  cable {cab} ({:.1}%)  exclusive",
        100.0 * held as f64 / n_f,
        100.0 * over as f64 / n_f,
        100.0 * rup as f64 / n_f,
        100.0 * cab as f64 / n_f
    );
    println!("  seal {run_proof}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
