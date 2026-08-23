//! FKPP traveling wave vs containment. Sibling of Topic 1 microbiome (community).
//! This bank is the threat class: inoculation becomes a front, or a kill term holds it.

use genesis_core::output;
use genesis_core::physics::microbiome;
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

const GRID: usize = 100;

#[derive(Debug, Serialize)]
struct PathogenRun {
    id: u32,
    short_id: String,
    diffusion_coeff: f64,
    replication_rate: f64,
    kill_rate: f64,
    wavefront_x: f64,
    t_breakthrough_s: f64,
    final_avg_concentration: f64,
    is_tissue_overrun: bool,
    is_contained: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> PathogenRun {
    let short_id = output::short_id(rng);
    let d = rng.range(0.05, 0.35);
    let r = rng.range(0.15, 0.85);
    let k = rng.range(0.0, 0.90); // treatment / immune kill
    let dx = 0.1f64;
    let dt = 0.02f64;
    let mut u = vec![0.0f64; GRID];
    u[GRID / 2] = 1.0;
    let mut nxt = vec![0.0f64; GRID];
    let steps = 500;
    let mut t_break = -1.0f64;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(d);
    proof.feed_f64(r);
    proof.feed_f64(k);

    for step in 0..steps {
        microbiome::step_fkpp_1d(&u, &mut nxt, d, r, k, dx, dt);
        u.copy_from_slice(&nxt);
        if t_break < 0.0 && u[GRID - 2] > 0.10 {
            t_break = step as f64 * dt;
        }
        if step % 50 == 0 {
            let avg: f64 = u.iter().sum::<f64>() / GRID as f64;
            proof.feed_f64(avg);
        }
    }

    let avg: f64 = u.iter().sum::<f64>() / GRID as f64;
    let mut front = 0usize;
    for (i, c) in u.iter().enumerate() {
        if *c > 0.10 {
            front = i;
        }
    }
    let overrun = u[GRID - 2] > 0.10;
    let contained = !overrun && avg < 0.20;
    proof.feed_str(if overrun {
        "TISSUE_OVERRUN"
    } else if contained {
        "FRONT_CONTAINED"
    } else {
        "FRONT_RESIDUAL"
    });

    PathogenRun {
        id,
        short_id,
        diffusion_coeff: d,
        replication_rate: r,
        kill_rate: k,
        wavefront_x: front as f64 * dx,
        t_breakthrough_s: t_break,
        final_avg_concentration: avg,
        is_tissue_overrun: overrun,
        is_contained: contained,
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
        .unwrap_or_else(|| "../../grokd/data/pathogen_fkpp_wavefront.parquet".to_string());

    println!("====================================================================");
    println!("  G^G: PATHOGEN FKPP WAVEFRONT  (community's missing threat class)");
    println!("  n={n}  ∂u/∂t = D∇²u + r u(1−u) − k u");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x5041_5448_5f464b50);
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
        Field::new("diffusion_coeff", DataType::Float64, false),
        Field::new("replication_rate", DataType::Float64, false),
        Field::new("kill_rate", DataType::Float64, false),
        Field::new("wavefront_x", DataType::Float64, false),
        Field::new("t_breakthrough_s", DataType::Float64, false),
        Field::new("final_avg_concentration", DataType::Float64, false),
        Field::new("is_tissue_overrun", DataType::Boolean, false),
        Field::new("is_contained", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(rows.iter().map(|r| Some(format!("path_{}", r.short_id))).collect::<StringArray>()),
            Arc::new(rows.iter().map(|r| Some(r.diffusion_coeff)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.replication_rate)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.kill_rate)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.wavefront_x)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.t_breakthrough_s)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.final_avg_concentration)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.is_tissue_overrun)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.is_contained)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.proof_hash.clone())).collect::<StringArray>()),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.clone()),
            parquet::file::metadata::KeyValue::new("generator".to_string(), "G^G pathogen FKPP wavefront v1.1".to_string()),
        ]))
        .build();
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let over = rows.iter().filter(|r| r.is_tissue_overrun).count();
    let held = rows.iter().filter(|r| r.is_contained).count();
    println!("  overrun {over} ({:.1}%)  contained {held} ({:.1}%)", 100.0 * over as f64 / n as f64, 100.0 * held as f64 / n as f64);
    println!("  seal {run_proof}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
