//! Forward-looking sonar under a thermocline. Mackenzie sound speed + Snell circular ray.
//! Ping at 10 Hz (not 1000 Hz — that clock belongs to slip). Gate: beam still misses
//! the target vertical span when range first drops inside the turning radius.

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

const PING_HZ: f64 = 10.0;
const DT: f64 = 1.0 / PING_HZ;

#[derive(Debug, Serialize)]
struct ThermoRun {
    id: u32,
    short_id: String,
    temp_surface_c: f64,
    temp_deep_c: f64,
    salinity_psu: f64,
    thermocline_thickness_m: f64,
    uuv_depth_m: f64,
    uuv_speed_ms: f64,
    reef_range_m: f64,
    turning_gate_m: f64,
    sound_speed_gradient: f64,
    ray_radius_m: f64,
    z_drop_at_gate_m: f64,
    ray_folded: bool,
    beam_misses_target: bool,
    inertial_collision: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> ThermoRun {
    let short_id = output::short_id(rng);
    let t_surf = rng.range(12.0, 28.0);
    let t_deep = rng.range(2.0, t_surf - 0.5);
    let s = rng.range(32.0, 37.0);
    let thickness = rng.range(10.0, 80.0);
    let depth = rng.range(8.0, 40.0);
    let speed = rng.range(4.0, 12.0);
    let reef0 = rng.range(80.0, 350.0);
    let gate = rng.range(80.0, 150.0);

    let c_upper = marine::mackenzie_sound_speed(t_surf, s, depth);
    let c_lower = marine::mackenzie_sound_speed(t_deep, s, depth + thickness);
    let dc_dz = (c_lower - c_upper) / thickness;
    let radius = marine::acoustic_ray_radius(c_upper, dc_dz);

    let reef_top = depth - 5.0;
    let reef_bot = depth + 2.0;

    let (dz0, fold0) = marine::acoustic_ray_drop(radius, reef0);
    let miss_initial = {
        let beam0 = depth + dz0;
        !(beam0 > reef_top && beam0 < reef_bot)
    };

    let mut x = 0.0;
    let mut detected = !miss_initial;
    let mut folded_any = fold0;
    let mut z_at_gate = dz0;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(t_surf);
    proof.feed_f64(t_deep);
    proof.feed_f64(s);
    proof.feed_f64(thickness);
    proof.feed_f64(depth);
    proof.feed_f64(reef0);
    proof.feed_f64(gate);
    proof.feed_f64(dz0);

    // Straight-line cruise. Re-ping as look-ahead shrinks — drop falls as x falls.
    let max_steps = ((reef0 / speed.max(0.1)) / DT).ceil() as usize + 2;
    for step in 0..max_steps {
        let look = (reef0 - x).max(0.0);
        let (dz, folded) = marine::acoustic_ray_drop(radius, look);
        folded_any |= folded;
        let beam_z = depth + dz;
        let hits = beam_z > reef_top && beam_z < reef_bot;
        if hits {
            detected = true;
        }
        if look <= gate {
            z_at_gate = dz;
            break;
        }
        if step % 10 == 0 {
            proof.feed_f64(dz);
        }
        x += speed * DT;
    }

    let collision = !detected;
    proof.feed_f64(z_at_gate);
    proof.feed_str(if collision {
        "ACOUSTIC_SHADOW"
    } else if miss_initial {
        "LATE_DETECT"
    } else {
        "TARGET_IN_BEAM"
    });

    ThermoRun {
        id,
        short_id,
        temp_surface_c: t_surf,
        temp_deep_c: t_deep,
        salinity_psu: s,
        thermocline_thickness_m: thickness,
        uuv_depth_m: depth,
        uuv_speed_ms: speed,
        reef_range_m: reef0,
        turning_gate_m: gate,
        sound_speed_gradient: dc_dz,
        ray_radius_m: radius,
        z_drop_at_gate_m: z_at_gate,
        ray_folded: folded_any,
        beam_misses_target: miss_initial,
        inertial_collision: collision,
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
                "{}/../../grokd/data/uuv_thermocline_refraction.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: UUV THERMOCLINE  (Mackenzie + Snell ray vs turning gate)");
    println!("  n={n}  ping {PING_HZ} Hz");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x5555_565f_5448_524d);
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
        Field::new("temp_surface_c", DataType::Float64, false),
        Field::new("temp_deep_c", DataType::Float64, false),
        Field::new("salinity_psu", DataType::Float64, false),
        Field::new("thermocline_thickness_m", DataType::Float64, false),
        Field::new("uuv_depth_m", DataType::Float64, false),
        Field::new("uuv_speed_ms", DataType::Float64, false),
        Field::new("reef_range_m", DataType::Float64, false),
        Field::new("turning_gate_m", DataType::Float64, false),
        Field::new("sound_speed_gradient", DataType::Float64, false),
        Field::new("ray_radius_m", DataType::Float64, false),
        Field::new("z_drop_at_gate_m", DataType::Float64, false),
        Field::new("ray_folded", DataType::Boolean, false),
        Field::new("beam_misses_target", DataType::Boolean, false),
        Field::new("inertial_collision", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(rows.iter().map(|r| Some(format!("thm_{}", r.short_id))).collect::<StringArray>()),
            Arc::new(rows.iter().map(|r| Some(r.temp_surface_c)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.temp_deep_c)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.salinity_psu)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.thermocline_thickness_m)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.uuv_depth_m)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.uuv_speed_ms)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.reef_range_m)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.turning_gate_m)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.sound_speed_gradient)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.ray_radius_m)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.z_drop_at_gate_m)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.ray_folded)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.beam_misses_target)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.inertial_collision)).collect::<BooleanArray>()),
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
                "G^G UUV thermocline refraction dual-regime v1.0".to_string(),
            ),
        ]))
        .build();
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let miss = rows.iter().filter(|r| r.beam_misses_target).count();
    let crash = rows.iter().filter(|r| r.inertial_collision).count();
    let late = rows
        .iter()
        .filter(|r| r.beam_misses_target && !r.inertial_collision)
        .count();
    let fold = rows.iter().filter(|r| r.ray_folded).count();
    println!(
        "  miss_initial {miss} ({:.1}%)  collision {crash} ({:.1}%)  late_detect {late} ({:.1}%)  folded {fold} ({:.1}%)",
        100.0 * miss as f64 / n_f,
        100.0 * crash as f64 / n_f,
        100.0 * late as f64 / n_f,
        100.0 * fold as f64 / n_f
    );
    println!("  seal {run_proof}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
