// J1989 Planetary Time-Evolution — Yoshida vs JPL Horizons kernel slice
// Epoch: April 26, 1989, 11:14 AM PDT (18:14 UTC)

use genesis_core::physics::ephemeris::{
    ephemeris_gate_km, BodyState, Ephemeris, JplEphemerisLoader, NBodyState,
};
use genesis_core::proof::{self, ProofChain};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{BooleanArray, Float64Array, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;

const AU_KM: f64 = 149_597_870.7;
const BODY_ORDER: [&str; 11] = [
    "Sun", "Mercury", "Venus", "Earth", "Moon", "Mars", "Jupiter", "Saturn", "Uranus", "Neptune",
    "Pluto",
];

struct ResidualRow {
    day: u32,
    jd: f64,
    body: String,
    r_int_km: [f64; 3],
    r_jpl_km: [f64; 3],
    delta_r_km: f64,
    delta_v_kms: f64,
    gate_km: f64,
    is_ephemeris_coherent: bool,
    proof_hash: String,
}

fn mag(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn write_parquet(path: &str, rows: &[ResidualRow], run_proof: &str, kernel_sha: &str) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("day", DataType::UInt32, false),
        Field::new("jd_tdb", DataType::Float64, false),
        Field::new("body", DataType::Utf8, false),
        Field::new("r_int_x_km", DataType::Float64, false),
        Field::new("r_int_y_km", DataType::Float64, false),
        Field::new("r_int_z_km", DataType::Float64, false),
        Field::new("r_jpl_x_km", DataType::Float64, false),
        Field::new("r_jpl_y_km", DataType::Float64, false),
        Field::new("r_jpl_z_km", DataType::Float64, false),
        Field::new("delta_r_km", DataType::Float64, false),
        Field::new("delta_v_kms", DataType::Float64, false),
        Field::new("gate_km", DataType::Float64, false),
        Field::new("is_ephemeris_coherent", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let days: UInt32Array = rows.iter().map(|r| Some(r.day)).collect();
    let jds: Float64Array = rows.iter().map(|r| Some(r.jd)).collect();
    let bodies: StringArray = rows.iter().map(|r| Some(r.body.clone())).collect();
    let ix: Float64Array = rows.iter().map(|r| Some(r.r_int_km[0])).collect();
    let iy: Float64Array = rows.iter().map(|r| Some(r.r_int_km[1])).collect();
    let iz: Float64Array = rows.iter().map(|r| Some(r.r_int_km[2])).collect();
    let jx: Float64Array = rows.iter().map(|r| Some(r.r_jpl_km[0])).collect();
    let jy: Float64Array = rows.iter().map(|r| Some(r.r_jpl_km[1])).collect();
    let jz: Float64Array = rows.iter().map(|r| Some(r.r_jpl_km[2])).collect();
    let dr: Float64Array = rows.iter().map(|r| Some(r.delta_r_km)).collect();
    let dv: Float64Array = rows.iter().map(|r| Some(r.delta_v_kms)).collect();
    let gates: Float64Array = rows.iter().map(|r| Some(r.gate_km)).collect();
    let coh: BooleanArray = rows.iter().map(|r| Some(r.is_ephemeris_coherent)).collect();
    let proofs: StringArray = rows.iter().map(|r| Some(r.proof_hash.clone())).collect();

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(days),
            Arc::new(jds),
            Arc::new(bodies),
            Arc::new(ix),
            Arc::new(iy),
            Arc::new(iz),
            Arc::new(jx),
            Arc::new(jy),
            Arc::new(jz),
            Arc::new(dr),
            Arc::new(dv),
            Arc::new(gates),
            Arc::new(coh),
            Arc::new(proofs),
        ],
    )
    .expect("batch");

    let file = std::fs::File::create(path)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.to_string()),
            parquet::file::metadata::KeyValue::new(
                "generator".to_string(),
                "G^G john_birth_celestial v2 JPL-Horizons residual".to_string(),
            ),
            parquet::file::metadata::KeyValue::new("kernel_sha256".to_string(), kernel_sha.to_string()),
            parquet::file::metadata::KeyValue::new("epoch".to_string(), "1989-04-26T18:14:00Z".to_string()),
        ]))
        .build();
    let mut writer = ArrowWriter::try_new(file, schema, Some(props)).expect("writer");
    writer.write(&batch).expect("write");
    writer.close().expect("close");
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let kernel_path = args
        .iter()
        .position(|a| a == "--kernel")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "data/jpl/epoch_19890426.json".to_string());
    let out_parquet = args
        .iter()
        .position(|a| a == "--parquet")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "../../grokd/data/celestial_de440_residual.parquet".to_string());

    let mut jpl = JplEphemerisLoader::new(&kernel_path);
    jpl.load().unwrap_or_else(|e| panic!("JPL kernel: {e}"));
    let kernel_bytes = std::fs::read(&kernel_path).expect("kernel bytes");
    let kernel_sha = hex::encode(Sha256::digest(&kernel_bytes));

    let jd0 = jpl.jd_start;
    let total_days = 365u32;
    let dt = 3_600.0; // 1 hour — Moon is in the dance
    let steps_per_day = 24u32;

    println!("==================================================================");
    println!("  CELESTIAL N-BODY: epoch_19890426  (11:14 AM PDT)");
    println!("  Integrator: 4th-Order Yoshida  ·  dt = 1 hour  ·  11 bodies");
    println!("  Truth:      JPL Horizons DE440/441  ·  SSB ECLIPJ2000");
    println!("  Kernel:     {}  sha256:{}…", kernel_path, &kernel_sha[..16]);
    println!("  JD TDB:     {:.6} → +{} days", jd0, total_days);
    println!("==================================================================\n");

    let mut system = NBodyState::new();
    for name in BODY_ORDER {
        let r = jpl.get_position(name, jd0).expect("r0");
        let v = jpl.get_velocity(name, jd0).expect("v0");
        let mu = jpl.get_mu(name).expect("mu");
        system.insert_body(
            name,
            BodyState {
                position: r,
                velocity: v,
                mu,
            },
        );
        println!(
            "  {name:8}  r0 = [{:10.4}, {:10.4}, {:10.4}] AU  |r|={:.4} AU",
            r[0] / AU_KM,
            r[1] / AU_KM,
            r[2] / AU_KM,
            mag(r) / AU_KM
        );
    }
    let pluto_z = system.bodies.get("Pluto").unwrap().position[2] / AU_KM;
    println!("\n  Pluto Z offset (JPL, not a comment): {pluto_z:.4} AU\n");

    let mut walk = ProofChain::new();
    walk.seed(b"epoch_19890426");
    walk.feed_str(&kernel_sha);

    let mut rows = Vec::with_capacity((total_days as usize + 1) * BODY_ORDER.len());
    let t0 = Instant::now();

    for day in 0..=total_days {
        let jd = jd0 + day as f64;
        for name in BODY_ORDER {
            let integ = system.bodies.get(name).unwrap();
            let r_int = integ.position;
            let v_int = integ.velocity;
            let r_jpl = jpl.get_position(name, jd).unwrap();
            let v_jpl = jpl.get_velocity(name, jd).unwrap();
            let dr = [
                r_int[0] - r_jpl[0],
                r_int[1] - r_jpl[1],
                r_int[2] - r_jpl[2],
            ];
            let dv = [
                v_int[0] - v_jpl[0],
                v_int[1] - v_jpl[1],
                v_int[2] - v_jpl[2],
            ];
            let delta_r = mag(dr);
            let delta_v = mag(dv);
            let gate = ephemeris_gate_km(name);
            let coherent = delta_r <= gate;

            let mut row_proof = ProofChain::new();
            row_proof.seed(b"epoch_19890426");
            row_proof.feed_str(name);
            row_proof.feed_f64(jd);
            row_proof.feed_f64(delta_r);
            row_proof.feed_f64(delta_v);
            let ph = row_proof.seal();

            walk.feed_f64(r_int[0]);
            walk.feed_f64(delta_r);

            rows.push(ResidualRow {
                day,
                jd,
                body: name.to_string(),
                r_int_km: r_int,
                r_jpl_km: r_jpl,
                delta_r_km: delta_r,
                delta_v_kms: delta_v,
                gate_km: gate,
                is_ephemeris_coherent: coherent,
                proof_hash: ph,
            });
        }

        if day % 30 == 0 || day == total_days {
            let earth_dr = rows
                .iter()
                .rev()
                .find(|r| r.body == "Earth")
                .unwrap()
                .delta_r_km;
            let pluto_dr = rows
                .iter()
                .rev()
                .find(|r| r.body == "Pluto")
                .unwrap()
                .delta_r_km;
            let earth = system.bodies.get("Earth").unwrap();
            println!(
                "Day {:03} | Earth |r|={:.4} AU  Δr_earth={:.3} km  Δr_pluto={:.3} km",
                day,
                mag(earth.position) / AU_KM,
                earth_dr,
                pluto_dr
            );
        }

        if day < total_days {
            for _ in 0..steps_per_day {
                system.step_nbody(dt);
            }
        }
    }

    let hashes: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let run_proof = proof::seal_run(&hashes);
    let walk_seal = walk.seal();
    write_parquet(&out_parquet, &rows, &run_proof, &kernel_sha).expect("parquet");

    let n = rows.len() as f64;
    let coherent = rows.iter().filter(|r| r.is_ephemeris_coherent).count();
    println!("------------------------------------------------------------------");
    println!("  rows:              {}", rows.len());
    println!(
        "  coherent:          {} ({:.1}%)",
        coherent,
        100.0 * coherent as f64 / n
    );
    for name in BODY_ORDER {
        let terminal = rows
            .iter()
            .rev()
            .find(|r| r.body == *name)
            .unwrap();
        println!(
            "  {name:8} day365  Δr={:12.3} km  Δv={:.6} km/s  gate={:.0} km  {}",
            terminal.delta_r_km,
            terminal.delta_v_kms,
            terminal.gate_km,
            if terminal.is_ephemeris_coherent {
                "IN GATE"
            } else {
                "DRIFT"
            }
        );
    }
    println!("  walk seal:         {}", walk_seal);
    println!("  run proof:         {}", run_proof);
    println!("  parquet:           {}", out_parquet);
    println!("  time:              {:?}", t0.elapsed());
    println!("==================================================================");
}
