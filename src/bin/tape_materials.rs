//! Ti-6Al-4V uniaxial load-path telemetry tape.
//!
//! Prescribed-strain tension at dt = 0.001 s (`dt = 0.001 s`). Engineering strain
//! advances at a constant rate of `10^-3 / s` until fracture at strain 0.15.
//! The alloy is elastic–perfectly plastic:
//!
//! ```text
//! E = 114 GPa
//! σ_y = 830 MPa
//! ε_y = σ_y / E
//!
//! ε < ε_y: σ = E ε, ε_p = 0, yielded = false
//! ε ≥ ε_y: σ = σ_y, ε_p = ε - ε_y, yielded = true
//! ```
//!
//! Stress is stored in pascals. Frames are written every 0.1 s, at yield
//! onset, and at fracture. Each written frame is one node in a hash chain
//! (a one-parent Merkle DAG), the same construction as the orbital tape:
//!
//! ```text
//! preimage = compact JSON of the frame with current_hash omitted
//! || previous_hash (raw hex UTF-8)
//! current_hash = hex(SHA-256(preimage))
//! ```
//!
//! The genesis parent is 64 zero hex digits. Output is one JSON object per
//! line at `data/materials_load_tape.jsonl`.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;

/// Ti-6Al-4V Young's modulus.
const YOUNGS_MODULUS_PA: f64 = 114.0e9;
/// Ti-6Al-4V yield strength.
const YIELD_STRESS_PA: f64 = 830.0e6;
/// Engineering strain at fracture.
const FRACTURE_STRAIN: f64 = 0.15;
/// Quasi-static tensile strain rate, 1/s.
const STRAIN_RATE_PER_S: f64 = 1.0e-3;

const DT_S: f64 = 0.001;
const SAMPLE_STRIDE: u64 = 100;

const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StateFrame {
    time: f64,
    strain: f64,
    stress: f64,
    plastic_deformation: f64,
    is_yielded: bool,
    previous_hash: String,
    current_hash: String,
}

/// Body committed to the digest. `current_hash` is excluded; `previous_hash` is included.
#[derive(Serialize)]
struct FramePreimage<'a> {
    time: f64,
    strain: f64,
    stress: f64,
    plastic_deformation: f64,
    is_yielded: bool,
    previous_hash: &'a str,
}

fn tape_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/materials_load_tape.jsonl")
}

fn yield_strain() -> f64 {
    YIELD_STRESS_PA / YOUNGS_MODULUS_PA
}

/// Uniaxial elastic–perfectly plastic response.
/// Returns `(stress_Pa, plastic_strain, is_yielded)`.
fn constitutive(strain: f64) -> (f64, f64, bool) {
    let ey = yield_strain();
    if strain < ey {
        (YOUNGS_MODULUS_PA * strain, 0.0, false)
    } else {
        (YIELD_STRESS_PA, strain - ey, true)
    }
}

fn frame_hash(preimage_json: &[u8], previous_hash: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(preimage_json);
    hasher.update(previous_hash.as_bytes());
    hex::encode(hasher.finalize())
}

fn seal_frame(
    time: f64,
    strain: f64,
    previous_hash: &str,
) -> Result<StateFrame, Box<dyn std::error::Error>> {
    let (stress, plastic_deformation, is_yielded) = constitutive(strain);
    let preimage = FramePreimage {
        time,
        strain,
        stress,
        plastic_deformation,
        is_yielded,
        previous_hash,
    };
    let preimage_json = serde_json::to_vec(&preimage)?;
    let current_hash = frame_hash(&preimage_json, previous_hash);
    Ok(StateFrame {
        time,
        strain,
        stress,
        plastic_deformation,
        is_yielded,
        previous_hash: previous_hash.to_string(),
        current_hash,
    })
}

struct LoadPath {
    frames: Vec<StateFrame>,
    steps: u64,
    halt: &'static str,
}

fn integrate() -> Result<LoadPath, Box<dyn std::error::Error>> {
    let t_fracture = FRACTURE_STRAIN / STRAIN_RATE_PER_S;
    let n_steps = (t_fracture / DT_S).round() as u64;
    if n_steps == 0 {
        return Err("fracture time is shorter than one step".into());
    }

    let ey = yield_strain();
    let t_yield = ey / STRAIN_RATE_PER_S;

    let mut events: Vec<(f64, f64)> = Vec::new();
    events.push((0.0, 0.0));

    let mut crossed_yield = false;
    for step in 1..=n_steps {
        let terminal = step == n_steps;
        let time = if terminal {
            t_fracture
        } else {
            step as f64 * DT_S
        };
        let strain = if terminal {
            FRACTURE_STRAIN
        } else {
            STRAIN_RATE_PER_S * time
        };

        if !crossed_yield && strain >= ey {
            events.push((t_yield, ey));
            crossed_yield = true;
        }
        if step % SAMPLE_STRIDE == 0 || terminal {
            events.push((time, strain));
        }
    }

    if !crossed_yield {
        return Err("strain reached fracture without crossing yield".into());
    }

    events.sort_by(|a, b| a.0.partial_cmp(&b.0).expect("sample times are ordered"));
    let mut unique: Vec<(f64, f64)> = Vec::with_capacity(events.len());
    for event in events {
        if let Some((prev_t, _)) = unique.last() {
            if (event.0 - prev_t).abs() <= 1e-9 {
                continue;
            }
        }
        unique.push(event);
    }

    let mut frames = Vec::with_capacity(unique.len());
    let mut previous_hash = GENESIS_HASH.to_string();
    for (time, strain) in unique {
        let frame = seal_frame(time, strain, &previous_hash)?;
        previous_hash = frame.current_hash.clone();
        frames.push(frame);
    }

    Ok(LoadPath {
        frames,
        steps: n_steps,
        halt: "strain >= 0.15",
    })
}

fn write_tape(
    path: &std::path::Path,
    frames: &[StateFrame],
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);
    for frame in frames {
        serde_json::to_writer(&mut writer, frame)?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;
    Ok(())
}

fn verify_tape(path: &std::path::Path) -> Result<usize, Box<dyn std::error::Error>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut count = 0_usize;
    let mut prev_current: Option<String> = None;

    for (index, line) in reader.lines().enumerate() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        let frame: StateFrame = serde_json::from_str(&line)?;
        if !frame.time.is_finite()
            || !frame.strain.is_finite()
            || !frame.stress.is_finite()
            || !frame.plastic_deformation.is_finite()
        {
            return Err(format!("frame {index} contains a non-finite state").into());
        }

        let expected_prev = prev_current.as_deref().unwrap_or(GENESIS_HASH);
        if frame.previous_hash != expected_prev {
            return Err(format!(
                "frame {index} previous_hash does not match the preceding current_hash"
            )
            .into());
        }

        // Hash the exact JSON bytes on disk, not a re-serialized parse.
        // serde_json float text does not round-trip, so verification closes the brace
        // that `current_hash` displaced.
        const MARK: &str = ",\"current_hash\":\"";
        let Some(idx) = line.rfind(MARK) else {
            return Err(format!("frame {index} is missing current_hash").into());
        };
        let mut preimage = line[..idx].as_bytes().to_vec();
        preimage.push(b'}');
        let recomputed = frame_hash(&preimage, &frame.previous_hash);
        if recomputed != frame.current_hash {
            return Err(format!("frame {index} current_hash does not match the preimage").into());
        }

        prev_current = Some(frame.current_hash);
        count += 1;
    }

    if count < 2 {
        return Err("tape has fewer than two frames".into());
    }
    Ok(count)
}

/// The elastic branch is Hooke's law up to 830 MPa. Past yield, stress stays
/// on that surface and further strain is plastic.
fn audit_load_path(frames: &[StateFrame]) -> Result<(), Box<dyn std::error::Error>> {
    if frames.len() < 2 {
        return Err("load path has fewer than two frames".into());
    }

    let ey = yield_strain();
    let mut saw_elastic = false;
    let mut saw_plastic = false;
    let mut prev_time = -1.0_f64;
    let mut prev_elastic_stress = -1.0_f64;

    for (index, frame) in frames.iter().enumerate() {
        if frame.time + 1e-12 < prev_time {
            return Err(format!("frame {index} time is not monotonic").into());
        }
        prev_time = frame.time;

        if frame.strain < 0.0 || frame.stress < 0.0 || frame.plastic_deformation < -1e-15 {
            return Err(format!("frame {index} has a negative mechanical state").into());
        }

        let (expect_stress, expect_plastic, expect_yielded) = constitutive(frame.strain);
        let stress_tol = 1e-6_f64 * expect_stress.abs().max(1.0);
        if (frame.stress - expect_stress).abs() > stress_tol
            || (frame.plastic_deformation - expect_plastic).abs() > 1e-12
            || frame.is_yielded != expect_yielded
        {
            return Err(format!("frame {index} disagrees with the constitutive law").into());
        }

        if frame.strain < ey {
            saw_elastic = true;
            if frame.is_yielded || frame.plastic_deformation != 0.0 {
                return Err(format!("frame {index} is plastic inside the elastic range").into());
            }
            if frame.strain > 0.0 {
                let modulus = frame.stress / frame.strain;
                let rel = (modulus - YOUNGS_MODULUS_PA).abs() / YOUNGS_MODULUS_PA;
                if rel > 1e-9 {
                    return Err(format!("frame {index} is not linear with Young's modulus").into());
                }
            }
            if frame.stress + 1e-6 < prev_elastic_stress {
                return Err(format!("frame {index} elastic stress decreased").into());
            }
            if frame.stress > YIELD_STRESS_PA + 1.0 {
                return Err(format!("frame {index} elastic stress exceeded yield").into());
            }
            prev_elastic_stress = frame.stress;
        } else {
            saw_plastic = true;
            if !frame.is_yielded {
                return Err(format!("frame {index} passed yield without yielding").into());
            }
            if (frame.stress - YIELD_STRESS_PA).abs() > 1e-3 {
                return Err(format!(
                    "frame {index} plastic stress is not the 830 MPa yield surface"
                )
                .into());
            }
            let expect_plastic = frame.strain - ey;
            if (frame.plastic_deformation - expect_plastic).abs() > 1e-12 {
                return Err(format!("frame {index} plastic strain is not ε - ε_y").into());
            }
        }
    }

    let last = frames.last().expect("tape has a terminal frame");
    if (last.strain - FRACTURE_STRAIN).abs() > 1e-12 {
        return Err(format!("terminal strain is {}, fracture is 0.15", last.strain).into());
    }
    if !last.is_yielded || last.plastic_deformation <= 0.0 {
        return Err("fracture frame is not in plastic deformation".into());
    }
    if !saw_elastic || !saw_plastic {
        return Err("tape is missing either the elastic branch or the plastic branch".into());
    }
    Ok(())
}

fn main() {
    if let Err(err) = run() {
        eprintln!("tape_materials: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let path_sim = integrate()?;
    audit_load_path(&path_sim.frames)?;
    let path = tape_path();
    write_tape(&path, &path_sim.frames)?;
    let verified = verify_tape(&path)?;

    let last = path_sim.frames.last().expect("tape has a terminal frame");
    let tip = &last.current_hash;
    let yield_mpa = YIELD_STRESS_PA / 1.0e6;
    println!("Ti-6Al-4V load tape");
    println!(" halt: {}", path_sim.halt);
    println!(" youngs_modulus: {:.0} GPa", YOUNGS_MODULUS_PA / 1.0e9);
    println!(" yield_stress: {yield_mpa:.0} MPa");
    println!(" sim_time_s: {:.6}", last.time);
    println!(" steps: {}", path_sim.steps);
    println!(" frames: {verified}");
    println!(" final_strain: {:.6}", last.strain);
    println!(" final_stress_Pa: {:.3}", last.stress);
    println!(" final_stress_MPa: {:.3}", last.stress / 1.0e6);
    println!(" plastic_strain: {:.6}", last.plastic_deformation);
    println!(" yielded: {}", last.is_yielded);
    println!(" tip: {tip}");
    println!(" path: {}", path.display());
    Ok(())
}
