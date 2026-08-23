//! Mycelial coupling. Kirchhoff on a living circuit: conductance = health / length.
//! Dual-regime: fragmented (source-sink split) vs below percolation (health < 0.4).
//! The question on disk: can a sparse healthy net outperform a dense sick one.

use genesis_core::output;
use genesis_core::physics::mycelial::MycelialMesh;
use genesis_core::proof::{self, ProofChain};
use genesis_core::rng::Rng;
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{BooleanArray, Float64Array, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;

const HZ: f64 = 10.0;
const DT: f64 = 1.0 / HZ;
const T_ESTABLISH: usize = 200; // 20 s
const T_PROPAGATE: usize = 600; // +40 s
const T_MEASURE: usize = 700; // +10 s
const PERCOLATION_HEALTH: f64 = 0.4;
const DELIVERY_GATE: f64 = 0.10;

const STRESSES: [&str; 5] = ["none", "drought", "toxin", "tilling", "pathogen"];

#[derive(Debug, Serialize)]
struct MycelialRun {
    id: u32,
    short_id: String,
    n_nodes: u32,
    radius_m: f64,
    health_mean: f64,
    connectivity: f64,
    stress: String,
    stress_intensity: f64,
    initial_density: f64,
    final_density: f64,
    initial_health: f64,
    final_health: f64,
    delivery_ratio: f64,
    components_after: u32,
    is_fragmented: bool,
    is_below_percolation: bool,
    is_delivered: bool,
    proof_hash: String,
}

fn run_one(id: u32, rng: &mut Rng) -> MycelialRun {
    let short_id = output::short_id(rng);
    let n_nodes = rng.range(24.0, 64.0) as usize;
    let radius = rng.range(200.0, 500.0);
    let health_mean = rng.range(0.12, 0.92);
    let connectivity = rng.range(0.08, 0.70);
    let stress = STRESSES[rng.index(STRESSES.len())];
    let intensity = if stress == "none" {
        0.0
    } else {
        rng.range(0.25, 0.95)
    };

    let mut mesh = MycelialMesh::generate(n_nodes, radius, health_mean, connectivity, rng);
    mesh.propagation_rate = rng.range(0.02, 0.10);
    mesh.decay_rate = rng.range(0.002, 0.04);

    let initial_density = mesh.density();
    let initial_health = mesh.average_edge_health();

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(health_mean);
    proof.feed_f64(connectivity);
    proof.feed_f64(radius);
    proof.feed_str(stress);
    proof.feed_f64(intensity);

    for step in 0..T_ESTABLISH {
        mesh.step_signal(DT);
        mesh.step_nutrients(DT);
        mesh.step_health(DT, mesh.decay_rate);
        if step % 20 == 0 {
            proof.feed_f64(mesh.delivery_ratio());
        }
    }

    if stress != "none" {
        mesh.apply_stress(stress, intensity, rng);
    }

    for step in T_ESTABLISH..T_PROPAGATE {
        mesh.step_signal(DT);
        mesh.step_nutrients(DT);
        mesh.step_health(DT, mesh.decay_rate);
        if step % 20 == 0 {
            proof.feed_f64(mesh.delivery_ratio());
        }
    }

    for step in T_PROPAGATE..T_MEASURE {
        mesh.step_signal(DT);
        mesh.step_nutrients(DT);
        if step % 20 == 0 {
            proof.feed_f64(mesh.delivery_ratio());
        }
    }

    let final_density = mesh.density();
    let final_health = mesh.average_edge_health();
    let delivery = mesh.delivery_ratio();
    let components = mesh.connected_components() as u32;
    let connected = mesh.source_sink_connected();
    let fragmented = !connected;
    let below = final_health < PERCOLATION_HEALTH;
    let delivered = connected && delivery >= DELIVERY_GATE;

    proof.feed_f64(delivery);
    proof.feed_f64(final_health);
    proof.feed_str(if fragmented {
        "FRAGMENTED"
    } else if below {
        "BELOW_PERCOLATION"
    } else if delivered {
        "DELIVERED"
    } else {
        "ATTENUATED"
    });

    MycelialRun {
        id,
        short_id,
        n_nodes: n_nodes as u32,
        radius_m: radius,
        health_mean,
        connectivity,
        stress: stress.to_string(),
        stress_intensity: intensity,
        initial_density,
        final_density,
        initial_health,
        final_health,
        delivery_ratio: delivery,
        components_after: components,
        is_fragmented: fragmented,
        is_below_percolation: below,
        is_delivered: delivered,
        proof_hash: proof.seal(),
    }
}

fn pearson(xs: &[f64], ys: &[f64]) -> f64 {
    let n = xs.len() as f64;
    if n < 3.0 {
        return 0.0;
    }
    let mx = xs.iter().sum::<f64>() / n;
    let my = ys.iter().sum::<f64>() / n;
    let mut cov = 0.0;
    let mut vx = 0.0;
    let mut vy = 0.0;
    for i in 0..xs.len() {
        let dx = xs[i] - mx;
        let dy = ys[i] - my;
        cov += dx * dy;
        vx += dx * dx;
        vy += dy * dy;
    }
    if vx > 0.0 && vy > 0.0 {
        cov / (vx.sqrt() * vy.sqrt())
    } else {
        0.0
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
                "{}/../../grokd/data/mycelial_coupling.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });

    println!("====================================================================");
    println!("  G^G: MYCELIAL COUPLING  (Kirchhoff · health > density)");
    println!("  n={n}  {HZ} Hz  percolation health {PERCOLATION_HEALTH}  delivery {DELIVERY_GATE}");
    println!("====================================================================\n");

    let mut rng = Rng::new(0xFADE_C0DE_BEEF_1234);
    let t0 = Instant::now();
    let mut rows = Vec::with_capacity(n as usize);
    for i in 0..n {
        rows.push(run_one(i, &mut rng));
    }
    let proofs: Vec<_> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let run_proof = proof::seal_run(&proofs);

    let h: Vec<f64> = rows.iter().map(|r| r.health_mean).collect();
    let d: Vec<f64> = rows.iter().map(|r| r.delivery_ratio).collect();
    let dens: Vec<f64> = rows.iter().map(|r| r.initial_density).collect();
    let r_health = pearson(&h, &d);
    let r_dens = pearson(&dens, &d);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::Utf8, false),
        Field::new("n_nodes", DataType::UInt32, false),
        Field::new("radius_m", DataType::Float64, false),
        Field::new("health_mean", DataType::Float64, false),
        Field::new("connectivity", DataType::Float64, false),
        Field::new("stress", DataType::Utf8, false),
        Field::new("stress_intensity", DataType::Float64, false),
        Field::new("initial_density", DataType::Float64, false),
        Field::new("final_density", DataType::Float64, false),
        Field::new("initial_health", DataType::Float64, false),
        Field::new("final_health", DataType::Float64, false),
        Field::new("delivery_ratio", DataType::Float64, false),
        Field::new("components_after", DataType::UInt32, false),
        Field::new("is_fragmented", DataType::Boolean, false),
        Field::new("is_below_percolation", DataType::Boolean, false),
        Field::new("is_delivered", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(rows.iter().map(|r| Some(format!("myc_{}", r.short_id))).collect::<StringArray>()),
            Arc::new(rows.iter().map(|r| Some(r.n_nodes)).collect::<UInt32Array>()),
            Arc::new(rows.iter().map(|r| Some(r.radius_m)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.health_mean)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.connectivity)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.stress.clone())).collect::<StringArray>()),
            Arc::new(rows.iter().map(|r| Some(r.stress_intensity)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.initial_density)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.final_density)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.initial_health)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.final_health)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.delivery_ratio)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.components_after)).collect::<UInt32Array>()),
            Arc::new(rows.iter().map(|r| Some(r.is_fragmented)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.is_below_percolation)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.is_delivered)).collect::<BooleanArray>()),
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
                "G^G mycelial coupling dual-regime v1.0".to_string(),
            ),
            parquet::file::metadata::KeyValue::new(
                "health_delivery_r".to_string(),
                format!("{r_health:.4}"),
            ),
            parquet::file::metadata::KeyValue::new(
                "density_delivery_r".to_string(),
                format!("{r_dens:.4}"),
            ),
        ]))
        .build();
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let n_f = n as f64;
    let frag = rows.iter().filter(|r| r.is_fragmented).count();
    let below = rows.iter().filter(|r| r.is_below_percolation).count();
    let del = rows.iter().filter(|r| r.is_delivered).count();
    let both = rows
        .iter()
        .filter(|r| r.is_fragmented && r.is_below_percolation)
        .count();

    let sh: Vec<&MycelialRun> = rows
        .iter()
        .filter(|r| r.connectivity < 0.25 && r.health_mean > 0.55)
        .collect();
    let ds: Vec<&MycelialRun> = rows
        .iter()
        .filter(|r| r.connectivity > 0.50 && r.health_mean < 0.35)
        .collect();
    let mean = |v: &[&MycelialRun]| -> f64 {
        if v.is_empty() {
            0.0
        } else {
            v.iter().map(|r| r.delivery_ratio).sum::<f64>() / v.len() as f64
        }
    };

    println!(
        "  fragmented {frag} ({:.1}%)  below_percolation {below} ({:.1}%)  both {both} ({:.1}%)  delivered {del} ({:.1}%)",
        100.0 * frag as f64 / n_f,
        100.0 * below as f64 / n_f,
        100.0 * both as f64 / n_f,
        100.0 * del as f64 / n_f
    );
    println!(
        "  sparse-healthy n={} del={:.3}   dense-sick n={} del={:.3}",
        sh.len(),
        mean(&sh),
        ds.len(),
        mean(&ds)
    );
    println!("  r(health, delivery)={r_health:.3}  r(density, delivery)={r_dens:.3}");
    println!("  seal {run_proof}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
