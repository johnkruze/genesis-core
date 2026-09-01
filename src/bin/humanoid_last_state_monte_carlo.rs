//! G^G Humanoid Last-State 64 B (M7 Meridian) Monte Carlo
//!
//! Packs 64 B LastStateFrame64 (SOMA.md envelope, body_id 30) into a sovereign
//! Parquet receipt and data/exports/sovereign/humanoid_dark_window.soma.bin.
//! Sovereign Receipt n=2500 Dual-Regime Parquet.

use arrow::array::{BooleanArray, Float64Array, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use genesis_core::last_state::{self, LastStateFrame64, BODY_HUMANOID};
use genesis_core::output;
use genesis_core::physics::resonance::{
    pd_ankle_torque_nm, zmp_from_ankle_torque_m, InvertedPendulum,
};
use genesis_core::proof::{self, ProofChain};
use genesis_core::rng::Rng;
use parquet::arrow::arrow_writer::ArrowWriter;
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;

const DEFAULT_N: usize = 2500;

#[derive(Debug, Serialize)]
struct Run {
    trajectory_id: u32,
    short_id: String,
    timestamp_ms: u32,
    com_z_m: f64,
    vel_x_ms: f64,
    pitch_rad: f64,
    zmp_margin_m: f64,
    is_dark_window: bool,
    is_buckle: bool,
    is_reflex_grasp: bool,
    frame_checksum_hex: String,
    proof_hash: String,
    packed_frame: Vec<u8>,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let timestamp_ms = id * 10;
    let mass = rng.range(58.0, 95.0);
    let h = rng.range(0.80, 0.95);
    let theta0 = rng.range(-0.08, 0.08);

    let mut plant = InvertedPendulum::new(theta0, h, mass);
    let kp = plant.mgh_nm_per_rad() * rng.range(1.20, 1.80);
    let kd = 2.0 * (kp * plant.inertia_kg_m2()).sqrt() * rng.range(0.35, 0.80);
    let mut peak_omega: f64 = 0.0;
    let dt = 0.01;
    for _ in 0..80 {
        let tau = pd_ankle_torque_nm(plant.theta_rad, plant.omega_rad_s, kp, kd, 140.0);
        plant.step(tau, dt);
        peak_omega = peak_omega.max(plant.omega_rad_s.abs());
    }

    let com_z = (h * plant.theta_rad.cos()).max(0.40);
    let vel_x = plant.omega_rad_s * h;
    let pitch = plant.theta_rad;
    let fnorm = mass * 9.81;
    let tau = pd_ankle_torque_nm(plant.theta_rad, plant.omega_rad_s, kp, kd, 140.0);
    let zmp_margin = 0.045 - zmp_from_ankle_torque_m(tau.abs(), fnorm).abs();

    // Radio blackout is independent of the plant (Dark Window). Buckle is the plant.
    let is_dark_window = rng.chance(0.32);
    let is_buckle = pitch.abs() > 0.18 || zmp_margin < 0.0;
    let is_reflex_grasp = peak_omega > 0.45;

    let packed = LastStateFrame64::pack_humanoid(
        timestamp_ms,
        [0.0, 0.0, com_z as f32],
        [vel_x as f32, 0.0, 0.0],
        pitch as f32,
        zmp_margin as f32,
        is_dark_window,
        is_buckle,
        is_reflex_grasp,
    )
    .to_bytes();

    let checksum_hex = hex::encode(&packed[48..64]);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(com_z);
    proof.feed_f64(vel_x);
    proof.feed_f64(pitch);
    proof.feed_f64(zmp_margin);
    proof.feed(&packed);
    proof.feed_str(if is_dark_window { "DARK" } else { "RADIO" });
    proof.feed_str(if is_buckle { "BUCKLE" } else { "UPRIGHT" });
    proof.feed_str(if is_reflex_grasp { "REFLEX" } else { "QUIET" });

    Run {
        trajectory_id: id,
        short_id,
        timestamp_ms,
        com_z_m: (com_z * 1000.0).round() / 1000.0,
        vel_x_ms: (vel_x * 1000.0).round() / 1000.0,
        pitch_rad: (pitch * 1000.0).round() / 1000.0,
        zmp_margin_m: (zmp_margin * 1000.0).round() / 1000.0,
        is_dark_window,
        is_buckle,
        is_reflex_grasp,
        frame_checksum_hex: checksum_hex,
        proof_hash: proof.seal(),
        packed_frame: packed.to_vec(),
    }
}

fn write_soma_bin(path: &str, rows: &[Run]) -> std::io::Result<()> {
    let frames: Vec<[u8; 64]> = rows
        .iter()
        .map(|r| {
            let mut b = [0u8; 64];
            b.copy_from_slice(&r.packed_frame);
            b
        })
        .collect();
    let bin = last_state::write_soma_file(BODY_HUMANOID, *b"HUMANOID", &frames);
    std::fs::write(path, bin)
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
                "{}/../../data/exports/sovereign/humanoid_last_state.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    let soma_bin_path = format!(
        "{}/../../data/exports/sovereign/humanoid_dark_window.soma.bin",
        env!("CARGO_MANIFEST_DIR")
    );

    println!("====================================================================");
    println!("  G^G: HUMANOID LAST-STATE 64 B (LastStateFrame64 body_id=30)");
    println!("  n={n}  out={out}");
    println!("  bin={soma_bin_path}");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0x736f_6d61_6875_6d61);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }

    write_soma_bin(&soma_bin_path, &rows).expect("write soma.bin");

    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("timestamp_ms", DataType::UInt32, false),
        Field::new("com_z_m", DataType::Float64, false),
        Field::new("vel_x_ms", DataType::Float64, false),
        Field::new("pitch_rad", DataType::Float64, false),
        Field::new("zmp_margin_m", DataType::Float64, false),
        Field::new("is_dark_window", DataType::Boolean, false),
        Field::new("is_buckle", DataType::Boolean, false),
        Field::new("is_reflex_grasp", DataType::Boolean, false),
        Field::new("frame_checksum_hex", DataType::Utf8, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.trajectory_id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.timestamp_ms).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.com_z_m).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.vel_x_ms).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.pitch_rad).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.zmp_margin_m).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_dark_window).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_buckle).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_reflex_grasp).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.frame_checksum_hex.as_str()).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");

    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G humanoid last_state dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let dark = rows.iter().filter(|r| r.is_dark_window).count();
    let buckle = rows.iter().filter(|r| r.is_buckle).count();
    let reflex = rows.iter().filter(|r| r.is_reflex_grasp).count();
    println!(
        "  dark_window {dark} ({:.1}%)  buckle {buckle} ({:.1}%)  reflex_grasp {reflex} ({:.1}%)",
        100.0 * dark as f64 / n_f,
        100.0 * buckle as f64 / n_f,
        100.0 * reflex as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  soma.bin {soma_bin_path}");
    println!("  {:?}", t0.elapsed());
}
