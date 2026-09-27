//! Bioreactor batch telemetry tape.
//!
//! One 72 h aerobic batch in a 1 m³ vessel. Biomass, substrate, and product
//! advance under classical RK4. Dissolved oxygen is the quasi-steady root of
//! the two-film balance, because the oxygen time constant is much faster than
//! growth. Each written frame is one node in a hash chain (a one-parent Merkle
//! DAG), the same construction as the orbital and materials tapes:
//!
//! ```text
//! preimage = compact JSON of the frame with current_hash omitted
//!          || previous_hash (raw hex UTF-8)
//! current_hash = hex(SHA-256(preimage))
//! ```
//!
//! The genesis parent is 64 zero hex digits. Output is one JSON object per
//! line at `data/bio_reactor_tape.jsonl`.
//!
//! State, with time in hours and concentrations in g/L:
//!
//! ```text
//! μ_eff = μ_max · S/(Ks+S) · CL/(Ko+CL) · f_τ
//! f_τ   = 1 / (1 + (τ / τ_crit)^2)
//! dX/dt = μ_eff X
//! dS/dt = -(μ_eff / Y_xs + m_s) X
//! dP/dt = (α μ_eff + β) X f_τ
//!
//! μ_broth = μ0 exp(0.21 X)
//! τ       = μ_broth · π · N / 0.1          impeller-tip shear, Pa
//! kLa     = van 't Riet (P/V, v_s) · (μ_broth/μ0)^(-0.45)
//! OTR     = kLa (C* - CL)                   g/L/h
//! CL      = dissolved oxygen solving OTR = OUR
//! ```
//!
//! Viscosity thickens with biomass, so the same growth both raises tip shear
//! and lowers kLa. Oxygen demand then bends μ_eff off the exponential, and
//! the shear factor trims product formation. That is the batch curve.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;

const VOLUME_M3: f64 = 1.0;
const ASPECT_H_OVER_T: f64 = 2.0;
const RPM: f64 = 280.0;
/// Rushton power number.
const POWER_NUMBER: f64 = 5.0;
const RHO_KG_M3: f64 = 1020.0;
/// Superficial sparge velocity, m/s.
const SUPERFICIAL_VELOCITY_M_S: f64 = 0.025;
/// Inoculum viscosity, Pa·s.
const MU0_PA_S: f64 = 0.0012;
const VISCOSITY_EXP_PER_G_L: f64 = 0.21;
/// Forge critical tip shear for this culture class.
const TAU_CRIT_PA: f64 = 45.0;
const SHEAR_HILL: f64 = 2.0;
/// Air saturation, g/L.
const C_STAR_G_L: f64 = 0.0076;

const MU_MAX_PER_H: f64 = 0.48;
const KS_G_L: f64 = 0.35;
const KO_G_L: f64 = 0.00025;
const Y_XS: f64 = 0.50;
const MAINTENANCE_S_PER_H: f64 = 0.012;
const Y_OX: f64 = 1.15;
const MAINTENANCE_O_PER_H: f64 = 0.006;
/// Luedeking–Piret growth-associated yield, g product / g biomass.
const ALPHA_P_X: f64 = 0.42;
/// Non-growth product formation, 1/h.
const BETA_PER_H: f64 = 0.004;

const X0_G_L: f64 = 0.12;
const S0_G_L: f64 = 50.0;

/// RK4 substeps per hour. 100 × 0.01 h.
const SUBSTEPS_PER_HOUR: u64 = 100;
const SAMPLE_STRIDE: u64 = 10;
const BATCH_HOURS: u64 = 72;

/// van 't Riet prefactor, giving kLa in 1/s for P/V in W/m³ and v_s in m/s.
const VANT_RIET_PREFACTOR: f64 = 0.026;
const VANT_RIET_POWER_EXP: f64 = 0.4;
const VANT_RIET_GAS_EXP: f64 = 0.5;
const VISCOSITY_KLA_EXP: f64 = -0.45;
const KLA_MIN_PER_H: f64 = 8.0;
const KLA_MAX_PER_H: f64 = 650.0;

const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StateFrame {
    time: f64,
    biomass_concentration: f64,
    substrate_concentration: f64,
    oxygen_transfer_rate: f64,
    shear_stress: f64,
    product_yield: f64,
    previous_hash: String,
    current_hash: String,
}

/// Body committed to the digest. `current_hash` is excluded; `previous_hash` is included.
#[derive(Serialize)]
struct FramePreimage<'a> {
    time: f64,
    biomass_concentration: f64,
    substrate_concentration: f64,
    oxygen_transfer_rate: f64,
    shear_stress: f64,
    product_yield: f64,
    previous_hash: &'a str,
}

#[derive(Clone, Copy)]
struct Culture {
    biomass: f64,
    substrate: f64,
    product: f64,
}

struct Resolved {
    mu_per_h: f64,
    oxygen_transfer_rate: f64,
    shear_stress: f64,
    shear_factor: f64,
}

fn tape_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/bio_reactor_tape.jsonl")
}

fn tank_diameter_m() -> f64 {
    // V = (π/4) T² H, H = aspect · T.
    (4.0 * VOLUME_M3 / (std::f64::consts::PI * ASPECT_H_OVER_T)).cbrt()
}

fn impeller_diameter_m() -> f64 {
    tank_diameter_m() / 3.0
}

fn agitation_rps() -> f64 {
    RPM / 60.0
}

fn broth_viscosity_pa_s(biomass: f64) -> f64 {
    MU0_PA_S * (VISCOSITY_EXP_PER_G_L * biomass.max(0.0)).exp()
}

fn tip_shear_pa(biomass: f64) -> f64 {
    // γ = tip_speed / (0.1 D) = π N / 0.1.
    let gamma = std::f64::consts::PI * agitation_rps() / 0.1;
    broth_viscosity_pa_s(biomass) * gamma
}

fn kla_per_h(biomass: f64) -> f64 {
    let n = agitation_rps();
    let diameter = impeller_diameter_m();
    let power_w = POWER_NUMBER * RHO_KG_M3 * n.powi(3) * diameter.powi(5);
    let power_per_volume = power_w / VOLUME_M3;
    let viscosity_ratio = broth_viscosity_pa_s(biomass) / MU0_PA_S;
    let kla = VANT_RIET_PREFACTOR
        * power_per_volume.powf(VANT_RIET_POWER_EXP)
        * SUPERFICIAL_VELOCITY_M_S.powf(VANT_RIET_GAS_EXP)
        * 3600.0
        * viscosity_ratio.powf(VISCOSITY_KLA_EXP);
    kla.clamp(KLA_MIN_PER_H, KLA_MAX_PER_H)
}

/// Quasi-steady dissolved oxygen: OTR(CL) = OUR(CL).
fn resolve(biomass: f64, substrate: f64) -> Resolved {
    let x = biomass.max(0.0);
    let s = substrate.max(0.0);
    let shear_stress = tip_shear_pa(x);
    let shear_factor = 1.0 / (1.0 + (shear_stress / TAU_CRIT_PA).powf(SHEAR_HILL));
    let kla = kla_per_h(x);
    let monod_s = if s <= 0.0 { 0.0 } else { s / (KS_G_L + s) };
    let growth_capacity = MU_MAX_PER_H * monod_s * shear_factor;

    if x <= 0.0 {
        return Resolved {
            mu_per_h: 0.0,
            oxygen_transfer_rate: 0.0,
            shear_stress,
            shear_factor,
        };
    }

    // G is the oxygen surplus at CL = 0 after maintenance. R scales the
    // growth-associated oxygen demand.
    let surplus = kla * C_STAR_G_L - MAINTENANCE_O_PER_H * x;
    if surplus <= 0.0 {
        return Resolved {
            mu_per_h: 0.0,
            oxygen_transfer_rate: kla * C_STAR_G_L,
            shear_stress,
            shear_factor,
        };
    }

    let demand = growth_capacity * x / Y_OX;
    let linear = demand + kla * KO_G_L - surplus;
    let discriminant = linear * linear + 4.0 * kla * surplus * KO_G_L;
    let dissolved = ((-linear + discriminant.max(0.0).sqrt()) / (2.0 * kla)).clamp(0.0, C_STAR_G_L);
    let monod_o = if dissolved <= 0.0 {
        0.0
    } else {
        dissolved / (KO_G_L + dissolved)
    };
    let mu_per_h = growth_capacity * monod_o;
    Resolved {
        mu_per_h,
        oxygen_transfer_rate: kla * (C_STAR_G_L - dissolved),
        shear_stress,
        shear_factor,
    }
}

fn rates(culture: Culture) -> (f64, f64, f64) {
    let culture = Culture {
        biomass: culture.biomass.max(0.0),
        substrate: culture.substrate.max(0.0),
        product: culture.product.max(0.0),
    };
    let state = resolve(culture.biomass, culture.substrate);
    let d_biomass = state.mu_per_h * culture.biomass;
    let d_substrate = if culture.substrate <= 0.0 {
        0.0
    } else {
        -(state.mu_per_h / Y_XS + MAINTENANCE_S_PER_H) * culture.biomass
    };
    let d_product =
        (ALPHA_P_X * state.mu_per_h + BETA_PER_H) * culture.biomass * state.shear_factor;
    (d_biomass, d_substrate, d_product)
}

fn rk4_step(culture: Culture, dt_h: f64) -> Culture {
    let k1 = rates(culture);
    let k2 = rates(shift(culture, k1, 0.5 * dt_h));
    let k3 = rates(shift(culture, k2, 0.5 * dt_h));
    let k4 = rates(shift(culture, k3, dt_h));
    Culture {
        biomass: (culture.biomass + dt_h / 6.0 * (k1.0 + 2.0 * k2.0 + 2.0 * k3.0 + k4.0)).max(0.0),
        substrate: (culture.substrate + dt_h / 6.0 * (k1.1 + 2.0 * k2.1 + 2.0 * k3.1 + k4.1)).max(0.0),
        product: (culture.product + dt_h / 6.0 * (k1.2 + 2.0 * k2.2 + 2.0 * k3.2 + k4.2)).max(0.0),
    }
}

fn shift(culture: Culture, rate: (f64, f64, f64), h: f64) -> Culture {
    Culture {
        biomass: (culture.biomass + h * rate.0).max(0.0),
        substrate: (culture.substrate + h * rate.1).max(0.0),
        product: (culture.product + h * rate.2).max(0.0),
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
    culture: Culture,
    previous_hash: &str,
) -> Result<StateFrame, Box<dyn std::error::Error>> {
    let state = resolve(culture.biomass, culture.substrate);
    let preimage = FramePreimage {
        time,
        biomass_concentration: culture.biomass,
        substrate_concentration: culture.substrate,
        oxygen_transfer_rate: state.oxygen_transfer_rate,
        shear_stress: state.shear_stress,
        product_yield: culture.product,
        previous_hash,
    };
    let preimage_json = serde_json::to_vec(&preimage)?;
    let current_hash = frame_hash(&preimage_json, previous_hash);
    Ok(StateFrame {
        time,
        biomass_concentration: culture.biomass,
        substrate_concentration: culture.substrate,
        oxygen_transfer_rate: state.oxygen_transfer_rate,
        shear_stress: state.shear_stress,
        product_yield: culture.product,
        previous_hash: previous_hash.to_string(),
        current_hash,
    })
}

struct Batch {
    frames: Vec<StateFrame>,
    steps: u64,
}

fn integrate() -> Result<Batch, Box<dyn std::error::Error>> {
    let dt_h = 1.0 / SUBSTEPS_PER_HOUR as f64;
    let mut culture = Culture {
        biomass: X0_G_L,
        substrate: S0_G_L,
        product: 0.0,
    };
    let total_steps = BATCH_HOURS * SUBSTEPS_PER_HOUR;

    let mut frames = Vec::new();
    let mut previous_hash = GENESIS_HASH.to_string();
    let opening = seal_frame(0.0, culture, &previous_hash)?;
    previous_hash = opening.current_hash.clone();
    frames.push(opening);

    for step in 1..=total_steps {
        culture = rk4_step(culture, dt_h);
        if step % SAMPLE_STRIDE == 0 {
            let time = step as f64 / SUBSTEPS_PER_HOUR as f64;
            let frame = seal_frame(time, culture, &previous_hash)?;
            previous_hash = frame.current_hash.clone();
            frames.push(frame);
        }
    }

    Ok(Batch {
        frames,
        steps: total_steps,
    })
}

fn write_tape(path: &std::path::Path, frames: &[StateFrame]) -> Result<(), Box<dyn std::error::Error>> {
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
            || !frame.biomass_concentration.is_finite()
            || !frame.substrate_concentration.is_finite()
            || !frame.oxygen_transfer_rate.is_finite()
            || !frame.shear_stress.is_finite()
            || !frame.product_yield.is_finite()
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

fn frame_near<'a>(frames: &'a [StateFrame], time_h: f64) -> &'a StateFrame {
    frames
        .iter()
        .min_by(|a, b| {
            (a.time - time_h)
                .abs()
                .partial_cmp(&(b.time - time_h).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .expect("batch has frames")
}

/// The batch curve leaves a straight chord: fast early growth, oxygen-limited
/// bending, then a stationary tail after substrate depletion.
fn audit_batch(frames: &[StateFrame]) -> Result<(), Box<dyn std::error::Error>> {
    if frames.len() < 3 {
        return Err("batch has fewer than three frames".into());
    }

    let first = &frames[0];
    let last = frames.last().expect("batch has a terminal frame");
    if first.time != 0.0 {
        return Err("tape does not open at t = 0".into());
    }
    if (last.time - BATCH_HOURS as f64).abs() > 1e-9 {
        return Err(format!("tape does not close at t = {} h", BATCH_HOURS).into());
    }
    if (first.biomass_concentration - X0_G_L).abs() > 1e-12
        || (first.substrate_concentration - S0_G_L).abs() > 1e-12
        || first.product_yield.abs() > 1e-12
    {
        return Err("opening frame is not the inoculum".into());
    }
    if first.previous_hash != GENESIS_HASH {
        return Err("opening frame is not chained to the genesis hash".into());
    }

    let mut prev_time = -1.0;
    for (index, frame) in frames.iter().enumerate() {
        if frame.time + 1e-12 < prev_time {
            return Err(format!("frame {index} time is not monotonic").into());
        }
        prev_time = frame.time;
        if frame.biomass_concentration < 0.0
            || frame.substrate_concentration < 0.0
            || frame.oxygen_transfer_rate < 0.0
            || frame.shear_stress < 0.0
            || frame.product_yield < 0.0
        {
            return Err(format!("frame {index} has a negative state").into());
        }
        if index > 0 {
            let prev = &frames[index - 1];
            if frame.biomass_concentration + 1e-9 < prev.biomass_concentration {
                return Err(format!("frame {index} biomass decreased").into());
            }
            if frame.substrate_concentration > prev.substrate_concentration + 1e-9 {
                return Err(format!("frame {index} substrate increased").into());
            }
            if frame.product_yield + 1e-9 < prev.product_yield {
                return Err(format!("frame {index} product decreased").into());
            }
        }
    }

    let span = last.biomass_concentration - first.biomass_concentration;
    if span < 10.0 * X0_G_L {
        return Err("biomass did not complete a batch expansion".into());
    }
    if last.substrate_concentration > 1.0 {
        return Err("substrate was not consumed across the batch".into());
    }
    if last.product_yield <= 1.0 {
        return Err("product yield did not accumulate".into());
    }

    let mut chord_deviation = 0.0_f64;
    for frame in frames {
        let linear = first.biomass_concentration + span * (frame.time / last.time);
        chord_deviation = chord_deviation.max((frame.biomass_concentration - linear).abs());
    }
    if chord_deviation / span < 0.2 {
        return Err("biomass track is too close to a straight line".into());
    }

    let early_a = frame_near(frames, 1.0);
    let early_b = frame_near(frames, 4.0);
    let late_a = frame_near(frames, 60.0);
    let early_mu = (early_b.biomass_concentration / early_a.biomass_concentration).ln()
        / (early_b.time - early_a.time);
    let late_mu = if late_a.biomass_concentration <= 0.0 {
        0.0
    } else {
        (last.biomass_concentration / late_a.biomass_concentration).ln() / (last.time - late_a.time)
    };
    if early_mu < 0.25 {
        return Err("early specific growth is missing the exponential branch".into());
    }
    if late_mu > 0.02 {
        return Err("late batch did not leave the exponential branch".into());
    }

    let shear_ratio = last.shear_stress / frames[1].shear_stress;
    if shear_ratio < 30.0 {
        return Err("shear did not rise with broth viscosity".into());
    }

    let peak = frames
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| {
            a.oxygen_transfer_rate
                .partial_cmp(&b.oxygen_transfer_rate)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .expect("batch has an oxygen peak");
    if peak.0 == 0 || peak.0 + 1 == frames.len() {
        return Err("oxygen transfer peaked on an endpoint".into());
    }
    if peak.1.oxygen_transfer_rate <= first.oxygen_transfer_rate
        || peak.1.oxygen_transfer_rate <= last.oxygen_transfer_rate
    {
        return Err("oxygen transfer did not rise and then fall".into());
    }

    Ok(())
}

fn main() {
    if let Err(err) = run() {
        eprintln!("tape_bio: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let batch = integrate()?;
    audit_batch(&batch.frames)?;
    let path = tape_path();
    write_tape(&path, &batch.frames)?;
    let verified = verify_tape(&path)?;

    let last = batch.frames.last().expect("tape has a terminal frame");
    let tip = &last.current_hash;
    println!("Bioreactor batch tape");
    println!("  sim_time_h:        {:.1}", last.time);
    println!("  steps:             {}", batch.steps);
    println!("  frames:            {verified}");
    println!("  final_biomass_g_l: {:.6}", last.biomass_concentration);
    println!("  final_substrate_g_l: {:.6}", last.substrate_concentration);
    println!("  final_product_g_l: {:.6}", last.product_yield);
    println!("  final_otr_g_l_h:  {:.6}", last.oxygen_transfer_rate);
    println!("  final_shear_Pa:   {:.6}", last.shear_stress);
    println!("  tip:               {tip}");
    println!("  path:              {}", path.display());
    Ok(())
}
