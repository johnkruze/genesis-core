use genesis_core::last_state::{LastStateFrame64, BODY_PLASMA, FLAG_PLASMA_BLACKOUT, FLAG_PLASMA_GPS_HELD, FLAG_PLASMA_MISS};
use genesis_core::physics::aero::{tas_from_mach, C_PlasmaState, GPS_L1_HZ};
use genesis_core::sweeper::{self, OracleVerdict};
use std::path::Path;
use std::time::Instant;

fn pack_from_state(step: u32, state: &C_PlasmaState) -> LastStateFrame64 {
    let fp_ghz = (state.peak_fp_hz / 1.0e9) as f32;
    let l1_ghz = (GPS_L1_HZ / 1.0e9) as f32;
    let mut frame = LastStateFrame64 {
        t: step as f64 * 0.05,
        pos: [state.x_m as f32, state.z_m as f32, state.tgt_m as f32],
        vel: [fp_ghz, l1_ghz, state.miss_m as f32],
        force_torque: if l1_ghz > 0.0 { fp_ghz / l1_ghz } else { 0.0 },
        residual: state.miss_m as f32,
        flags: 0,
        proof: [0; 16],
    };
    if state.blackout {
        frame.flags |= FLAG_PLASMA_BLACKOUT;
    }
    if state.miss_m > 39.0 {
        frame.flags |= FLAG_PLASMA_MISS;
    }
    if state.gps_held {
        frame.flags |= FLAG_PLASMA_GPS_HELD;
    }
    frame
}

fn main() {
    let domain = sweeper::soma_frame_domain(BODY_PLASMA);
    println!("═══════════════════════════════════════════════════════════");
    println!("  HYPERSONIC: sheath step interleaved with frame pack");
    println!("  Domain: {domain}");
    println!("═══════════════════════════════════════════════════════════\n");

    let mach = 6.0;
    let mut state = C_PlasmaState::reentry(mach, 28_000.0, 40.0_f64.to_radians(), 12.0);
    let v = tas_from_mach(mach);
    println!("  TAS {v:.1} m/s  dt 0.05 s");

    let steps = 100_000u32;
    let mut frames: Vec<LastStateFrame64> = Vec::with_capacity(steps as usize);
    let started = Instant::now();
    for i in 0..steps {
        state.step(0.05);
        frames.push(pack_from_state(i, &state));
    }
    let elapsed = started.elapsed();
    println!(
        "  {} step+pack pairs in {:.2?} ({:.1} ns/pair)",
        steps,
        elapsed,
        elapsed.as_secs_f64() * 1.0e9 / steps as f64
    );
    println!(
        "  final z={:.1} m  blackout={}  miss={:.2} m  fp={:.2} GHz",
        state.z_m, state.blackout, state.miss_m, state.peak_fp_hz / 1.0e9
    );

    let (tx, rx) = sweeper::frame_channel(1024);
    let failed = Path::new(sweeper::FAILED_ANCHORS_DIR).to_path_buf();
    let sweeper = std::thread::spawn(move || {
        let mut sealed = Vec::with_capacity(frames_hint(steps));
        while let Ok(raw) = rx.recv() {
            let mut frame = LastStateFrame64::from_bytes(raw);
            frame.reseal();
            sealed.push(frame);
        }
        let digest = sweeper::frame_bytes_digest(&sealed);
        let verdict = sweeper::record_trajectory_proof(&domain, &digest);
        if verdict.anchors_locally() {
            let (reason, candid_raw) = match &verdict {
                OracleVerdict::Rejected { reason, raw } => (reason.clone(), raw.clone()),
                OracleVerdict::Transport { reason } => (reason.clone(), String::new()),
                OracleVerdict::Recorded { .. } => unreachable!(),
            };
            let _ = sweeper::write_failed_anchor(
                &failed,
                &sweeper::FailedAnchor {
                    domain,
                    frame_digest: digest,
                    frame_hex: hex::encode(sealed.last().map(|f| f.to_bytes()).unwrap_or([0; 64])),
                    reason,
                    candid_raw,
                },
            );
        }
        verdict
    });

    let enqueue_started = Instant::now();
    for frame in &frames {
        sweeper::enqueue_frame(&tx, frame.to_bytes()).expect("enqueue");
    }
    drop(tx);
    let enqueue = enqueue_started.elapsed();
    println!(
        "  enqueued {} frames in {:.2?} ({:.1} ns/frame, sweeper hashes)",
        frames.len(),
        enqueue,
        enqueue.as_secs_f64() * 1.0e9 / frames.len() as f64
    );

    match sweeper.join().expect("sweeper") {
        OracleVerdict::Recorded { raw, .. } => println!("  recorded: {raw}"),
        OracleVerdict::Rejected { reason, .. } => {
            println!("  canister rejected: {reason}");
            println!("  digest queued under {}", sweeper::FAILED_ANCHORS_DIR);
        }
        OracleVerdict::Transport { reason } => {
            println!("  transport: {reason}");
            println!("  digest queued under {}", sweeper::FAILED_ANCHORS_DIR);
        }
    }
}

fn frames_hint(steps: u32) -> usize {
    steps as usize
}
