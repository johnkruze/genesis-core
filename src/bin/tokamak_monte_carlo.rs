//! Tokamak MHD quench. dt=1 µs, 10 kHz AI field, 10 ms horizon.
//! Exclusive three-way: radial wall · z-shear divertor · confined.
//! Remainder of “NONE” is confined — named, re-sealed. No FFI.

use genesis_core::output;
use genesis_core::physics::tokamak::{DIVERTOR_Z_LIMIT, Tokamak};
use genesis_core::proof;
use genesis_core::rng::Rng;
use rayon::prelude::*;
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{BooleanArray, Float64Array, StringArray, UInt32Array, UInt64Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::ArrowWriter;

const DEFAULT_N: usize = 2500;
const DT_US: f64 = 1.0;
const MAX_TIME_US: u64 = 50_000; // 50 ms — √(GAMMA_Z_SQ) ≈ 32 ms growth
const F_AI_US: u64 = 100; // 10 kHz control

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    gpu_bit_depth: String,
    radial_noise_mt: f64,
    z_asymmetry_noise_mt: f64,
    r0: f64,
    particle_density: f64,
    t_to_breach_us: u64,
    is_radial: bool,
    is_z_shear: bool,
    is_confined: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let gpu_bit_depth = match rng.index(4) {
        0 => "INT4",
        1 => "INT8",
        2 => "FP16",
        _ => "FP32",
    };
    let base_noise_mt = match gpu_bit_depth {
        "INT4" => 5.0,
        "INT8" => 0.5,
        "FP16" => 0.05,
        _ => 0.001,
    };
    let r0 = rng.range(1.0, 1.55);
    let particle_density = rng.range(1e19, 1e20);
    // Mix radial vs z-dominant vs quiet so all three exclusive classes fire.
    // Vertical growth time of the module is ~32 ms; seed a divertor-side offset.
    let mix = rng.index(5);
    let (radial_noise_mt, z_asymmetry_noise_mt, z0) = match mix {
        0 | 1 => (
            base_noise_mt * rng.range(0.8, 3.0),
            base_noise_mt * rng.range(0.0, 0.15),
            rng.range(-0.02, 0.02),
        ),
        2 | 3 => (
            0.0,
            base_noise_mt * rng.range(1.0, 4.0),
            rng.range(0.08, 0.18) * if rng.chance(0.5) { 1.0 } else { -1.0 },
        ),
        _ => (
            base_noise_mt * rng.range(0.0, 0.12),
            base_noise_mt * rng.range(0.0, 0.12),
            rng.range(-0.01, 0.01),
        ),
    };

    let mut tokamak = Tokamak::new();
    tokamak.plasma_radius = r0;
    tokamak.temperature = 100e6;
    tokamak.particle_density = particle_density;
    tokamak.z_displacement = z0;
    tokamak.b_field = tokamak.exact_equilibrium_b_field();
    tokamak.proof.seed(&id.to_le_bytes());
    tokamak.proof.feed_str(gpu_bit_depth);
    tokamak.proof.feed_f64(radial_noise_mt);
    tokamak.proof.feed_f64(z_asymmetry_noise_mt);
    tokamak.proof.feed_f64(z0);

    let target_b = tokamak.b_field;
    let rad_noise_t = radial_noise_mt / 1000.0;
    let z_noise_t = z_asymmetry_noise_mt / 1000.0;
    let mut last_ai_update: u64 = 0;

    while !tokamak.quenched && tokamak.time_us < MAX_TIME_US {
        if tokamak.time_us.saturating_sub(last_ai_update) >= F_AI_US {
            tokamak.apply_agentic_ai_field(target_b, rad_noise_t, z_noise_t);
            last_ai_update = tokamak.time_us;
        }
        tokamak.step(DT_US);
    }

    let z_abs = tokamak.z_displacement.abs();
    let is_z_shear = tokamak.quenched && z_abs >= DIVERTOR_Z_LIMIT;
    let is_radial = tokamak.quenched && !is_z_shear;
    let is_confined = !tokamak.quenched;
    let class = if is_z_shear {
        "Z_SHEAR"
    } else if is_radial {
        "RADIAL"
    } else {
        "CONFINED"
    };
    tokamak.proof.feed_str(class);

    let t_to_breach_us = if tokamak.quenched { tokamak.time_us } else { 0 };
    let proof_hash = tokamak.get_sealed_hash();

    Run {
        id,
        short_id,
        gpu_bit_depth: gpu_bit_depth.to_string(),
        radial_noise_mt,
        z_asymmetry_noise_mt,
        r0,
        particle_density,
        t_to_breach_us,
        is_radial,
        is_z_shear,
        is_confined,
        proof_hash,
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
                "{}/../../data/exports/sovereign/tokamak_shear.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: TOKAMAK CONFINEMENT  (dt=1 µs · 10 kHz AI · 50 ms)");
    println!("  n={n}  exclusive RADIAL / Z_SHEAR / CONFINED");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let rows: Vec<Run> = (0..n as u32)
        .into_par_iter()
        .map(|i| {
            let mut rng = Rng::new(0x70CA_0001u64.wrapping_add(i as u64).wrapping_mul(0x9E3779B97F4A7C15));
            run_one(i, &mut rng)
        })
        .collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let unique: std::collections::HashSet<&str> = proofs.iter().map(|s| s.as_str()).collect();
    assert_eq!(unique.len(), n, "proof_hash must be unique");
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("gpu_bit_depth", DataType::Utf8, false),
        Field::new("radial_noise_mt", DataType::Float64, false),
        Field::new("z_asymmetry_noise_mt", DataType::Float64, false),
        Field::new("r0", DataType::Float64, false),
        Field::new("particle_density", DataType::Float64, false),
        Field::new("t_to_breach_us", DataType::UInt64, false),
        Field::new("is_radial", DataType::Boolean, false),
        Field::new("is_z_shear", DataType::Boolean, false),
        Field::new("is_confined", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.gpu_bit_depth.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.radial_noise_mt).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.z_asymmetry_noise_mt).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.r0).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.particle_density).collect::<Vec<_>>())),
            Arc::new(UInt64Array::from(rows.iter().map(|r| r.t_to_breach_us).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_radial).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_z_shear).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_confined).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G tokamak confinement dual-regime v1.1");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let nf = n as f64;
    let rad = rows.iter().filter(|r| r.is_radial).count();
    let zs = rows.iter().filter(|r| r.is_z_shear).count();
    let conf = rows.iter().filter(|r| r.is_confined).count();
    let both = rows
        .iter()
        .filter(|r| (r.is_radial as u8) + (r.is_z_shear as u8) + (r.is_confined as u8) != 1)
        .count();
    println!(
        "  exclusive: radial {rad} ({:.1}%)  z-shear {zs} ({:.1}%)  confined {conf} ({:.1}%)  sum {}  overlap {both}",
        100.0 * rad as f64 / nf,
        100.0 * zs as f64 / nf,
        100.0 * conf as f64 / nf,
        rad + zs + conf
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
