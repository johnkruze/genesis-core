//! 48-hour motion: point Cauchy inverse design at a hashed mesh.
//! `--mesh path.obj` · `--load-n` · n=2500 dual-regime aligned vs unaligned.

use genesis_core::output;
use genesis_core::physics::materials::CauchyStressTensor;
use genesis_core::proof::{self, ProofChain};
use genesis_core::rng::Rng;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{BooleanArray, Float64Array, StringArray, UInt64Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;

struct MeshExtent {
    sha256: String,
    n_verts: u64,
    n_faces: u64,
    length_m: f64,
    height_m: f64,
    width_m: f64,
}

fn parse_obj(path: &str) -> MeshExtent {
    let bytes = std::fs::read(path).unwrap_or_default();
    let sha256 = hex::encode(Sha256::digest(&bytes));
    let text = String::from_utf8_lossy(&bytes);
    let mut n_verts = 0u64;
    let mut n_faces = 0u64;
    let mut min = [f64::MAX; 3];
    let mut max = [f64::MIN; 3];
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("v ") {
            let mut it = rest.split_whitespace();
            if let (Some(xs), Some(ys), Some(zs)) = (it.next(), it.next(), it.next()) {
                if let (Ok(x), Ok(y), Ok(z)) = (xs.parse::<f64>(), ys.parse::<f64>(), zs.parse::<f64>()) {
                    n_verts += 1;
                    min[0] = min[0].min(x);
                    min[1] = min[1].min(y);
                    min[2] = min[2].min(z);
                    max[0] = max[0].max(x);
                    max[1] = max[1].max(y);
                    max[2] = max[2].max(z);
                }
            }
        } else if line.starts_with("f ") {
            n_faces += 1;
        }
    }
    MeshExtent {
        sha256,
        n_verts,
        n_faces,
        length_m: (max[0] - min[0]).max(1e-6),
        height_m: (max[1] - min[1]).max(1e-6),
        width_m: (max[2] - min[2]).max(1e-6),
    }
}

#[derive(Debug, Serialize)]
struct ForgeRow {
    id: u32,
    short_id: String,
    tip_load_n: f64,
    alignment_score: f64,
    unaligned_peak_mpa: f64,
    aligned_peak_mpa: f64,
    is_unaligned_yielded: bool,
    is_forge_yielded: bool,
    load_capacity_ratio: f64,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng, mesh: &MeshExtent, load_n: f64) -> ForgeRow {
    let short_id = output::short_id(rng);
    let tip = if load_n > 0.0 {
        load_n * rng.range(0.4, 1.6)
    } else {
        rng.range(10_000.0, 300_000.0)
    };
    let alignment = rng.range(0.25, 1.0);
    let length_mm = mesh.length_m * 1000.0;
    let height_mm = mesh.height_m * 1000.0;
    let r_bound = (mesh.width_m * 1000.0 * 0.5).max(1.0);
    let inertia = std::f64::consts::PI * r_bound.powi(4) / 4.0;
    let moment = tip * length_mm;
    let sigma_u = moment * (height_mm * 0.5) / inertia;
    let tau_u = tip / (std::f64::consts::PI * r_bound.powi(2));
    let t_u = CauchyStressTensor {
        sigma_xx: sigma_u,
        sigma_yy: sigma_u * 0.1,
        sigma_zz: sigma_u * 0.05,
        tau_xy: tau_u * 0.3,
        tau_xz: tau_u,
        tau_yz: tau_u * 0.15,
    };
    let (pu, _) = t_u.solve_principal_eigensystem();
    let yield_u = 85.0;
    let unaligned_peak = pu[0];
    let unaligned_yielded = unaligned_peak > yield_u;

    let yield_a = 150.0 + 700.0 * alignment;
    let inertia_a = inertia * (1.2 + alignment);
    let sigma_a = moment * (height_mm * 0.5) / inertia_a;
    let tau_a = tau_u * (1.0 - 0.45 * alignment);
    let t_a = CauchyStressTensor {
        sigma_xx: sigma_a,
        sigma_yy: sigma_a * 0.1,
        sigma_zz: sigma_a * 0.05,
        tau_xy: tau_a * 0.2,
        tau_xz: tau_a,
        tau_yz: tau_a * 0.1,
    };
    let (pa, _) = t_a.solve_principal_eigensystem();
    let aligned_peak = pa[0];
    let aligned_yielded = aligned_peak > yield_a;
    let cap_u = (tip / 1000.0) * (yield_u / unaligned_peak.max(1e-9));
    let cap_a = (tip / 1000.0) * (yield_a / aligned_peak.max(1e-9));

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_str(&mesh.sha256);
    proof.feed_f64(tip);
    proof.feed_f64(unaligned_peak);
    proof.feed_f64(aligned_peak);
    proof.feed_str(if aligned_yielded { "FORGE_YIELDED" } else { "FORGE_HELD" });

    ForgeRow {
        id,
        short_id,
        tip_load_n: tip,
        alignment_score: alignment,
        unaligned_peak_mpa: unaligned_peak,
        aligned_peak_mpa: aligned_peak,
        is_unaligned_yielded: unaligned_yielded,
        is_forge_yielded: aligned_yielded,
        load_capacity_ratio: cap_a / cap_u.max(1e-9),
        proof_hash: proof.seal(),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(2500);
    let mesh_path = args
        .iter()
        .position(|a| a == "--mesh")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| {
            "../../doe-genesis/topic-3-materials-predictable-functionality/data/forge_strut.obj"
                .to_string()
        });
    let load_n: f64 = args
        .iter()
        .position(|a| a == "--load-n")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(50_000.0);
    let out = args
        .iter()
        .position(|a| a == "--parquet")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "../../grokd/data/forge_living_geometry.parquet".to_string());

    let mesh = parse_obj(&mesh_path);
    println!("====================================================================");
    println!("  G^G LIVING GEOMETRY FORGE");
    println!("  mesh {}  sha256:{}…", mesh_path, &mesh.sha256[..16]);
    println!("  verts={} faces={}  L={:.4} m H={:.4} m W={:.4} m", mesh.n_verts, mesh.n_faces, mesh.length_m, mesh.height_m, mesh.width_m);
    println!("  n={n}  load={load_n} N");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x4c49_5645_5f47454f);
    let t0 = Instant::now();
    let mut rows = Vec::with_capacity(n as usize);
    for i in 0..n {
        rows.push(run_one(i, &mut rng, &mesh, load_n));
    }
    let proofs: Vec<_> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let run_proof = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::Utf8, false),
        Field::new("tip_load_n", DataType::Float64, false),
        Field::new("alignment_score", DataType::Float64, false),
        Field::new("unaligned_peak_mpa", DataType::Float64, false),
        Field::new("aligned_peak_mpa", DataType::Float64, false),
        Field::new("is_unaligned_yielded", DataType::Boolean, false),
        Field::new("is_forge_yielded", DataType::Boolean, false),
        Field::new("load_capacity_ratio", DataType::Float64, false),
        Field::new("mesh_n_verts", DataType::UInt64, false),
        Field::new("mesh_n_faces", DataType::UInt64, false),
        Field::new("mesh_sha256", DataType::Utf8, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let nv = mesh.n_verts;
    let nf = mesh.n_faces;
    let sha = mesh.sha256.clone();
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(rows.iter().map(|r| Some(format!("lg_{}", r.short_id))).collect::<StringArray>()),
            Arc::new(rows.iter().map(|r| Some(r.tip_load_n)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.alignment_score)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.unaligned_peak_mpa)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.aligned_peak_mpa)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.is_unaligned_yielded)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.is_forge_yielded)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.load_capacity_ratio)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|_| Some(nv)).collect::<UInt64Array>()),
            Arc::new(rows.iter().map(|_| Some(nf)).collect::<UInt64Array>()),
            Arc::new(rows.iter().map(|_| Some(sha.clone())).collect::<StringArray>()),
            Arc::new(rows.iter().map(|r| Some(r.proof_hash.clone())).collect::<StringArray>()),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.clone()),
            parquet::file::metadata::KeyValue::new("generator".to_string(), "G^G living geometry forge v1.0".to_string()),
            parquet::file::metadata::KeyValue::new("mesh_sha256".to_string(), mesh.sha256.clone()),
            parquet::file::metadata::KeyValue::new("mesh_path".to_string(), mesh_path.clone()),
        ]))
        .build();
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let uy = rows.iter().filter(|r| r.is_unaligned_yielded).count();
    let fy = rows.iter().filter(|r| r.is_forge_yielded).count();
    println!("  unaligned yielded {uy} ({:.1}%)  forge yielded {fy} ({:.1}%)", 100.0 * uy as f64 / n as f64, 100.0 * fy as f64 / n as f64);
    println!("  seal {run_proof}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
