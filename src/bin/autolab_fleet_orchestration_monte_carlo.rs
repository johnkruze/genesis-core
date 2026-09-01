use std::sync::Arc;
use std::time::Instant;
use arrow::array::{Float64Array, StringArray, BooleanArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use serde::Serialize;

use genesis_core::output;
use genesis_core::physics::atheric::SPEED_OF_LIGHT;
use genesis_core::proof::{self, ProofChain};
use genesis_core::rng::Rng;

#[derive(Debug, Serialize)]
struct FleetRunResult {
    id: u32,
    short_id: String,
    fleet_velocity_m_s: f64,
    inter_robot_gap_m: f64,
    comms_latency_ms: f64,
    calculated_stopping_distance_m: f64,
    required_safety_buffer_m: f64,
    is_ebrake_engaged: bool,
    is_collision_prevented: bool,
    proof_hash: String,
}

fn run_single_fleet(
    id: u32,
    rng: &mut Rng,
) -> FleetRunResult {
    let short_id = output::short_id(rng);
    
    // Sweep fleet velocity (0.5 to 4.5 m/s), inter-robot gap (0.5 to 8.0 m), comms latency (5 to 500 ms)
    let velocity = rng.range(0.5, 4.5);
    let gap = rng.range(0.5, 8.0);
    let c_floor_ms = (gap / SPEED_OF_LIGHT) * 1e3;
    let latency_ms = rng.range(5.0, 500.0).max(c_floor_ms);
    let brake_decel = rng.range(2.5, 6.0); // 2.5 to 6.0 m/s^2 emergency deceleration

    // Reaction distance d_react = velocity * (latency / 1000)
    let react_dist = velocity * (latency_ms / 1000.0);
    // Braking distance d_brake = velocity^2 / (2 * decel)
    let brake_dist = (velocity * velocity) / (2.0 * brake_decel);
    let total_stopping_distance = react_dist + brake_dist;

    let required_safety_buffer = total_stopping_distance + 0.25; // 25cm safety margin
    let is_ebrake_engaged = total_stopping_distance > (gap * 0.70) || latency_ms > 150.0;
    let is_collision_prevented = gap >= total_stopping_distance;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(velocity);
    proof.feed_f64(gap);
    proof.feed_f64(latency_ms);
    proof.feed_f64(total_stopping_distance);
    proof.feed_str(if is_collision_prevented { "COLLISION_PREVENTED" } else { "STOPPING_ENVELOPE_BREACHED" });

    FleetRunResult {
        id,
        short_id,
        fleet_velocity_m_s: velocity,
        inter_robot_gap_m: gap,
        comms_latency_ms: latency_ms,
        calculated_stopping_distance_m: total_stopping_distance,
        required_safety_buffer_m: required_safety_buffer,
        is_ebrake_engaged,
        is_collision_prevented,
        proof_hash: proof.seal(),
    }
}

fn write_parquet_dataset(path: &str, results: &[FleetRunResult], run_proof: &str) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::Utf8, false),
        Field::new("fleet_velocity_m_s", DataType::Float64, false),
        Field::new("inter_robot_gap_m", DataType::Float64, false),
        Field::new("comms_latency_ms", DataType::Float64, false),
        Field::new("calculated_stopping_distance_m", DataType::Float64, false),
        Field::new("required_safety_buffer_m", DataType::Float64, false),
        Field::new("is_ebrake_engaged", DataType::Boolean, false),
        Field::new("is_collision_prevented", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let ids: StringArray = results.iter().map(|r| Some(format!("fleet_{}", r.short_id))).collect();
    let vels: Float64Array = results.iter().map(|r| Some(r.fleet_velocity_m_s)).collect();
    let gaps: Float64Array = results.iter().map(|r| Some(r.inter_robot_gap_m)).collect();
    let lats: Float64Array = results.iter().map(|r| Some(r.comms_latency_ms)).collect();
    let stops: Float64Array = results.iter().map(|r| Some(r.calculated_stopping_distance_m)).collect();
    let bufs: Float64Array = results.iter().map(|r| Some(r.required_safety_buffer_m)).collect();
    let ebrakes: BooleanArray = results.iter().map(|r| Some(r.is_ebrake_engaged)).collect();
    let prevents: BooleanArray = results.iter().map(|r| Some(r.is_collision_prevented)).collect();
    let proofs: StringArray = results.iter().map(|r| Some(r.proof_hash.clone())).collect();

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(ids),
            Arc::new(vels),
            Arc::new(gaps),
            Arc::new(lats),
            Arc::new(stops),
            Arc::new(bufs),
            Arc::new(ebrakes),
            Arc::new(prevents),
            Arc::new(proofs),
        ],
    ).expect("Failed to create RecordBatch");

    let file = std::fs::File::create(path)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.to_string()),
            parquet::file::metadata::KeyValue::new("generator".to_string(), "G^G Multi-Robot Fleet Orchestration & E-Stop v1.0".to_string()),
        ]))
        .build();

    let mut writer = ArrowWriter::try_new(file, schema, Some(props))
        .expect("Failed to create Parquet ArrowWriter");
    writer.write(&batch).expect("Failed to write Parquet batch");
    writer.close().expect("Failed to close Parquet writer");

    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n_trajectories: u32 = args.get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(2_500);

    let out_parquet = args.iter().position(|a| a == "--parquet")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "../../doe-genesis/topic-4-autonomous-laboratories/data/autolab_fleet_orchestration_ebrake.parquet".to_string());

    println!("====================================================================");
    println!("  G^G KERNEL: MULTI-ROBOT FLEET ORCHESTRATION & E-STOP SWEEP");
    println!("  Target Trajectories: {}", n_trajectories);
    println!("  Simulating Comms Blackout, Stopping Envelopes & E-Stop Reflex...");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x464C_4545_545f4f52); // Seed "FLEET_OR"
    let start = Instant::now();

    let mut results = Vec::with_capacity(n_trajectories as usize);
    for i in 0..n_trajectories {
        results.push(run_single_fleet(i, &mut rng));
    }

    let mut master_proof = ProofChain::new();
    master_proof.seed(b"G^G_FLEET_MASTER_PROOF_v1.0");
    for r in &results {
        master_proof.feed_str(&r.proof_hash);
    }
    let master_seal = master_proof.seal();

    write_parquet_dataset(&out_parquet, &results, &master_seal)
        .expect("Failed to write Parquet dataset");

    let collision_prevented = results.iter().filter(|r| r.is_collision_prevented).count();
    let ebrake_engaged = results.iter().filter(|r| r.is_ebrake_engaged).count();

    println!("====================================================================");
    println!("  MULTI-ROBOT FLEET SWEEP COMPLETE");
    println!("  Total Trajectories Simulated: {}", n_trajectories);
    println!("  Collision Avoidance Secured:       {} ({:.1}%)", collision_prevented, (collision_prevented as f64 / n_trajectories as f64) * 100.0);
    println!("  Emergency E-Brake Engaged:         {} ({:.1}%)", ebrake_engaged, (ebrake_engaged as f64 / n_trajectories as f64) * 100.0);
    println!("  Master SHA-256 Run Proof:           {}", master_seal);
    println!("  Simulation Time:                    {:?}", start.elapsed());
    println!("  Parquet Dataset Written To:          {}", out_parquet);
    println!("====================================================================\n");
}
