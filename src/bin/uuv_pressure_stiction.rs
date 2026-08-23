//! Abyssal fin-shaft stiction. Hydrostatic crush of the seal vs PI windup.
//! Dual-regime: never breaks away in 2 s (held) vs snap past 25° (tumble).
//! 1000 Hz clock — the windup is the halt analog.

use genesis_core::output;
use genesis_core::physics::marine::MarinePhysics;
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

const HZ: f64 = 1000.0;
const DT: f64 = 1.0 / HZ;
const T_SIM: f64 = 2.0;
const TUMBLE_DEG: f64 = 25.0;
const CLEAN_DEG: f64 = 5.0;

#[derive(Debug, Serialize)]
struct StictionRun {
    id: u32,
    short_id: String,
    depth_m: f64,
    uuv_speed_ms: f64,
    hydrostatic_psi: f64,
    stiction_nm: f64,
    kp: f64,
    ki: f64,
    tau_max_nm: f64,
    target_fin_deg: f64,
    final_fin_deg: f64,
    max_fin_deg: f64,
    t_breakaway_s: f64,
    is_stiction_held: bool,
    is_tumble: bool,
    is_clean: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> StictionRun {
    let short_id = output::short_id(rng);
    let depth = rng.range(200.0, 4000.0);
    let speed = rng.range(4.0, 12.0);
    let mu = rng.range(0.008, 0.022);
    let kp = rng.range(1.5, 10.0);
    let ki = rng.range(0.4, 12.0);
    let tau_max = rng.range(8.0, 55.0);
    let target = rng.range(10.0, 18.0);
    let moi = rng.range(0.3, 0.8);

    let physics = MarinePhysics::default();
    let p_pa = physics.hydrostatic_pressure(depth);
    let p_psi = p_pa / 6894.76;
    let stiction = 2.0 + p_psi * mu;
    let kinetic = stiction * 0.3;

    let mut angle = 0.0;
    let mut omega = 0.0;
    let mut integral = 0.0;
    let mut broken = false;
    let mut t_break = -1.0;
    let mut max_abs = 0.0;
    let mut tumble = false;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(depth);
    proof.feed_f64(speed);
    proof.feed_f64(stiction);
    proof.feed_f64(kp);
    proof.feed_f64(ki);
    proof.feed_f64(tau_max);
    proof.feed_f64(target);

    let steps = (T_SIM * HZ) as usize;
    for tick in 0..steps {
        let err = target - angle;
        integral += err * DT;
        let cmd = (kp * err + ki * integral).clamp(-tau_max, tau_max);
        if !broken && cmd.abs() > stiction {
            broken = true;
            t_break = tick as f64 * DT;
        }
        let acc = if broken {
            let visc = 0.8 * omega;
            (cmd - kinetic * cmd.signum() - visc) / moi
        } else {
            omega = 0.0;
            0.0
        };
        omega += acc * DT;
        angle += omega * DT;
        let abs_a = angle.abs();
        if abs_a > max_abs {
            max_abs = abs_a;
        }
        if tick % 200 == 0 {
            proof.feed_f64(angle);
        }
        if abs_a > TUMBLE_DEG {
            tumble = true;
            break;
        }
    }

    let held = !broken;
    let clean = broken && !tumble && (angle - target).abs() < CLEAN_DEG;
    proof.feed_f64(angle);
    proof.feed_str(if tumble {
        "TUMBLE"
    } else if held {
        "STICTION_HELD"
    } else if clean {
        "FIN_CLEAN"
    } else {
        "OVERSHOOT_RECOVERED"
    });

    StictionRun {
        id,
        short_id,
        depth_m: depth,
        uuv_speed_ms: speed,
        hydrostatic_psi: p_psi,
        stiction_nm: stiction,
        kp,
        ki,
        tau_max_nm: tau_max,
        target_fin_deg: target,
        final_fin_deg: angle,
        max_fin_deg: max_abs,
        t_breakaway_s: t_break,
        is_stiction_held: held,
        is_tumble: tumble,
        is_clean: clean,
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
                "{}/../../grokd/data/uuv_pressure_stiction.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: UUV PRESSURE STICTION  (seal crush vs PI windup)");
    println!("  n={n}  1000 Hz  tumble gate {TUMBLE_DEG} deg");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x5555_565f_5354_4943);
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
        Field::new("uuv_speed_ms", DataType::Float64, false),
        Field::new("hydrostatic_psi", DataType::Float64, false),
        Field::new("stiction_nm", DataType::Float64, false),
        Field::new("kp", DataType::Float64, false),
        Field::new("ki", DataType::Float64, false),
        Field::new("tau_max_nm", DataType::Float64, false),
        Field::new("target_fin_deg", DataType::Float64, false),
        Field::new("final_fin_deg", DataType::Float64, false),
        Field::new("max_fin_deg", DataType::Float64, false),
        Field::new("t_breakaway_s", DataType::Float64, false),
        Field::new("is_stiction_held", DataType::Boolean, false),
        Field::new("is_tumble", DataType::Boolean, false),
        Field::new("is_clean", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(rows.iter().map(|r| Some(format!("stc_{}", r.short_id))).collect::<StringArray>()),
            Arc::new(rows.iter().map(|r| Some(r.depth_m)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.uuv_speed_ms)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.hydrostatic_psi)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.stiction_nm)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.kp)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.ki)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.tau_max_nm)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.target_fin_deg)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.final_fin_deg)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.max_fin_deg)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.t_breakaway_s)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.is_stiction_held)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.is_tumble)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.is_clean)).collect::<BooleanArray>()),
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
                "G^G UUV pressure stiction dual-regime v1.0".to_string(),
            ),
        ]))
        .build();
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let held = rows.iter().filter(|r| r.is_stiction_held).count();
    let tumble = rows.iter().filter(|r| r.is_tumble).count();
    let clean = rows.iter().filter(|r| r.is_clean).count();
    println!(
        "  held {held} ({:.1}%)  tumble {tumble} ({:.1}%)  clean {clean} ({:.1}%)",
        100.0 * held as f64 / n_f,
        100.0 * tumble as f64 / n_f,
        100.0 * clean as f64 / n_f
    );
    println!("  seal {run_proof}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
