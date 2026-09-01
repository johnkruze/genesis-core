//! Class-8 air line. Organ-pipe f=c/(2L), Joukowsky ΔP=ρ c Δv. Mix coincident ABS vs not.
//! Clock: constitutive. Gates: coincident |Δf|<0.8 Hz vs delivered P < 60 psi.

use genesis_core::output;
use genesis_core::physics::hydraulics::{
    acoustic_fluid_wave_speed_m_s, pneumatic_line_resonance_freq_hz, AIR_BULK_MODULUS_PA,
};
use genesis_core::physics::resonance::vibration_transmissibility;
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
const P_SYS_PA: f64 = 8.27e5; // 120 psi
const P_MIN_PA: f64 = 4.14e5; // 60 psi

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    line_length_m: f64,
    min_delivered_psi: f64,
    is_coincident: bool,
    is_pneumatic_choked: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let l = rng.range(14.0, 26.0);
    let coincident_mix = rng.chance(0.38);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(l);

    let c = acoustic_fluid_wave_speed_m_s(AIR_BULK_MODULUS_PA, 1.2);
    let f0 = pneumatic_line_resonance_freq_hz(c, l, false);
    let abs_hz = if coincident_mix {
        f0 * rng.range(0.96, 1.04)
    } else {
        rng.range(4.0, 14.0)
    };
    let df = (f0 - abs_hz).abs();
    let coincident = df < 0.80;
    // Joukowsky in air is kPa — it does not choke 120 psi. Organ-pipe TR does.
    let tr = vibration_transmissibility(abs_hz, f0, 0.10);
    let p_del = P_SYS_PA / tr.max(1.0);
    let choke = p_del < P_MIN_PA;

    proof.feed_f64(p_del);
    proof.feed_str(if choke {
        "CHOKED"
    } else if coincident {
        "COINCIDENT"
    } else {
        "LIVE"
    });

    Run {
        id,
        short_id,
        line_length_m: (l * 10.0).round() / 10.0,
        min_delivered_psi: (p_del / 6895.0 * 10.0).round() / 10.0,
        is_coincident: coincident,
        is_pneumatic_choked: choke,
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
                "{}/../../data/exports/sovereign/pneumatic_line_resonance.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    println!("====================================================================");
    println!("  G^G: AIR LINE  (organ-pipe + Joukowsky)");
    println!("  n={n}");
    println!("====================================================================\n");
    let t0 = Instant::now();
    let mut rng = Rng::new(0xA11E_00B3);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("line_length_m", DataType::Float64, false),
        Field::new("min_delivered_psi", DataType::Float64, false),
        Field::new("is_coincident", DataType::Boolean, false),
        Field::new("is_pneumatic_choked", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.line_length_m).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.min_delivered_psi).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_coincident).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_pneumatic_choked).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G air-line Joukowsky dual-regime v3.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let nf = n as f64;
    let a = rows.iter().filter(|r| r.is_coincident).count();
    let b = rows.iter().filter(|r| r.is_pneumatic_choked).count();
    println!(
        "  coincident {a} ({:.1}%)  choke {b} ({:.1}%)",
        100.0 * a as f64 / nf,
        100.0 * b as f64 / nf
    );
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
