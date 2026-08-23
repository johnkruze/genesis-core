//! Carrier-island von Kármán wake vs probe/drogue oscillator.
//! Dual-regime: in-band (f_wake near f_n) vs shear > 12 kN snap.
//! Actuator is first-order lag, not a hardcoded negative gain.

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

const HZ: f64 = 200.0;
const DT: f64 = 1.0 / HZ;
const T_SIM: f64 = 8.0;
const SHEAR_GATE_N: f64 = 12_000.0;

#[derive(Debug, Serialize)]
struct WakeRun {
    id: u32,
    short_id: String,
    mass_kg: f64,
    stiffness_npm: f64,
    zeta: f64,
    f_n_hz: f64,
    f_wake_hz: f64,
    freq_ratio: f64,
    tau_s: f64,
    wind_kts: f64,
    peak_shear_n: f64,
    is_in_band: bool,
    is_probe_snapped: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> WakeRun {
    let short_id = output::short_id(rng);
    let m = rng.range(180.0, 700.0);
    let k = rng.range(8.0e4, 6.0e5);
    let zeta = rng.range(0.02, 0.14);
    let f_n = (k / m).sqrt() / (2.0 * std::f64::consts::PI);
    let f_wake = rng.range(1.2, 7.5);
    let tau = rng.range(0.025, 0.35);
    let wind = rng.range(18.0, 48.0);
    let f0 = (wind / 30.0).powi(2) * rng.range(800.0, 3500.0) * (marine::RHO_SEAWATER / 1025.0);
    let c = 2.0 * zeta * (k * m).sqrt();
    let omega_w = 2.0 * std::f64::consts::PI * f_wake;
    let ratio = f_wake / f_n.max(1e-6);
    let in_band = (ratio - 1.0).abs() < 0.18;

    let mut x = 0.0;
    let mut v = 0.0;
    let mut f_act = 0.0;
    let mut peak = 0.0;
    let mut snapped = false;
    let kp = rng.range(4.0e3, 2.5e4);
    let kd = rng.range(200.0, 1800.0);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(m);
    proof.feed_f64(k);
    proof.feed_f64(f_n);
    proof.feed_f64(f_wake);
    proof.feed_f64(tau);

    let steps = (T_SIM * HZ) as usize;
    for tick in 0..steps {
        let t = tick as f64 * DT;
        let wake = f0 * (omega_w * t).sin();
        let cmd = -kp * x - kd * v;
        f_act += (cmd - f_act) * (DT / tau);
        let acc = (wake + f_act - c * v - k * x) / m;
        v += acc * DT;
        x += v * DT;
        let shear = (k * x).abs();
        if shear > peak {
            peak = shear;
        }
        if tick % 40 == 0 {
            proof.feed_f64(x);
        }
        if shear > SHEAR_GATE_N {
            snapped = true;
            break;
        }
    }

    proof.feed_f64(peak);
    proof.feed_str(if snapped {
        "PROBE_SNAP"
    } else if in_band {
        "IN_BAND_HELD"
    } else {
        "OFF_RESONANCE"
    });

    WakeRun {
        id,
        short_id,
        mass_kg: m,
        stiffness_npm: k,
        zeta,
        f_n_hz: f_n,
        f_wake_hz: f_wake,
        freq_ratio: ratio,
        tau_s: tau,
        wind_kts: wind,
        peak_shear_n: peak,
        is_in_band: in_band,
        is_probe_snapped: snapped,
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
                "{}/../../grokd/data/wake_resonance.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: WAKE RESONANCE  (von Kármán vs probe oscillator)");
    println!("  n={n}  {HZ} Hz  shear gate {SHEAR_GATE_N} N");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x5741_4b45_5253_4e43);
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
        Field::new("mass_kg", DataType::Float64, false),
        Field::new("stiffness_npm", DataType::Float64, false),
        Field::new("zeta", DataType::Float64, false),
        Field::new("f_n_hz", DataType::Float64, false),
        Field::new("f_wake_hz", DataType::Float64, false),
        Field::new("freq_ratio", DataType::Float64, false),
        Field::new("tau_s", DataType::Float64, false),
        Field::new("wind_kts", DataType::Float64, false),
        Field::new("peak_shear_n", DataType::Float64, false),
        Field::new("is_in_band", DataType::Boolean, false),
        Field::new("is_probe_snapped", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(rows.iter().map(|r| Some(format!("wak_{}", r.short_id))).collect::<StringArray>()),
            Arc::new(rows.iter().map(|r| Some(r.mass_kg)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.stiffness_npm)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.zeta)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.f_n_hz)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.f_wake_hz)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.freq_ratio)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.tau_s)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.wind_kts)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.peak_shear_n)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.is_in_band)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.is_probe_snapped)).collect::<BooleanArray>()),
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
                "G^G wake resonance dual-regime v1.0".to_string(),
            ),
        ]))
        .build();
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let band = rows.iter().filter(|r| r.is_in_band).count();
    let snap = rows.iter().filter(|r| r.is_probe_snapped).count();
    let both = rows
        .iter()
        .filter(|r| r.is_in_band && r.is_probe_snapped)
        .count();
    println!(
        "  in-band {band} ({:.1}%)  snapped {snap} ({:.1}%)  both {both} ({:.1}%)",
        100.0 * band as f64 / n_f,
        100.0 * snap as f64 / n_f,
        100.0 * both as f64 / n_f
    );
    println!("  seal {run_proof}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
