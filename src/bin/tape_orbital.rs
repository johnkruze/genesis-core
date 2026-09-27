//! Stardust reentry telemetry tape.
//!
//! Explicit Euler at dt = 0.001 s (`dt = 0.001 s`) for the Forge Stardust initial
//! condition: mass 45.8 kg, diameter 0.811 m, Cd 1.65, nose radius 0.23 m,
//! entry interface 120 km, speed 12900 m/s, EFPA −8.2 deg. Atmosphere, drag,
//! and Sutton–Graves heating match `orbital_plasma_discovery_mc`.
//!
//! Frames are written every 0.1 s of simulation time, plus the terminal step
//! if it falls off that grid. Each written frame is one node in a hash chain
//! (a one-parent Merkle DAG):
//!
//! ```text
//! preimage = compact JSON of the frame with current_hash omitted
//! || previous_hash (raw hex UTF-8)
//! current_hash = hex(SHA-256(preimage))
//! ```
//!
//! The genesis parent is 64 zero hex digits. Output is one JSON object per
//! line at `data/orbital_reentry_tape.jsonl`.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;

const MASS_KG: f64 = 45.8;
const DIAMETER_M: f64 = 0.811;
const CD: f64 = 1.65;
const R_NOSE_M: f64 = 0.23;
const V_ENTRY_M_S: f64 = 12_900.0;
const EFPA_DEG: f64 = -8.2;
const ALT_ENTRY_M: f64 = 120_000.0;

const H_SCALE_M: f64 = 7_400.0;
const RHO_SL: f64 = 1.225;
const G0: f64 = 9.81;
const R_EARTH_M: f64 = 6_371_000.0;
const SUTTON_GRAVES_K: f64 = 1.7415e-4;

const DT_S: f64 = 0.001;
const SAMPLE_STRIDE: u64 = 100;
const V_HALT_M_S: f64 = 80.0;
const MAX_TIME_S: f64 = 1_800.0;

const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StateFrame {
    time: f64,
    altitude: f64,
    velocity: f64,
    g_force: f64,
    heat_flux: f64,
    previous_hash: String,
    current_hash: String,
}

/// Body committed to the digest. `current_hash` is excluded; `previous_hash` is included.
#[derive(Serialize)]
struct FramePreimage<'a> {
    time: f64,
    altitude: f64,
    velocity: f64,
    g_force: f64,
    heat_flux: f64,
    previous_hash: &'a str,
}

fn tape_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/orbital_reentry_tape.jsonl")
}

fn reference_area_m2() -> f64 {
    std::f64::consts::PI * (DIAMETER_M * 0.5).powi(2)
}

fn atm_density(alt_m: f64) -> f64 {
    if alt_m < 0.0 {
        return RHO_SL;
    }
    RHO_SL * (-alt_m / H_SCALE_M).exp()
}

/// Sutton–Graves convective heat flux, W/m².
fn sutton_graves_heat_flux(rho: f64, velocity: f64) -> f64 {
    SUTTON_GRAVES_K * (rho / R_NOSE_M).sqrt() * velocity.powi(3)
}

fn observables(altitude_m: f64, velocity_m_s: f64) -> (f64, f64) {
    let rho = atm_density(altitude_m);
    let q_dyn = 0.5 * rho * velocity_m_s * velocity_m_s;
    let drag_accel = (CD * reference_area_m2() * q_dyn) / MASS_KG;
    let g_force = drag_accel / G0;
    let heat_flux = sutton_graves_heat_flux(rho, velocity_m_s);
    (g_force, heat_flux)
}

fn frame_hash(preimage_json: &[u8], previous_hash: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(preimage_json);
    hasher.update(previous_hash.as_bytes());
    hex::encode(hasher.finalize())
}

fn seal_frame(
    time: f64,
    altitude: f64,
    velocity: f64,
    previous_hash: &str,
) -> Result<StateFrame, Box<dyn std::error::Error>> {
    let (g_force, heat_flux) = observables(altitude, velocity);
    let preimage = FramePreimage {
        time,
        altitude,
        velocity,
        g_force,
        heat_flux,
        previous_hash,
    };
    let preimage_json = serde_json::to_vec(&preimage)?;
    let current_hash = frame_hash(&preimage_json, previous_hash);
    Ok(StateFrame {
        time,
        altitude,
        velocity,
        g_force,
        heat_flux,
        previous_hash: previous_hash.to_string(),
        current_hash,
    })
}

struct Trajectory {
    frames: Vec<StateFrame>,
    steps: u64,
    peak_g: f64,
    peak_heat_flux: f64,
    halt: &'static str,
}

fn integrate() -> Result<Trajectory, Box<dyn std::error::Error>> {
    let gamma_entry = EFPA_DEG * std::f64::consts::PI / 180.0;
    let mut altitude = ALT_ENTRY_M;
    let mut velocity = V_ENTRY_M_S;
    let mut v_vertical = velocity * gamma_entry.sin();
    let mut v_horizontal = velocity * gamma_entry.cos();

    let mut frames = Vec::new();
    let mut previous_hash = GENESIS_HASH.to_string();
    let opening = seal_frame(0.0, altitude, velocity, &previous_hash)?;
    previous_hash = opening.current_hash.clone();
    frames.push(opening);

    let mut peak_g = 0.0_f64;
    let mut peak_heat_flux = 0.0_f64;
    let max_steps = (MAX_TIME_S / DT_S) as u64;
    let area = reference_area_m2();
    let mut halt = "MAX_TIME_S";

    for step in 1..=max_steps {
        let rho = atm_density(altitude);
        let q_dyn = 0.5 * rho * velocity * velocity;
        let drag_accel = (CD * area * q_dyn) / MASS_KG;
        let (g_force, heat_flux) = {
            let g = drag_accel / G0;
            (g, sutton_graves_heat_flux(rho, velocity))
        };
        peak_g = peak_g.max(g_force);
        peak_heat_flux = peak_heat_flux.max(heat_flux);

        let gamma = if velocity > 1.0 {
            (v_vertical / velocity).clamp(-1.0, 1.0).asin()
        } else {
            -std::f64::consts::FRAC_PI_2
        };

        v_horizontal -= drag_accel * gamma.cos() * DT_S;
        v_horizontal = v_horizontal.max(0.0);

        let centrifugal = (v_horizontal * v_horizontal) / (R_EARTH_M + altitude);
        v_vertical += (-drag_accel * gamma.sin() - G0 + centrifugal) * DT_S;

        velocity = (v_horizontal * v_horizontal + v_vertical * v_vertical).sqrt();
        altitude += v_vertical * DT_S;

        let timed_out = step == max_steps;
        let stopped = altitude < 0.0 || velocity < V_HALT_M_S || timed_out;
        if step % SAMPLE_STRIDE == 0 || stopped {
            let time = step as f64 / 1000.0;
            let frame = seal_frame(time, altitude, velocity, &previous_hash)?;
            previous_hash = frame.current_hash.clone();
            frames.push(frame);
        }

        if altitude < 0.0 {
            halt = "altitude < 0";
            break;
        }
        if velocity < V_HALT_M_S {
            halt = "speed < 80 m/s";
            break;
        }
        if timed_out {
            break;
        }
    }

    if halt == "MAX_TIME_S" {
        return Err("integration hit the time cap before altitude < 0 or speed < 80 m/s".into());
    }

    let steps = frames
        .last()
        .map(|f| (f.time * 1000.0).round() as u64)
        .unwrap_or(0);

    Ok(Trajectory {
        frames,
        steps,
        peak_g,
        peak_heat_flux,
        halt,
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
            || !frame.altitude.is_finite()
            || !frame.velocity.is_finite()
            || !frame.g_force.is_finite()
            || !frame.heat_flux.is_finite()
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

fn main() {
    if let Err(err) = run() {
        eprintln!("tape_orbital: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let trajectory = integrate()?;
    let path = tape_path();
    write_tape(&path, &trajectory.frames)?;
    let verified = verify_tape(&path)?;

    let last = trajectory.frames.last().expect("tape has a terminal frame");
    let tip = &last.current_hash;
    println!("Stardust reentry tape");
    println!(" halt: {}", trajectory.halt);
    println!(" sim_time_s: {:.3}", last.time);
    println!(" steps: {}", trajectory.steps);
    println!(" frames: {verified}");
    println!(" final_alt_m: {:.3}", last.altitude);
    println!(" final_vel_m_s: {:.6}", last.velocity);
    println!(" peak_g: {:.3}", trajectory.peak_g);
    println!(" peak_q_W_m2: {:.3}", trajectory.peak_heat_flux);
    println!(" tip: {tip}");
    println!(" path: {}", path.display());
    Ok(())
}
