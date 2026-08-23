//! Hull crush vs power-starved ascent. Buoyancy + hydrostatic from MarinePhysics.
//! Dual-regime: depth > true crush is not the same column as battery hitting reserve.
//! Believed crush can be optimistic — that is the reconstructible mission error.

use genesis_core::output;
use genesis_core::physics::marine::{AuvModel, MarinePhysics, RHO_SEAWATER};
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

const DT: f64 = 0.5;
const T_SIM: f64 = 30.0 * 60.0;

#[derive(Debug, Serialize)]
struct SubRun {
    id: u32,
    short_id: String,
    dive_zone: String,
    mass_kg: f64,
    target_depth_m: f64,
    true_crush_m: f64,
    believed_crush_m: f64,
    battery_wh: f64,
    max_depth_m: f64,
    peak_pressure_mpa: f64,
    battery_used_pct: f64,
    is_crushed: bool,
    is_power_starved: bool,
    proof_hash: String,
}

fn zone_of(depth: f64) -> &'static str {
    if depth < 1000.0 {
        "twilight"
    } else if depth < 4000.0 {
        "midnight"
    } else if depth < 6000.0 {
        "abyssal"
    } else {
        "hadal"
    }
}

fn run_one(id: u32, rng: &mut Rng) -> SubRun {
    let short_id = output::short_id(rng);
    let physics = MarinePhysics::default();
    let mass = rng.range(2000.0, 5000.0);
    let volume = mass / RHO_SEAWATER * 1.01;
    let batt = rng.range(18.0, 160.0);
    let mut auv = AuvModel {
        mass,
        volume,
        drag_area: 0.5,
        cd: 0.8,
        max_thrust: 3000.0,
        n_thrusters: 4,
        battery_wh: batt,
        battery_remaining: batt,
        reserve_fraction: 0.1,
    };

    let true_crush = rng.range(1500.0, 9500.0);
    let believed = true_crush * rng.range(0.82, 1.18);
    let target = believed * rng.range(0.55, 0.97);

    let mut pos = [0.0, 0.0, -10.0];
    let mut vel = [0.0; 3];
    let mut max_depth = 10.0;
    let mut crushed = false;
    let mut starved = false;
    let mut phase = 0u8; // 0 dive, 1 survey, 2 ascent, 3 surface

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(mass);
    proof.feed_f64(true_crush);
    proof.feed_f64(believed);
    proof.feed_f64(target);
    proof.feed_f64(batt);

    let max_steps = (T_SIM / DT) as usize;
    for step in 0..max_steps {
        let depth = -pos[2];
        if depth > max_depth {
            max_depth = depth;
        }
        if depth > true_crush {
            crushed = true;
            break;
        }

        let mut thrust = [0.0; 3];
        match phase {
            0 => {
                if depth < target {
                    thrust[2] = -auv.max_thrust * 0.8;
                } else {
                    phase = 1;
                }
            }
            1 => {
                thrust[0] = auv.max_thrust * 0.4;
                let err = target - depth;
                thrust[2] = (err * 12.0).clamp(-auv.max_thrust, auv.max_thrust);
                if step as f64 * DT > T_SIM * 0.45 {
                    phase = 2;
                }
            }
            _ => {
                thrust[2] = auv.max_thrust;
                if depth <= 1.0 {
                    break;
                }
            }
        }

        let power = auv.thrust_power(thrust[2].abs() + thrust[0].abs());
        if !auv.consume_energy(power + 20.0, DT) {
            starved = true;
            phase = 2;
            thrust[2] = if auv.battery_remaining > 0.0 {
                auv.max_thrust
            } else {
                0.0
            };
        }

        let buoyancy = physics.buoyancy(auv.volume);
        let weight = auv.mass * physics.gravity;
        let force_z = buoyancy - weight + thrust[2];
        vel[2] += (force_z / auv.mass) * DT;
        vel[2] *= 0.95;
        pos[2] += vel[2] * DT;
        vel[0] += (thrust[0] / auv.mass) * DT;
        vel[0] *= 0.8;
        pos[0] += vel[0] * DT;
        if pos[2] > 0.0 {
            pos[2] = 0.0;
            vel[2] = 0.0;
            break;
        }
        if step % 120 == 0 {
            proof.feed_f64(depth);
        }
    }

    let peak_p = physics.hydrostatic_pressure(max_depth) / 1.0e6;
    let used = (1.0 - auv.battery_fraction()) * 100.0;
    proof.feed_f64(max_depth);
    proof.feed_str(if crushed {
        "HULL_CRUSHED"
    } else if starved {
        "POWER_STARVED"
    } else {
        "SURFACED"
    });

    SubRun {
        id,
        short_id,
        dive_zone: zone_of(target).to_string(),
        mass_kg: mass,
        target_depth_m: target,
        true_crush_m: true_crush,
        believed_crush_m: believed,
        battery_wh: batt,
        max_depth_m: max_depth,
        peak_pressure_mpa: peak_p,
        battery_used_pct: used,
        is_crushed: crushed,
        is_power_starved: starved,
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
                "{}/../../grokd/data/submarine_crush.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: SUBMARINE CRUSH  (believed crush vs true hull, battery ascent)");
    println!("  n={n}  dt={DT}s  horizon {T_SIM}s");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x5355_425f_4352_5553);
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
        Field::new("dive_zone", DataType::Utf8, false),
        Field::new("mass_kg", DataType::Float64, false),
        Field::new("target_depth_m", DataType::Float64, false),
        Field::new("true_crush_m", DataType::Float64, false),
        Field::new("believed_crush_m", DataType::Float64, false),
        Field::new("battery_wh", DataType::Float64, false),
        Field::new("max_depth_m", DataType::Float64, false),
        Field::new("peak_pressure_mpa", DataType::Float64, false),
        Field::new("battery_used_pct", DataType::Float64, false),
        Field::new("is_crushed", DataType::Boolean, false),
        Field::new("is_power_starved", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(rows.iter().map(|r| Some(format!("sub_{}", r.short_id))).collect::<StringArray>()),
            Arc::new(rows.iter().map(|r| Some(r.dive_zone.clone())).collect::<StringArray>()),
            Arc::new(rows.iter().map(|r| Some(r.mass_kg)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.target_depth_m)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.true_crush_m)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.believed_crush_m)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.battery_wh)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.max_depth_m)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.peak_pressure_mpa)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.battery_used_pct)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.is_crushed)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.is_power_starved)).collect::<BooleanArray>()),
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
                "G^G submarine crush dual-regime v1.0".to_string(),
            ),
        ]))
        .build();
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let crush = rows.iter().filter(|r| r.is_crushed).count();
    let star = rows.iter().filter(|r| r.is_power_starved).count();
    let both = rows
        .iter()
        .filter(|r| r.is_crushed && r.is_power_starved)
        .count();
    let live = rows
        .iter()
        .filter(|r| !r.is_crushed && !r.is_power_starved)
        .count();
    println!(
        "  crushed {crush} ({:.1}%)  power-starved {star} ({:.1}%)  both {both} ({:.1}%)  live {live} ({:.1}%)",
        100.0 * crush as f64 / n_f,
        100.0 * star as f64 / n_f,
        100.0 * both as f64 / n_f,
        100.0 * live as f64 / n_f
    );
    println!("  seal {run_proof}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
