//! Sea-skimming contact vs CA-CFAR threshold. R^4 range law + clutter floor.
//! Dual-regime: buried the whole way (never above CFAR) vs track never reaches 80
//! before impact. Radar clock 20 Hz — 1000 Hz was costume on a 50 s intercept.

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

const HZ: f64 = 20.0;
const DT: f64 = 1.0 / HZ;
const TRACK_GATE: f64 = 80.0;

#[derive(Debug, Serialize)]
struct ClutterRun {
    id: u32,
    short_id: String,
    rcs_dbsm: f64,
    clutter_db: f64,
    cfar_margin_db: f64,
    radar_const_db: f64,
    missile_ms: f64,
    peak_snr_db: f64,
    peak_track: f64,
    t_first_detect_s: f64,
    is_cfar_buried: bool,
    is_impact: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> ClutterRun {
    let short_id = output::short_id(rng);
    let rcs = rng.range(-18.0, 8.0);
    let clutter = rng.range(8.0, 38.0);
    let margin = rng.range(8.0, 18.0);
    let radar_c = rng.range(58.0, 95.0);
    let v_m = rng.range(220.0, 340.0);
    let mut range = rng.range(10_000.0, 18_000.0);

    let mut track = 0.0;
    let mut peak_snr = -80.0;
    let mut peak_track = 0.0;
    let mut ever_above = false;
    let mut t_det = -1.0;
    let mut impact = true;
    let mut elapsed = 0.0;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(rcs);
    proof.feed_f64(clutter);
    proof.feed_f64(margin);
    proof.feed_f64(radar_c);
    proof.feed_f64(marine::SOUND_SPEED);

    let max_t = range / v_m + 2.0;
    while elapsed < max_t && range > 0.0 {
        range -= v_m * DT;
        elapsed += DT;
        let r_km = (range / 1000.0).max(0.05);
        let signal = rcs + radar_c - 40.0 * r_km.log10();
        let grazing = 8.0 / r_km.max(1.0);
        let thresh = clutter + grazing + margin;
        let snr = signal - thresh;
        if snr > peak_snr {
            peak_snr = snr;
        }
        // Detects inside 2.5 km cannot cue an interceptor in time — not a useful lock.
        let useful = range > 2500.0;
        if snr > 0.0 {
            if useful {
                ever_above = true;
                if t_det < 0.0 {
                    t_det = elapsed;
                }
            }
            track += 4.5 * DT;
        } else {
            track -= 6.0 * DT;
        }
        track = track.clamp(0.0, 100.0);
        if track > peak_track {
            peak_track = track;
        }
        if track > TRACK_GATE {
            impact = false;
            break;
        }
        if (elapsed / DT) as u64 % 20 == 0 {
            proof.feed_f64(snr);
        }
    }

    let buried = !ever_above;
    proof.feed_f64(peak_snr);
    proof.feed_str(if !impact {
        "TRACK_HELD"
    } else if buried {
        "CFAR_BURIED"
    } else {
        "LATE_TRACK"
    });

    ClutterRun {
        id,
        short_id,
        rcs_dbsm: rcs,
        clutter_db: clutter,
        cfar_margin_db: margin,
        radar_const_db: radar_c,
        missile_ms: v_m,
        peak_snr_db: peak_snr,
        peak_track,
        t_first_detect_s: t_det,
        is_cfar_buried: buried,
        is_impact: impact,
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
                "{}/../../grokd/data/nav_sea_clutter.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: SEA CLUTTER CFAR  (R^4 vs clutter floor, track gate {TRACK_GATE})");
    println!("  n={n}  {HZ} Hz");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x4346_4152_5345_4153);
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
        Field::new("rcs_dbsm", DataType::Float64, false),
        Field::new("clutter_db", DataType::Float64, false),
        Field::new("cfar_margin_db", DataType::Float64, false),
        Field::new("radar_const_db", DataType::Float64, false),
        Field::new("missile_ms", DataType::Float64, false),
        Field::new("peak_snr_db", DataType::Float64, false),
        Field::new("peak_track", DataType::Float64, false),
        Field::new("t_first_detect_s", DataType::Float64, false),
        Field::new("is_cfar_buried", DataType::Boolean, false),
        Field::new("is_impact", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(rows.iter().map(|r| Some(format!("cfr_{}", r.short_id))).collect::<StringArray>()),
            Arc::new(rows.iter().map(|r| Some(r.rcs_dbsm)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.clutter_db)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.cfar_margin_db)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.radar_const_db)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.missile_ms)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.peak_snr_db)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.peak_track)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.t_first_detect_s)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.is_cfar_buried)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.is_impact)).collect::<BooleanArray>()),
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
                "G^G nav sea clutter CFAR dual-regime v1.0".to_string(),
            ),
        ]))
        .build();
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let buried = rows.iter().filter(|r| r.is_cfar_buried).count();
    let hit = rows.iter().filter(|r| r.is_impact).count();
    let late = rows
        .iter()
        .filter(|r| r.is_impact && !r.is_cfar_buried)
        .count();
    let held = rows.iter().filter(|r| !r.is_impact).count();
    println!(
        "  buried {buried} ({:.1}%)  impact {hit} ({:.1}%)  late_track {late} ({:.1}%)  held {held} ({:.1}%)",
        100.0 * buried as f64 / n_f,
        100.0 * hit as f64 / n_f,
        100.0 * late as f64 / n_f,
        100.0 * held as f64 / n_f
    );
    println!("  seal {run_proof}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
