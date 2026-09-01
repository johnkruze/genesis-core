use std::time::Instant;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output;
use genesis_core::rng::Rng;
use genesis_core::physics::atheric::{
    AthericSystem, RESONANCE_GATE
};
use serde::Serialize;
use std::sync::Arc;
use arrow::array::{Float64Array, StringArray, BooleanArray};
use arrow::record_batch::RecordBatch;
use arrow::datatypes::{Schema, Field, DataType};
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::file::properties::WriterProperties;
use parquet::basic::Compression;

#[derive(Debug, Serialize)]
struct AthericRunResult {
    id: u32,
    short_id: String,
    tx_power_dbm: f64,
    distance_km: f64,
    n_jammed_channels: usize,
    clock_desync: bool,
    average_snr_db: f64,
    channel_resonance_coherence: f64,
    shannon_capacity_bps: f64,
    is_snr_blackout: bool,
    is_desync_blackout: bool,
    is_telemetry_resonant: bool,
    proof_hash: String,
}

fn run_single_atheric(
    id: u32,
    rng: &mut Rng,
) -> AthericRunResult {
    let short_id = output::short_id(rng);
    
    // Lab + floor RF: Tx 8–28 dBm, distance 0.02–3.5 km, jam 0–6 of 8 channels, desync ~18%
    let tx_dbm = rng.range(8.0, 28.0);
    let dist_km = rng.range(0.02, 3.5);
    let jam_count = rng.range(0.0, 6.5) as usize;
    let clock_desync = rng.range(0.0, 1.0) < 0.18;

    let tx_watts = genesis_core::physics::atheric::dbm_to_watts(tx_dbm);
    let mut sys = AthericSystem::new(8, tx_watts, -100.0, dist_km);
    // 2.4 GHz ISM Friis path loss
    for (i, ch) in sys.channels.iter_mut().enumerate() {
        let freq_hz = 2.4e9 + (i as f64 * 10e6);
        ch.frequency = freq_hz;
        ch.signal_power =
            genesis_core::physics::atheric::free_space_received(tx_watts, freq_hz, dist_km * 1000.0);
        // Multipath fading in lab clutter
        ch.fading = rng.range(0.35, 1.0);
    }
    
    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(tx_dbm);
    proof.feed_f64(dist_km);
    proof.feed_f64(jam_count as f64);

    if jam_count > 0 {
        sys.apply_jamming(jam_count.min(8), rng);
    }

    if clock_desync {
        sys.apply_clock_drift();
    }

    let min_snr_db = 3.0;
    let avg_snr = sys.avg_snr_db();
    let coherence = sys.coherence(min_snr_db);
    let capacity = sys.total_capacity();

    // Dual-axis blackout: path-loss/jam (SNR/coherence) OR clock desync
    let is_snr_blackout = coherence < 0.50 || avg_snr < min_snr_db;
    let is_desync_blackout = sys.desync;
    let is_resonant = !is_snr_blackout && !is_desync_blackout;

    proof.feed_f64(avg_snr);
    proof.feed_f64(coherence);
    let tag = match (is_desync_blackout, is_snr_blackout) {
        (true, true) => "RF_DESYNC_AND_SNR_BLACKOUT",
        (true, false) => "RF_DESYNC_BLACKOUT",
        (false, true) => "RF_SNR_PATHLOSS_BLACKOUT",
        (false, false) => "ATHERIC_RESONANCE_PASSED",
    };
    proof.feed_str(tag);

    AthericRunResult {
        id,
        short_id,
        tx_power_dbm: tx_dbm,
        distance_km: dist_km,
        n_jammed_channels: jam_count.min(8),
        clock_desync: sys.desync,
        average_snr_db: avg_snr,
        channel_resonance_coherence: coherence,
        shannon_capacity_bps: capacity,
        is_snr_blackout,
        is_desync_blackout,
        is_telemetry_resonant: is_resonant,
        proof_hash: proof.seal(),
    }
}

fn write_parquet_dataset(path: &str, results: &[AthericRunResult], run_proof: &str) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::Utf8, false),
        Field::new("tx_power_dbm", DataType::Float64, false),
        Field::new("distance_km", DataType::Float64, false),
        Field::new("n_jammed_channels", DataType::Float64, false),
        Field::new("clock_desync", DataType::Boolean, false),
        Field::new("average_snr_db", DataType::Float64, false),
        Field::new("channel_resonance_coherence", DataType::Float64, false),
        Field::new("shannon_capacity_bps", DataType::Float64, false),
        Field::new("is_snr_blackout", DataType::Boolean, false),
        Field::new("is_desync_blackout", DataType::Boolean, false),
        Field::new("is_telemetry_resonant", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let ids: StringArray = results.iter().map(|r| Some(format!("rf_{}", r.short_id))).collect();
    let powers: Float64Array = results.iter().map(|r| Some(r.tx_power_dbm)).collect();
    let dists: Float64Array = results.iter().map(|r| Some(r.distance_km)).collect();
    let jams: Float64Array = results.iter().map(|r| Some(r.n_jammed_channels as f64)).collect();
    let desyncs: BooleanArray = results.iter().map(|r| Some(r.clock_desync)).collect();
    let snrs: Float64Array = results.iter().map(|r| Some(r.average_snr_db)).collect();
    let cohs: Float64Array = results.iter().map(|r| Some(r.channel_resonance_coherence)).collect();
    let caps: Float64Array = results.iter().map(|r| Some(r.shannon_capacity_bps)).collect();
    let snr_fail: BooleanArray = results.iter().map(|r| Some(r.is_snr_blackout)).collect();
    let desync_fail: BooleanArray = results.iter().map(|r| Some(r.is_desync_blackout)).collect();
    let resos: BooleanArray = results.iter().map(|r| Some(r.is_telemetry_resonant)).collect();
    let proofs: StringArray = results.iter().map(|r| Some(r.proof_hash.clone())).collect();

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(ids),
            Arc::new(powers),
            Arc::new(dists),
            Arc::new(jams),
            Arc::new(desyncs),
            Arc::new(snrs),
            Arc::new(cohs),
            Arc::new(caps),
            Arc::new(snr_fail),
            Arc::new(desync_fail),
            Arc::new(resos),
            Arc::new(proofs),
        ],
    ).expect("Failed to create RecordBatch");

    let file = std::fs::File::create(path)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.to_string()),
            parquet::file::metadata::KeyValue::new(
                "generator".to_string(),
                "G^G Atheric RF Network Operations v1.1 dual-axis".to_string(),
            ),
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
        .unwrap_or_else(|| "../../doe-genesis/topic-4-autonomous-laboratories/data/autolab_atheric_network_rf.parquet".to_string());

    println!("====================================================================");
    println!("  G^G KERNEL: AUTONOMOUS LAB ATHERIC RF NETWORK OPERATIONS SWEEP");
    println!("  Target Trajectories: {}", n_trajectories);
    println!("  Simulating Frequency Hopping, Friis Path Loss & Shannon Capacity...");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x4154_4845_5249435f);
    let start = Instant::now();

    let mut results = Vec::with_capacity(n_trajectories as usize);
    for i in 0..n_trajectories {
        results.push(run_single_atheric(i, &mut rng));
    }

    let proof_hashes: Vec<_> = results.iter().map(|r| r.proof_hash.clone()).collect();
    let run_proof = proof::seal_run(&proof_hashes);

    write_parquet_dataset(&out_parquet, &results, &run_proof)
        .expect("Failed to write Parquet dataset");

    let resonant_runs = results.iter().filter(|r| r.is_telemetry_resonant).count();
    let blackout_runs = n_trajectories as usize - resonant_runs;

    println!("====================================================================");
    println!("  ATHERIC RF NETWORK SWEEP COMPLETE");
    println!("  Total Trajectories Simulated: {}", n_trajectories);
    println!("  Resonant Telemetry Passed:         {} ({:.1}%)", resonant_runs, (resonant_runs as f64 / n_trajectories as f64) * 100.0);
    println!("  RF Channel Blackout Failures:      {} ({:.1}%)", blackout_runs, (blackout_runs as f64 / n_trajectories as f64) * 100.0);
    println!("  Master SHA-256 Run Proof:           {}", run_proof);
    println!("  Simulation Time:                    {:?}", start.elapsed());
    println!("  Parquet Dataset Written To:          {}", out_parquet);
    println!("====================================================================\n");
}
