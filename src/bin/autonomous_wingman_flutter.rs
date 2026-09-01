//! Transonic wingman. q = ½ρV². Delay-line aileron. Ongoing sin(ωt) at flutter freq.
//! Clock: 200 Hz, 3 s. Gates: |y|≥15 mm flutter vs |y|≥50 mm delamination.
//! Organ: aero::DelayAeroservoelastic, dynamic_pressure_pa. Not LBM.

use genesis_core::output;
use genesis_core::physics::aero::{
    dynamic_pressure_pa, isa_density_kg_m3, tas_from_mach, DelayAeroservoelastic,
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

const DEFAULT_N: usize = 2500;
const DT: f64 = 0.005;
const HORIZON_S: f64 = 3.0;
const Y_FLUTTER_M: f64 = 0.015;
const Y_DELAM_M: f64 = 0.050;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    airspeed_mach: f64,
    latency_ms: f64,
    max_wing_deflection_m: f64,
    is_flutter: bool,
    is_structural_failure: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let mach = rng.range(0.55, 1.15);
    let latency_s = rng.range(0.008, 0.040);
    let alt_m = rng.range(0.0, 8_000.0);
    let f_hz = rng.range(10.0, 22.0);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(mach);
    proof.feed_f64(latency_s);
    proof.feed_f64(alt_m);
    proof.feed_f64(f_hz);

    let rho = isa_density_kg_m3(alt_m);
    let v = tas_from_mach(mach);
    let q = dynamic_pressure_pa(rho, v);
    let q_sl = dynamic_pressure_pa(1.225, tas_from_mach(0.85));
    let aero_amp = 12.0 * (q / q_sl);
    // Delayed aileron: negative gain at small delay damps; 180° of a 12 Hz cycle is ~42 ms.
    let aileron_gain = -25.0 * (q / q_sl) * (latency_s / 0.020);

    let mut wing = DelayAeroservoelastic::new(f_hz, 0.035, latency_s, DT);
    let mut peak: f64 = 0.0;
    let steps = (HORIZON_S / DT) as usize;
    for k in 0..steps {
        let t = k as f64 * DT;
        let y = wing.step(aero_amp * (wing.omega_rad_s * t).sin(), aileron_gain, DT);
        peak = peak.max(y.abs());
        if peak >= Y_DELAM_M {
            break;
        }
        if k % 40 == 0 {
            proof.feed_f64(y);
        }
    }

    let flutter = peak >= Y_FLUTTER_M;
    let delam = peak >= Y_DELAM_M;
    proof.feed_f64(peak);
    proof.feed_str(if delam {
        "DELAMINATED"
    } else if flutter {
        "FLUTTER"
    } else {
        "NOMINAL"
    });

    Run {
        id,
        short_id,
        airspeed_mach: (mach * 100.0).round() / 100.0,
        latency_ms: (latency_s * 1e3 * 10.0).round() / 10.0,
        max_wing_deflection_m: (peak * 1e4).round() / 1e4,
        is_flutter: flutter,
        is_structural_failure: delam,
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
                "{}/../../data/exports/sovereign/autonomous_wingman_flutter.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: WINGMAN DELAY-LOOP  (q=½ρV², 200 Hz, sin(ωt))");
    println!("  n={n}  flutter {Y_FLUTTER_M} m  delam {Y_DELAM_M} m");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let mut rng = Rng::new(0xF107_7E80);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("airspeed_mach", DataType::Float64, false),
        Field::new("latency_ms", DataType::Float64, false),
        Field::new("max_wing_deflection_m", DataType::Float64, false),
        Field::new("is_flutter", DataType::Boolean, false),
        Field::new("is_structural_failure", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.airspeed_mach).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.latency_ms).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.max_wing_deflection_m).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_flutter).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_structural_failure).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G wingman delay-loop dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let n_f = n as f64;
    let f = rows.iter().filter(|r| r.is_flutter).count();
    let d = rows.iter().filter(|r| r.is_structural_failure).count();
    println!(
        "  flutter {f} ({:.1}%)  delam {d} ({:.1}%)",
        100.0 * f as f64 / n_f,
        100.0 * d as f64 / n_f
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
