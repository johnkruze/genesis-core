//! Acceptance-layer sweeper.
//!
//! The physics thread enqueues a 64-byte frame. This thread seals the slot
//! proof, hashes the frame bytes (the `.soma.bin` digest at header offset 24),
//! and submits that digest under `SOMA_FRAME:{body_id}`. A domain the canister
//! does not list yet is still the domain we send.

use crate::last_state::{LastStateFrame64, SPEC_VERSION};
use crate::schema::trajectory_generated::kid_cosmo::{
    finish_trajectory_collection_buffer, ReasoningContext, ReasoningContextArgs, TelemetrySnapshot,
    TelemetrySnapshotArgs, Trajectory, TrajectoryArgs, TrajectoryCollection,
    TrajectoryCollectionArgs, TrajectoryScore, TrajectoryScoreArgs, Vector3,
};
use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const CANISTER_ID: &str = "ad7wi-4aaaa-aaaad-aeijq-cai";
pub const ORACLE_METHOD: &str = "record_trajectory_proof";
pub const FAILED_ANCHORS_DIR: &str = "failed_anchors";

/// Domain string for a last-state body. Body 10 and body 11 are sent as
/// `SOMA_FRAME:10` and `SOMA_FRAME:11` even when the deployed allowlist rejects them.
pub fn soma_frame_domain(body_id: u16) -> String {
    format!("SOMA_FRAME:{body_id}")
}

pub fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

pub fn frames_as_bytes(frames: &[LastStateFrame64]) -> &[u8] {
    const _: () = assert!(std::mem::size_of::<LastStateFrame64>() == 64);
    unsafe { std::slice::from_raw_parts(frames.as_ptr().cast::<u8>(), frames.len() * 64) }
}

/// SHA-256 of concatenated frame bytes. This is the header digest at offset 24.
pub fn frame_bytes_digest(frames: &[LastStateFrame64]) -> String {
    hex::encode(Sha256::digest(frames_as_bytes(frames)))
}

pub fn digest_frame_bytes(raw: &[u8]) -> String {
    hex::encode(Sha256::digest(raw))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OracleVerdict {
    Recorded { domain: String, hash: String, raw: String },
    Rejected { reason: String, raw: String },
    Transport { reason: String },
}

impl OracleVerdict {
    pub fn recorded(&self) -> bool {
        matches!(self, OracleVerdict::Recorded { .. })
    }

    pub fn anchors_locally(&self) -> bool {
        !self.recorded()
    }
}

/// Candid text from `dfx canister call`. Success is `variant { Ok = ... }`.
/// A rejection is `variant { Err = ... }`. A non-variant body is not a record.
pub fn interpret_candid(stdout: &str) -> OracleVerdict {
    let raw = stdout.trim().to_string();
    if let Some(payload) = candid_variant_payload(&raw, "Ok") {
        return OracleVerdict::Recorded {
            domain: String::new(),
            hash: String::new(),
            raw: payload,
        };
    }
    if let Some(payload) = candid_variant_payload(&raw, "Err") {
        return OracleVerdict::Rejected {
            reason: payload.clone(),
            raw: payload,
        };
    }
    OracleVerdict::Rejected {
        reason: "unrecognized candid reply".into(),
        raw,
    }
}

fn candid_variant_payload(text: &str, tag: &str) -> Option<String> {
    let marker = format!("variant {{ {tag}");
    let start = text.find(&marker)?;
    let after = &text[start + marker.len()..];
    let eq = after.find('=')?;
    let rest = after[eq + 1..].trim_start();
    if let Some(stripped) = rest.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = stripped.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
                continue;
            }
            if c == '"' {
                return Some(out);
            }
            out.push(c);
        }
        return None;
    }
    let end = rest.find('}').unwrap_or(rest.len());
    Some(rest[..end].trim().trim_end_matches(';').trim().to_string())
}

pub fn frame_channel(bound: usize) -> (Sender<[u8; 64]>, Receiver<[u8; 64]>) {
    bounded(bound)
}

/// Timed dump. Hashing is not in this function.
pub fn enqueue_frame(tx: &Sender<[u8; 64]>, frame: [u8; 64]) -> Result<Duration, TrySendError<[u8; 64]>> {
    let started = Instant::now();
    tx.try_send(frame)?;
    Ok(started.elapsed())
}

#[derive(Debug, Clone)]
pub struct FailedAnchor {
    pub domain: String,
    pub frame_digest: String,
    pub frame_hex: String,
    pub reason: String,
    pub candid_raw: String,
}

pub fn write_failed_anchor(dir: &Path, anchor: &FailedAnchor) -> std::io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("{stamp}-{}.anchor", &anchor.frame_digest[..16.min(anchor.frame_digest.len())]));
    let body = format!(
        "domain={}\nframe_digest={}\nframe_hex={}\nreason={}\ncandid={}\n",
        anchor.domain, anchor.frame_digest, anchor.frame_hex, anchor.reason, anchor.candid_raw
    );
    let mut file = fs::File::create(&path)?;
    file.write_all(body.as_bytes())?;
    Ok(path)
}

/// Seal `proof`, hash the 64 bytes, submit `SOMA_FRAME:{body_id}`.
/// The api_key argument is empty. Authorization is the caller principal on the canister.
/// Network, auth, and domain rejections are written under `failed_dir`.
pub fn anchor_frame(body_id: u16, raw: [u8; 64], failed_dir: &Path) -> OracleVerdict {
    let mut frame = LastStateFrame64::from_bytes(raw);
    frame.reseal();
    let sealed = frame.to_bytes();
    let digest = digest_frame_bytes(&sealed);
    let domain = soma_frame_domain(body_id);
    let verdict = record_trajectory_proof(&domain, &digest);
    if verdict.anchors_locally() {
        let (reason, candid_raw) = match &verdict {
            OracleVerdict::Rejected { reason, raw } => (reason.clone(), raw.clone()),
            OracleVerdict::Transport { reason } => (reason.clone(), String::new()),
            OracleVerdict::Recorded { .. } => unreachable!(),
        };
        let _ = write_failed_anchor(
            failed_dir,
            &FailedAnchor {
                domain: domain.clone(),
                frame_digest: digest,
                frame_hex: hex::encode(sealed),
                reason,
                candid_raw,
            },
        );
    }
    verdict
}

pub fn record_trajectory_proof(domain: &str, proof_hash: &str) -> OracleVerdict {
    if !is_sha256_hex(proof_hash) {
        return OracleVerdict::Rejected {
            reason: format!("proof_hash must be 64 lowercase hex chars (got {})", proof_hash.len()),
            raw: String::new(),
        };
    }
    let candid = format!("(\"\", \"{domain}\", \"{proof_hash}\")");
    let mut child = match Command::new("dfx")
        .args([
            "canister",
            "call",
            CANISTER_ID,
            ORACLE_METHOD,
            &candid,
            "--network",
            "ic",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            return OracleVerdict::Transport {
                reason: format!("dfx exec failed: {err}"),
            };
        }
    };

    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = String::new();
                let mut stderr = String::new();
                if let Some(mut out) = child.stdout.take() {
                    let _ = out.read_to_string(&mut stdout);
                }
                if let Some(mut err) = child.stderr.take() {
                    let _ = err.read_to_string(&mut stderr);
                }
                if !status.success() {
                    let code = status.code().unwrap_or(-1);
                    return OracleVerdict::Transport {
                        reason: format!("dfx exit {code}: {}", stderr.trim()),
                    };
                }
                return interpret_candid(&stdout);
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return OracleVerdict::Transport {
                        reason: "dfx timed out after 45s".into(),
                    };
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(err) => {
                return OracleVerdict::Transport {
                    reason: format!("dfx wait failed: {err}"),
                };
            }
        }
    }
}

/// Collection whose `trajectory_hash` is the sealed frame digest, not a hash of this buffer.
pub fn build_collection(
    body_id: u16,
    frame: &LastStateFrame64,
    reasoning: Option<(&str, bool)>,
) -> Vec<u8> {
    let mut sealed = *frame;
    sealed.reseal();
    let raw = sealed.to_bytes();
    let frame_digest = digest_frame_bytes(&raw);

    let mut fbb = flatbuffers::FlatBufferBuilder::with_capacity(512);
    let frame_vec = fbb.create_vector(&raw);
    let phase = fbb.create_string("last_state");
    let pos = Vector3::new(sealed.pos[0] as f64, sealed.pos[1] as f64, sealed.pos[2] as f64);
    let snapshot = TelemetrySnapshot::create(
        &mut fbb,
        &TelemetrySnapshotArgs {
            t: sealed.t,
            alt: sealed.pos[1] as f64,
            vel: sealed.vel[2] as f64,
            phase: Some(phase),
            pos: Some(&pos),
            rho: sealed.residual as f64,
        },
    );
    let reasoning_off = if let Some((text, is_anomaly)) = reasoning {
        let anomaly = fbb.create_string(text);
        Some(ReasoningContext::create(
            &mut fbb,
            &ReasoningContextArgs {
                is_anomaly,
                anomaly_type: Some(anomaly),
                snapshot: Some(snapshot),
            },
        ))
    } else {
        None
    };
    let id = fbb.create_string(&format!("body-{body_id}"));
    let type_name = fbb.create_string("last_state_frame64");
    let hash = fbb.create_string(&frame_digest);
    let score = TrajectoryScore::create(
        &mut fbb,
        &TrajectoryScoreArgs {
            mission_success: reasoning.map(|(_, flag)| !flag).unwrap_or(true),
            final_velocity: sealed.vel[2] as f64,
            dispersion: sealed.residual as f64,
        },
    );
    let data = fbb.create_vector(&[snapshot]);
    let trajectory = Trajectory::create(
        &mut fbb,
        &TrajectoryArgs {
            id: Some(id),
            type_: Some(type_name),
            odin_backtest: false,
            score: Some(score),
            trajectory_hash: Some(hash),
            reasoning_context: reasoning_off,
            data: Some(data),
            last_state_frame: Some(frame_vec),
            spec_version: SPEC_VERSION,
        },
    );
    let trajectories = fbb.create_vector(&[trajectory]);
    let document_type = fbb.create_string("TrajectoryCollection");
    let title = fbb.create_string("spectra-shared-mind");
    let author = fbb.create_string("genesis_core");
    let date = fbb.create_string("2026-09-24");
    let thesis = fbb.create_string("frame digest is trajectory_hash");
    let hardware = fbb.create_string("forge-cpu");
    let integrity = fbb.create_string(&frame_digest);
    let k_index = fbb.create_string(&soma_frame_domain(body_id));
    let collection = TrajectoryCollection::create(
        &mut fbb,
        &TrajectoryCollectionArgs {
            document_type: Some(document_type),
            title: Some(title),
            author: Some(author),
            date: Some(date),
            thesis_reference: Some(thesis),
            hardware: Some(hardware),
            integrity: Some(integrity),
            total_trajectories: 1,
            k_index: Some(k_index),
            domains: 1,
            trajectories: Some(trajectories),
        },
    );
    finish_trajectory_collection_buffer(&mut fbb, collection);
    fbb.finished_data().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::last_state::{LastStateFrame64, BODY_FUSION, BODY_HAND, BODY_PLASMA, FLAG_HAND_PAD_SLIP};
    use crate::schema::trajectory_generated::kid_cosmo::root_as_trajectory_collection;
    use crate::physics::dexterous::{
        evaluate_hand_tendon_dynamics, C_HandTendonState, N_HAND_FINGERS, THUMB_OPPOSITION_RAD,
    };
    use crate::physics::reactor::ReactorState;
    use std::time::Duration;

    #[test]
    fn candid_err_variant_is_rejection() {
        let stdout = r#"(variant { Err = "unknown attestation domain" })"#;
        let verdict = interpret_candid(stdout);
        assert!(!verdict.recorded());
        match verdict {
            OracleVerdict::Rejected { reason, .. } => {
                assert_eq!(reason, "unknown attestation domain");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn candid_ok_variant_is_recorded() {
        let stdout = r#"(variant { Ok = "recorded" })"#;
        assert!(interpret_candid(stdout).recorded());
    }

    #[test]
    fn json_error_string_is_not_treated_as_protocol() {
        let stdout = r#"("{\"error\":\"Unauthorized\"}")"#;
        let verdict = interpret_candid(stdout);
        assert!(!verdict.recorded());
        match verdict {
            OracleVerdict::Rejected { reason, .. } => {
                assert_eq!(reason, "unrecognized candid reply");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn domains_stay_on_the_body_id() {
        assert_eq!(soma_frame_domain(BODY_PLASMA), "SOMA_FRAME:10");
        assert_eq!(soma_frame_domain(BODY_FUSION), "SOMA_FRAME:11");
        assert_eq!(soma_frame_domain(BODY_HAND), "SOMA_FRAME:31");
    }

    #[test]
    fn trajectory_hash_is_the_frame_digest() {
        let frame = LastStateFrame64::pack_fusion(
            1.0, 1.0, 0.0065, 1.0, 0.0, 0.0, 0.0, 0.0, 0.2, true, false,
        );
        let buf = build_collection(BODY_FUSION, &frame, None);
        let root = root_as_trajectory_collection(&buf).unwrap();
        let traj = root.trajectories().unwrap().get(0);
        assert_eq!(traj.spec_version(), SPEC_VERSION);
        let stored = traj.last_state_frame().unwrap().bytes();
        assert_eq!(stored.len(), 64);
        let expected = digest_frame_bytes(stored);
        assert_eq!(traj.trajectory_hash().unwrap(), expected);
        assert_ne!(digest_frame_bytes(&buf), expected);
    }

    #[test]
    fn enqueue_is_under_ten_microseconds() {
        let (tx, rx) = frame_channel(8);
        let frame = [0xABu8; 64];
        let mut best = Duration::from_secs(1);
        for _ in 0..32 {
            let dt = enqueue_frame(&tx, frame).unwrap();
            best = best.min(dt);
            let _ = rx.try_recv().unwrap();
        }
        assert!(
            best < Duration::from_micros(10),
            "enqueue took {best:?}"
        );
    }

    #[test]
    fn rejection_is_written_to_disk() {
        let dir = std::env::temp_dir().join(format!(
            "failed_anchors-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let anchor = FailedAnchor {
            domain: "SOMA_FRAME:10".into(),
            frame_digest: "ab".repeat(32),
            frame_hex: "00".repeat(64),
            reason: "dfx exit 1".into(),
            candid_raw: String::new(),
        };
        let path = write_failed_anchor(&dir, &anchor).unwrap();
        let text = fs::read_to_string(path).unwrap();
        assert!(text.contains("SOMA_FRAME:10"));
        assert!(text.contains(&anchor.frame_digest));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hand_slip_then_channel() {
        let mut state = C_HandTendonState {
            q_mcp: [0.05; N_HAND_FINGERS],
            q_pip: [0.04; N_HAND_FINGERS],
            q_dip: [0.03; N_HAND_FINGERS],
            qdot_mcp: [0.0; N_HAND_FINGERS],
            qdot_pip: [0.0; N_HAND_FINGERS],
            qdot_dip: [0.0; N_HAND_FINGERS],
            tendon_stretch_m: 0.0,
            tendon_tension_n: 0.0,
            opposition_rad: THUMB_OPPOSITION_RAD,
            object_span_m: 0.072,
            commanded_close_rad: 1.45,
            pad_normal_n: 0.0,
            normal_force: 0.0,
            slip_velocity: 0.0,
            slip_angular_velocity: 0.0,
            object_mass: 0.12,
            static_friction_coeff: 0.85,
            dynamic_friction_coeff: 0.68,
            reflex_active: false,
        };
        for _ in 0..40 {
            assert!(!evaluate_hand_tendon_dynamics(&mut state, 0.001).pad_slip);
        }
        state.object_span_m = 0.022;
        state.commanded_close_rad = 0.42;
        state.object_mass = 2.4;
        state.static_friction_coeff = 0.11;
        state.dynamic_friction_coeff = 0.09;
        state.slip_velocity = 0.0;
        let (tx, rx) = frame_channel(4);
        let mut sent = false;
        for step in 0..100 {
            let result = evaluate_hand_tendon_dynamics(&mut state, 0.001);
            if result.pad_slip {
                let frame = LastStateFrame64 {
                    t: step as f64 * 0.001,
                    pos: [result.tendon_tension_n, result.pad_normal_n, result.stretch_m],
                    vel: [state.opposition_rad, state.q_mcp[1], state.slip_velocity],
                    force_torque: result.margin,
                    residual: state.object_span_m,
                    flags: FLAG_HAND_PAD_SLIP,
                    proof: [0; 16],
                };
                assert!(enqueue_frame(&tx, frame.to_bytes()).unwrap() < Duration::from_micros(10));
                sent = true;
                break;
            }
        }
        assert!(sent);
        let raw = rx.recv().unwrap();
        assert_eq!(raw.len(), 64);
    }

    #[test]
    fn reactor_crosses_prompt_on_microsecond_steps() {
        let mut reactor = ReactorState::new();
        let dt = 1e-6;
        let rod_per_s = 8.0;
        let mut steps = 0u32;
        let mut crossed = false;
        while steps < 50_000 {
            reactor.pull_control_rods(rod_per_s * dt);
            reactor.step(dt);
            steps += 1;
            if reactor.prompt_critical {
                crossed = true;
                break;
            }
        }
        assert!(crossed, "did not cross in {steps} steps of {dt} s");
        assert!(steps > 100, "crossed in {steps} steps; rod pull was not integrated");
        assert!((reactor.time_s - steps as f64 * dt).abs() < 1e-9);
    }
}
