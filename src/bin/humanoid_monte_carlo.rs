//! G^G Humanoid Monte Carlo — THE TERRAN TRUTH
//! Bipedal stability, slip, and substrate yielding.
//! Organ: terran (SoilProfile, RobotContact).
//! Sovereign Receipt n=2500 Dual-Regime Parquet.

use arrow::array::{BooleanArray, Float64Array, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use genesis_core::output;
use genesis_core::physics::terran::{Locomotion, RobotContact, SoilProfile, SoilType};
use genesis_core::proof::{self, ProofChain};
use genesis_core::rng::Rng;
use parquet::arrow::arrow_writer::ArrowWriter;
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;

const DEFAULT_N: usize = 2500;

#[derive(Debug, Clone, Copy, PartialEq)]
enum FailureMode {
    FootSlip,
    BalanceLoss,
    SoilYield,
    ActuatorFault,
}

#[derive(Debug, Serialize)]
struct Run {
    trajectory_id: u32,
    short_id: String,
    soil_type: String,
    robot_mass_kg: f64,
    surface_pressure_pa: f64,
    yield_stress_pa: f64,
    max_compaction: f64,
    is_foot_slip: bool,
    is_balance_loss: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);

    let soil_type = match rng.index(4) {
        0 => SoilType::Sand,
        1 => SoilType::Loam,
        2 => SoilType::Clay,
        _ => SoilType::Andisol,
    };
    let moisture = rng.range(0.05, 0.40);
    let glomalin = rng.range(20.0, 150.0);
    let mut soil = SoilProfile {
        soil_type,
        moisture,
        glomalin_mg_g: glomalin,
        compaction: rng.range(0.0, 0.2),
        depth_layers: 10,
    };

    let locomotion = Locomotion::Legged;
    let robot_mass = rng.range(60.0, 150.0);
    let footprint = rng.range(0.02, 0.05);

    let robot = RobotContact {
        mass_kg: robot_mass,
        footprint_m2: footprint,
        locomotion,
    };

    let mut failure_chance = 0.10;
    if matches!(soil.soil_type, SoilType::Sand) {
        failure_chance += 0.15;
    }
    if matches!(soil.soil_type, SoilType::Clay) && moisture > 0.3 {
        failure_chance += 0.15;
    }

    let failure = if rng.chance(failure_chance) {
        Some(FailureMode::FootSlip)
    } else if rng.chance(0.12) {
        Some(FailureMode::BalanceLoss)
    } else if rng.chance(0.08) {
        Some(FailureMode::SoilYield)
    } else if rng.chance(0.03) {
        Some(FailureMode::ActuatorFault)
    } else {
        None
    };

    let surface_pressure = robot.surface_pressure() * 2.0;
    let yield_stress = soil.effective_yield_stress();

    let dt = 0.01;
    let max_steps = 2000;
    let mut step = 0_usize;
    let mut max_compaction = 0.0_f64;
    let mut foot_slip_occurred = false;
    let mut balance_loss_occurred = false;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(robot_mass);
    proof.feed_str(soil_type.as_str());

    while step < max_steps {
        let (compact_inc, _) = soil.evaluate_contact(&robot);
        soil.compaction = (soil.compaction + compact_inc * dt).min(1.0);
        max_compaction = max_compaction.max(compact_inc);

        let zmp_error = rng.range(-0.02, 0.02);
        let slip_ratio = if matches!(soil.soil_type, SoilType::Sand) {
            rng.range(0.05, 0.20)
        } else {
            rng.range(0.0, 0.08)
        };

        if matches!(failure, Some(FailureMode::FootSlip)) && slip_ratio > 0.10 {
            foot_slip_occurred = true;
        }

        if matches!(failure, Some(FailureMode::BalanceLoss)) && zmp_error.abs() > 0.015 {
            balance_loss_occurred = true;
        }

        if surface_pressure > yield_stress && matches!(failure, Some(FailureMode::SoilYield)) {
            balance_loss_occurred = true;
        }

        if step % 100 == 0 {
            proof.feed_f64(soil.compaction);
            proof.feed_f64(slip_ratio);
        }

        step += 1;
    }

    if matches!(failure, Some(FailureMode::FootSlip)) {
        foot_slip_occurred = true;
    }
    if matches!(failure, Some(FailureMode::BalanceLoss)) || matches!(failure, Some(FailureMode::SoilYield)) {
        balance_loss_occurred = true;
    }

    proof.feed_str(if balance_loss_occurred {
        "BALANCE_LOSS"
    } else if foot_slip_occurred {
        "FOOT_SLIP"
    } else {
        "NOMINAL"
    });

    Run {
        trajectory_id: id,
        short_id,
        soil_type: soil_type.as_str().to_string(),
        robot_mass_kg: (robot_mass * 100.0).round() / 100.0,
        surface_pressure_pa: (surface_pressure * 10.0).round() / 10.0,
        yield_stress_pa: (yield_stress * 10.0).round() / 10.0,
        max_compaction: (max_compaction * 1000.0).round() / 1000.0,
        is_foot_slip: foot_slip_occurred,
        is_balance_loss: balance_loss_occurred,
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
                "{}/../../data/exports/sovereign/humanoid_monte_carlo.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: HUMANOID MONTE CARLO (terran SoilProfile + RobotContact)");
    println!("  n={n}  out={out}");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0xb19e_d00d_baad_f00d);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("soil_type", DataType::Utf8, false),
        Field::new("robot_mass_kg", DataType::Float64, false),
        Field::new("surface_pressure_pa", DataType::Float64, false),
        Field::new("yield_stress_pa", DataType::Float64, false),
        Field::new("max_compaction", DataType::Float64, false),
        Field::new("is_foot_slip", DataType::Boolean, false),
        Field::new("is_balance_loss", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.trajectory_id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.soil_type.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.robot_mass_kg).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.surface_pressure_pa).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.yield_stress_pa).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.max_compaction).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_foot_slip).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_balance_loss).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");

    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G humanoid terran monte carlo dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let slip = rows.iter().filter(|r| r.is_foot_slip).count();
    let balance = rows.iter().filter(|r| r.is_balance_loss).count();
    println!(
        "  foot_slip {slip} ({:.1}%)  balance_loss {balance} ({:.1}%)",
        100.0 * slip as f64 / n_f,
        100.0 * balance as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
