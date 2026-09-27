//! G^G Forge: ORBITAL PLASMA DISCOVERY — Monte Carlo Survival Corridor Search
//! (Updated for EFPA and Velocity Sweep)

use genesis_core::output;
use genesis_core::physics::plasma_facing::PlasmaFacingDesignParams;
use genesis_core::physics::thermal::{
    self, LumpedThermalNode,
};
use genesis_core::proof::{self, ProofChain};
use genesis_core::rng::Rng;

use rayon::prelude::*;
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{BooleanArray, Float64Array, StringArray, UInt32Array, UInt64Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::ArrowWriter;

const DEFAULT_N: usize = 100_000;
const DEFAULT_SEED: u64 = 0xFE12_1E0D_A0FE_E000;
const SEED_MULT: u64 = 0x9E37_79B1_85EB_CA87;
const ALT_ENTRY: f64 = 120_000.0;
/// Integrator has no lift term.
const L_D: f64 = 0.0;
const SWEEP_CD: f64 = 1.65;

const A_REF: f64 = 1.1310;
const MASS_KG: f64 = 350.0;
const R_NOSE: f64 = 0.25;

const ABLATION_RATE_COEFF: f64 = 0.02;
const TPS_THICKNESS_MM: f64 = 50.0;
const ABLATION_ONSET_MW: f64 = 0.3;

const HULL_THERMAL_CAP: f64 = 100_000.0;
const HULL_THERMAL_RES: f64 = 0.030;
const INTERNAL_MELT_C: f64 = 250.0;
const TPS_EMISSIVITY: f64 = 0.85;

const H_SCALE: f64 = 7_400.0;
const RHO_SL: f64 = 1.225;
const DT: f64 = 0.01;
const MAX_TIME_S: f64 = 600.0;
const V_CRASH_LIMIT: f64 = 80.0;
const PROOF_STRIDE: usize = 100;
const MAX_G_LOAD: f64 = 40.0;

const STARDUST_MASS_KG: f64 = 45.8;
const STARDUST_DIAMETER_M: f64 = 0.811;
const STARDUST_CD: f64 = 1.65;
const STARDUST_R_NOSE_M: f64 = 0.23;
const STARDUST_TPS_MM: f64 = 58.0;
const STARDUST_V_ENTRY_M_S: f64 = 12_900.0;
const STARDUST_EFPA_DEG: f64 = -8.2;

#[derive(Clone, Copy)]
struct Vehicle {
    case_name: &'static str,
    mass_kg: f64,
    a_ref_m2: f64,
    cd: f64,
    r_nose_m: f64,
    tps_thickness_mm: f64,
    fixed_v_entry_m_s: Option<f64>,
    fixed_efpa_deg: Option<f64>,
}

fn sweep_vehicle() -> Vehicle {
    Vehicle {
        case_name: "sweep",
        mass_kg: MASS_KG,
        a_ref_m2: A_REF,
        cd: SWEEP_CD,
        r_nose_m: R_NOSE,
        tps_thickness_mm: TPS_THICKNESS_MM,
        fixed_v_entry_m_s: None,
        fixed_efpa_deg: None,
    }
}

fn stardust_vehicle() -> Vehicle {
    let a_ref_m2 = std::f64::consts::PI * (STARDUST_DIAMETER_M / 2.0).powi(2);
    Vehicle {
        case_name: "stardust",
        mass_kg: STARDUST_MASS_KG,
        a_ref_m2,
        cd: STARDUST_CD,
        r_nose_m: STARDUST_R_NOSE_M,
        tps_thickness_mm: STARDUST_TPS_MM,
        fixed_v_entry_m_s: Some(STARDUST_V_ENTRY_M_S),
        fixed_efpa_deg: Some(STARDUST_EFPA_DEG),
    }
}

fn vehicle_for(case_name: &str) -> Vehicle {
    if case_name == "stardust" {
        stardust_vehicle()
    } else {
        sweep_vehicle()
    }
}

#[inline]
fn atm_density(alt_m: f64) -> f64 {
    if alt_m < 0.0 { return RHO_SL; }
    RHO_SL * (-alt_m / H_SCALE).exp()
}

#[inline]
fn atm_temperature_k(alt_m: f64) -> f64 {
    let alt_km = alt_m / 1000.0;
    if alt_km > 85.0 { 186.87 }
    else if alt_km > 47.0 { 270.65 - 2.8 * (alt_km - 47.0) }
    else if alt_km > 32.0 { 228.65 + 2.8 * (alt_km - 32.0) }
    else if alt_km > 11.0 { 216.65 }
    else { 288.15 - 6.5 * alt_km }
}

#[inline]
fn sutton_graves_heat_flux(rho: f64, velocity: f64, r_nose_m: f64) -> f64 {
    1.7415e-4 * (rho / r_nose_m).sqrt() * velocity.powi(3)
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Outcome {
    Survived,
    MeltedSurface,
    MeltedInternals,
    Crashed,
    Crushed,
    SkipOut,
}

impl Outcome {
    fn label(&self) -> &'static str {
        match self {
            Outcome::Survived => "SURVIVED",
            Outcome::MeltedSurface => "MELTED_SURFACE",
            Outcome::MeltedInternals => "MELTED_INTERNALS",
            Outcome::Crashed => "CRASHED",
            Outcome::Crushed => "CRUSHED",
            Outcome::SkipOut => "SKIP_OUT",
        }
    }
}

#[derive(Debug)]
struct TrajectoryResult {
    id: u32,
    short_id: String,
    v_entry_m_s: f64,
    efpa_deg: f64,
    outcome: Outcome,
    max_heat_flux_mw_m2: f64,
    max_surface_temp_k: f64,
    max_internal_temp_c: f64,
    total_heat_load_mj_m2: f64,
    final_velocity_m_s: f64,
    final_altitude_m: f64,
    time_in_plasma_s: f64,
    peak_deceleration_g: f64,
    ablation_depth_mm: f64,
    tps_remaining_mm: f64,
    ballistic_coeff: f64,
    mass_kg: f64,
    a_ref_m2: f64,
    cd: f64,
    r_nose_m: f64,
    tps_thickness_mm: f64,
    l_d: f64,
    entry_alt_m: f64,
    dt_s: f64,
    seed: u64,
    case_name: &'static str,
    first_gate: &'static str,
    t_gate_s: f64,
    h_gate_m: f64,
    v_gate_m_s: f64,
    h_peak_g_m: f64,
    h_peak_q_m: f64,
    gate_name: &'static str,
    gate_value: f64,
    gate_limit: f64,
    is_survived: bool,
    is_melted_surface: bool,
    is_melted_internals: bool,
    is_crashed: bool,
    is_crushed: bool,
    is_skipout: bool,
    proof_hash: String,
}

fn run_trajectory(index: usize, seed: u64, vehicle: Vehicle) -> TrajectoryResult {
    let mut rng = Rng::new(seed);
    let short_id = output::short_id(&mut rng);

    let (v_entry, efpa_deg) = match (vehicle.fixed_v_entry_m_s, vehicle.fixed_efpa_deg) {
        (Some(v), Some(efpa)) => (v, efpa),
        _ => (rng.range(7_500.0, 12_900.0), rng.range(-15.0, -1.0)),
    };
    let gamma_entry = efpa_deg * std::f64::consts::PI / 180.0;

    let cd = vehicle.cd;
    let a_exposed = vehicle.a_ref_m2;
    let mass_kg = vehicle.mass_kg;
    let tps_mm = vehicle.tps_thickness_mm;
    let ballistic_coeff = mass_kg / (cd * a_exposed);

    let pfm_params = PlasmaFacingDesignParams::default();

    let mut velocity = v_entry;
    let mut altitude = ALT_ENTRY;
    let mut v_vertical = velocity * gamma_entry.sin();
    let mut v_horizontal = velocity * gamma_entry.cos();

    let mut surface_temp_k: f64 = 300.0;
    let mut ablation_total_mm: f64 = 0.0;
    let mut hull_node = LumpedThermalNode::new(25.0, HULL_THERMAL_CAP, HULL_THERMAL_RES);

    let mut max_heat_flux: f64 = 0.0;
    let mut max_surface_temp: f64 = 300.0;
    let mut max_internal_temp: f64 = 25.0;
    let mut total_heat_load: f64 = 0.0;
    let mut time_in_plasma: f64 = 0.0;
    let mut peak_decel_g: f64 = 0.0;

    let mut proof = ProofChain::new();
    proof.seed(&index.to_le_bytes());
    proof.feed_f64(v_entry);
    proof.feed_f64(efpa_deg);
    proof.feed_str("orbital_plasma_discovery");
    proof.feed_str(vehicle.case_name);
    proof.feed_f64(mass_kg);
    proof.feed_f64(a_exposed);
    proof.feed_f64(vehicle.r_nose_m);
    proof.feed_f64(tps_mm);

    let max_steps = (MAX_TIME_S / DT) as usize;
    let mut outcome = Outcome::Crashed;
    let mut latched = false;
    let mut first_gate: &'static str = "MAX_TIME_S";
    let mut t_gate_s = MAX_TIME_S;
    let mut h_gate_m = altitude;
    let mut v_gate_m_s = velocity;
    let mut gate_name: &'static str = "MAX_TIME_S";
    let mut gate_value = MAX_TIME_S;
    let mut gate_limit = MAX_TIME_S;
    let mut h_peak_g_m = altitude;
    let mut h_peak_q_m = altitude;

    for step in 0..max_steps {
        let t_s = step as f64 * DT;
        let rho = atm_density(altitude);
        let t_inf_k = atm_temperature_k(altitude);
        let q_dyn = 0.5 * rho * velocity * velocity;

        let drag_accel = (cd * a_exposed * q_dyn) / mass_kg;
        let current_g = drag_accel / 9.81;
        if current_g >= peak_decel_g {
            peak_decel_g = current_g;
            h_peak_g_m = altitude;
        }

        if current_g > MAX_G_LOAD && !latched {
            latched = true;
            outcome = Outcome::Crushed;
            first_gate = "MAX_G_LOAD";
            t_gate_s = t_s;
            h_gate_m = altitude;
            v_gate_m_s = velocity;
            gate_name = "MAX_G_LOAD";
            gate_value = current_g;
            gate_limit = MAX_G_LOAD;
        }

        let q_conv_base = sutton_graves_heat_flux(rho, velocity, vehicle.r_nose_m);
        let q_mw_m2 = q_conv_base / 1e6;

        if q_mw_m2 > 0.1 { time_in_plasma += DT; }
        total_heat_load += q_mw_m2 * DT;
        if q_mw_m2 >= max_heat_flux {
            max_heat_flux = q_mw_m2;
            h_peak_q_m = altitude;
        }

        let t_rad_eq = if q_conv_base > 0.0 {
            (q_conv_base / (TPS_EMISSIVITY * thermal::STEFAN_BOLTZMANN_SIGMA)).powf(0.25)
        } else { t_inf_k };

        let tau_surface = 3.0;
        let decay = (-DT / tau_surface).exp();
        surface_temp_k = t_rad_eq + (surface_temp_k - t_rad_eq) * decay;
        max_surface_temp = max_surface_temp.max(surface_temp_k);

        if q_mw_m2 > ABLATION_ONSET_MW {
            let excess_mw = q_mw_m2 - ABLATION_ONSET_MW;
            let recession_rate_mm_s = ABLATION_RATE_COEFF * excess_mw * (1.0 + 0.1 * excess_mw) / (pfm_params.heat_of_vaporization_mj_kg / 11.5);
            ablation_total_mm += recession_rate_mm_s * DT;
        }

        if ablation_total_mm > tps_mm && !latched {
            latched = true;
            outcome = Outcome::MeltedSurface;
            first_gate = "tps_thickness_mm";
            t_gate_s = t_s;
            h_gate_m = altitude;
            v_gate_m_s = velocity;
            gate_name = "tps_thickness_mm";
            gate_value = ablation_total_mm;
            gate_limit = tps_mm;
        }

        let tps_fraction_remaining = ((tps_mm - ablation_total_mm) / tps_mm).clamp(0.0, 1.0);
        let tps_block_eff = 0.97 * tps_fraction_remaining + 0.50 * (1.0 - tps_fraction_remaining);
        let heat_through_tps_w = q_conv_base * a_exposed * (1.0 - tps_block_eff);
        hull_node.step(heat_through_tps_w, t_inf_k - 273.15, DT);
        max_internal_temp = max_internal_temp.max(hull_node.temperature_c);

        if hull_node.temperature_c > INTERNAL_MELT_C && !latched {
            latched = true;
            outcome = Outcome::MeltedInternals;
            first_gate = "INTERNAL_MELT_C";
            t_gate_s = t_s;
            h_gate_m = altitude;
            v_gate_m_s = velocity;
            gate_name = "INTERNAL_MELT_C";
            gate_value = hull_node.temperature_c;
            gate_limit = INTERNAL_MELT_C;
        }

        let gamma_angle = if velocity > 1.0 { (v_vertical / velocity).clamp(-1.0, 1.0).asin() } else { -std::f64::consts::FRAC_PI_2 };
        v_horizontal -= drag_accel * gamma_angle.cos() * DT;
        v_horizontal = v_horizontal.max(0.0);
        
        let r_earth = 6371000.0;
        let centrifugal = (v_horizontal * v_horizontal) / (r_earth + altitude);
        v_vertical += (-drag_accel * gamma_angle.sin() - 9.81 + centrifugal) * DT;

        velocity = (v_horizontal * v_horizontal + v_vertical * v_vertical).sqrt();
        altitude += v_vertical * DT;

        if step % PROOF_STRIDE == 0 {
            proof.feed_f64(velocity);
            proof.feed_f64(altitude);
            proof.feed_f64(q_mw_m2);
            proof.feed_f64(hull_node.temperature_c);
            proof.feed_f64(surface_temp_k);
        }

        if altitude <= 0.0 {
            if !latched {
                latched = true;
                t_gate_s = t_s + DT;
                h_gate_m = altitude;
                v_gate_m_s = velocity;
                if velocity < V_CRASH_LIMIT {
                    outcome = Outcome::Survived;
                    first_gate = "NONE";
                    gate_name = "NONE";
                    gate_value = velocity;
                    gate_limit = V_CRASH_LIMIT;
                } else {
                    outcome = Outcome::Crashed;
                    first_gate = "V_CRASH_LIMIT";
                    gate_name = "V_CRASH_LIMIT";
                    gate_value = velocity;
                    gate_limit = V_CRASH_LIMIT;
                }
            }
            break;
        }

        if step > 100 && altitude > ALT_ENTRY && v_vertical > 0.0 {
            if !latched {
                latched = true;
                outcome = Outcome::SkipOut;
                first_gate = "ALT_ENTRY";
                t_gate_s = t_s + DT;
                h_gate_m = altitude;
                v_gate_m_s = velocity;
                gate_name = "ALT_ENTRY";
                gate_value = altitude;
                gate_limit = ALT_ENTRY;
            }
            break;
        }
    }

    if !latched {
        h_gate_m = altitude;
        v_gate_m_s = velocity;
    }

    proof.feed_f64(efpa_deg);
    proof.feed_f64(v_entry);
    proof.feed_f64(max_heat_flux);
    proof.feed_f64(max_internal_temp);
    proof.feed_str(outcome.label());
    let proof_hash = proof.seal();

    TrajectoryResult {
        id: index as u32,
        short_id,
        v_entry_m_s: (v_entry * 10.0).round() / 10.0,
        efpa_deg: (efpa_deg * 1000.0).round() / 1000.0,
        outcome,
        max_heat_flux_mw_m2: (max_heat_flux * 1000.0).round() / 1000.0,
        max_surface_temp_k: max_surface_temp.round(),
        max_internal_temp_c: (max_internal_temp * 10.0).round() / 10.0,
        total_heat_load_mj_m2: (total_heat_load * 100.0).round() / 100.0,
        final_velocity_m_s: (velocity * 10.0).round() / 10.0,
        final_altitude_m: altitude.max(0.0).round(),
        time_in_plasma_s: (time_in_plasma * 10.0).round() / 10.0,
        peak_deceleration_g: (peak_decel_g * 10.0).round() / 10.0,
        ablation_depth_mm: (ablation_total_mm * 100.0).round() / 100.0,
        tps_remaining_mm: ((tps_mm - ablation_total_mm).max(0.0) * 100.0).round() / 100.0,
        ballistic_coeff: (ballistic_coeff * 10.0).round() / 10.0,
        mass_kg,
        a_ref_m2: a_exposed,
        cd,
        r_nose_m: vehicle.r_nose_m,
        tps_thickness_mm: tps_mm,
        l_d: L_D,
        entry_alt_m: ALT_ENTRY,
        dt_s: DT,
        seed,
        case_name: vehicle.case_name,
        first_gate,
        t_gate_s,
        h_gate_m,
        v_gate_m_s,
        h_peak_g_m,
        h_peak_q_m,
        gate_name,
        gate_value,
        gate_limit,
        is_survived: outcome == Outcome::Survived,
        is_melted_surface: outcome == Outcome::MeltedSurface,
        is_melted_internals: outcome == Outcome::MeltedInternals,
        is_crashed: outcome == Outcome::Crashed,
        is_crushed: outcome == Outcome::Crushed,
        is_skipout: outcome == Outcome::SkipOut,
        proof_hash,
    }
}

struct Cli {
    n: Option<usize>,
    parquet: Option<String>,
    case_name: String,
    seed: u64,
    overwrite: bool,
}

fn print_help() {
    println!(
        "\
orbital_plasma_discovery_mc

Usage:
  cargo run --release --bin orbital_plasma_discovery_mc -- [options]

Options:
  --help                 Print this usage and exit. Does not run a trajectory.
  --n <usize>            Trajectory count.
                         Default: 1 for --case stardust, {default_n} for --case sweep.
  --parquet <path>       Output parquet path.
  --case <sweep|stardust>
                         sweep (default): mass {mass} kg, A_ref {area}, Cd {cd}, Rn {rn} m,
                         TPS {tps} mm. Ve uniform on [7500, 12900] m/s.
                         EFPA uniform on [-15, -1] deg.
                         stardust: fixed IC, separate from the sweep vehicle.
                         mass {sd_mass} kg, diameter {sd_d} m, Cd {sd_cd}, Rn {sd_rn} m,
                         TPS {sd_tps} mm, Ve {sd_v} m/s, EFPA {sd_efpa} deg.
  --seed <u64>           Base seed. Decimal or 0x hex. Default 0x{default_seed:X}.
                         Row seed = base XOR (index * 0x{seed_mult:X}).
  --overwrite            Replace the target parquet if it already exists.

Default parquet (refused when the file already exists, unless --overwrite):
  --case sweep      <manifest>/../../data/orbital_plasma_discovery_sweep.parquet
  --case stardust   <manifest>/../../data/stardust_validation.parquet

The sweep default path is not orbital_plasma_discovery.parquet.
",
        default_n = DEFAULT_N,
        mass = MASS_KG,
        area = A_REF,
        cd = SWEEP_CD,
        rn = R_NOSE,
        tps = TPS_THICKNESS_MM,
        sd_mass = STARDUST_MASS_KG,
        sd_d = STARDUST_DIAMETER_M,
        sd_cd = STARDUST_CD,
        sd_rn = STARDUST_R_NOSE_M,
        sd_tps = STARDUST_TPS_MM,
        sd_v = STARDUST_V_ENTRY_M_S,
        sd_efpa = STARDUST_EFPA_DEG,
        default_seed = DEFAULT_SEED,
        seed_mult = SEED_MULT,
    );
}

fn parse_u64(text: &str) -> Result<u64, String> {
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).map_err(|e| format!("--seed {text}: {e}"))
    } else {
        text.parse::<u64>().map_err(|e| format!("--seed {text}: {e}"))
    }
}

fn require_value(flag: &str, value: Option<String>) -> Result<String, String> {
    value.ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_cli(args: &[String]) -> Result<Cli, String> {
    let mut n = None;
    let mut parquet = None;
    let mut case_name = "sweep".to_string();
    let mut seed = DEFAULT_SEED;
    let mut overwrite = false;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "--overwrite" => overwrite = true,
            "--n" => {
                let value = require_value("--n", args.get(i + 1).cloned())?;
                n = Some(value.parse::<usize>().map_err(|e| format!("--n {value}: {e}"))?);
                i += 1;
            }
            "--parquet" => {
                parquet = Some(require_value("--parquet", args.get(i + 1).cloned())?);
                i += 1;
            }
            "--case" => {
                case_name = require_value("--case", args.get(i + 1).cloned())?;
                if case_name != "sweep" && case_name != "stardust" {
                    return Err(format!("--case {case_name} is not sweep or stardust"));
                }
                i += 1;
            }
            "--seed" => {
                let value = require_value("--seed", args.get(i + 1).cloned())?;
                seed = parse_u64(&value)?;
                i += 1;
            }
            other => return Err(format!("unknown argument {other}")),
        }
        i += 1;
    }
    Ok(Cli { n, parquet, case_name, seed, overwrite })
}

fn default_parquet(case_name: &str) -> String {
    let file = if case_name == "stardust" {
        "stardust_validation.parquet"
    } else {
        "orbital_plasma_discovery_sweep.parquet"
    };
    format!("{}/../../data/{file}", env!("CARGO_MANIFEST_DIR"))
}

fn print_stardust_block(row: &TrajectoryResult) {
    println!("first_gate: {}", row.first_gate);
    println!(
        "t_gate_s: {:.3} h_gate_m: {:.1} v_gate_m_s: {:.1}",
        row.t_gate_s, row.h_gate_m, row.v_gate_m_s
    );
    println!(
        "peak_q_mw_m2: {} peak_g: {} recession_mm: {} max_bondline_c: {}",
        row.max_heat_flux_mw_m2,
        row.peak_deceleration_g,
        row.ablation_depth_mm,
        row.max_internal_temp_c
    );
    println!(
        "v_final: {} h_final: {} outcome: {}",
        row.final_velocity_m_s,
        row.final_altitude_m,
        row.outcome.label()
    );
    println!(
        "h_peak_g_m: {:.1} h_peak_q_m: {:.1}",
        row.h_peak_g_m, row.h_peak_q_m
    );
}

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if raw.iter().any(|a| a == "--help") {
        print_help();
        std::process::exit(0);
    }
    let cli = match parse_cli(&raw) {
        Ok(cli) => cli,
        Err(err) => {
            eprintln!("{err}");
            print_help();
            std::process::exit(2);
        }
    };
    let n = cli.n.unwrap_or(if cli.case_name == "stardust" { 1 } else { DEFAULT_N });
    if n == 0 {
        eprintln!("--n must be >= 1");
        std::process::exit(2);
    }
    let out = cli.parquet.clone().unwrap_or_else(|| default_parquet(&cli.case_name));
    if std::path::Path::new(&out).exists() && !cli.overwrite {
        eprintln!("refusing to write {out}: file exists. Pass --overwrite to replace it.");
        std::process::exit(1);
    }

    let vehicle = vehicle_for(&cli.case_name);
    let base_seed = cli.seed;

    println!("case: {}", vehicle.case_name);
    println!("n: {n}");
    println!("parquet: {out}");

    let t0 = Instant::now();

    let rows: Vec<TrajectoryResult> = (0..n).into_par_iter().map(|i| {
        let seed = base_seed ^ (i as u64).wrapping_mul(SEED_MULT);
        run_trajectory(i, seed, vehicle)
    }).collect();

    let elapsed = t0.elapsed();

    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let seal = proof::seal_run(&proofs);

    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("v_entry_m_s", DataType::Float64, false),
        Field::new("efpa_deg", DataType::Float64, false),
        Field::new("outcome", DataType::Utf8, false),
        Field::new("max_heat_flux_mw_m2", DataType::Float64, false),
        Field::new("max_surface_temp_k", DataType::Float64, false),
        Field::new("max_internal_temp_c", DataType::Float64, false),
        Field::new("total_heat_load_mj_m2", DataType::Float64, false),
        Field::new("final_velocity_m_s", DataType::Float64, false),
        Field::new("final_altitude_m", DataType::Float64, false),
        Field::new("time_in_plasma_s", DataType::Float64, false),
        Field::new("peak_deceleration_g", DataType::Float64, false),
        Field::new("ablation_depth_mm", DataType::Float64, false),
        Field::new("tps_remaining_mm", DataType::Float64, false),
        Field::new("ballistic_coeff", DataType::Float64, false),
        Field::new("mass_kg", DataType::Float64, false),
        Field::new("a_ref_m2", DataType::Float64, false),
        Field::new("cd", DataType::Float64, false),
        Field::new("r_nose_m", DataType::Float64, false),
        Field::new("tps_thickness_mm", DataType::Float64, false),
        Field::new("l_d", DataType::Float64, false),
        Field::new("entry_alt_m", DataType::Float64, false),
        Field::new("dt_s", DataType::Float64, false),
        Field::new("seed", DataType::UInt64, false),
        Field::new("case_name", DataType::Utf8, false),
        Field::new("first_gate", DataType::Utf8, false),
        Field::new("t_gate_s", DataType::Float64, false),
        Field::new("h_gate_m", DataType::Float64, false),
        Field::new("v_gate_m_s", DataType::Float64, false),
        Field::new("is_survived", DataType::Boolean, false),
        Field::new("is_melted_surface", DataType::Boolean, false),
        Field::new("is_melted_internals", DataType::Boolean, false),
        Field::new("is_crashed", DataType::Boolean, false),
        Field::new("is_crushed", DataType::Boolean, false),
        Field::new("is_skipout", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.v_entry_m_s).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.efpa_deg).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.outcome.label()).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.max_heat_flux_mw_m2).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.max_surface_temp_k).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.max_internal_temp_c).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.total_heat_load_mj_m2).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.final_velocity_m_s).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.final_altitude_m).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.time_in_plasma_s).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.peak_deceleration_g).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.ablation_depth_mm).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.tps_remaining_mm).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.ballistic_coeff).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.mass_kg).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.a_ref_m2).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.cd).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.r_nose_m).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.tps_thickness_mm).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.l_d).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.entry_alt_m).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.dt_s).collect::<Vec<_>>())),
            Arc::new(UInt64Array::from(rows.iter().map(|r| r.seed).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.case_name).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.first_gate).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.t_gate_s).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.h_gate_m).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(rows.iter().map(|r| r.v_gate_m_s).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_survived).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_melted_surface).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_melted_internals).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_crashed).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_crushed).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_skipout).collect::<Vec<_>>())),
            Arc::new(StringArray::from(rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>())),
        ],
    ).expect("RecordBatch construction failed");

    let file = std::fs::File::create(&out).expect("Failed to create parquet file");
    let generator = if cli.case_name == "stardust" {
        "G^G Orbital Plasma Discovery MC v1.0 — Stardust"
    } else {
        "G^G Orbital Plasma Discovery MC v1.0 — Sweep"
    };
    let props = output::parquet_receipt_properties(&seal, generator);
    let mut writer = ArrowWriter::try_new(file, schema, Some(props)).expect("ArrowWriter init");
    writer.write(&batch).expect("Parquet write");
    writer.close().expect("Parquet close");

    let nf = n as f64;
    let survived = rows.iter().filter(|r| r.is_survived).count();
    let melted_s = rows.iter().filter(|r| r.is_melted_surface).count();
    let melted_i = rows.iter().filter(|r| r.is_melted_internals).count();
    let crashed = rows.iter().filter(|r| r.is_crashed).count();
    let crushed = rows.iter().filter(|r| r.is_crushed).count();
    let skipout = rows.iter().filter(|r| r.is_skipout).count();

    if cli.case_name == "stardust" {
        for row in &rows {
            print_stardust_block(row);
        }
    } else {
        println!("SURVIVED: {survived} ({:.2}%)", 100.0 * survived as f64 / nf);
        println!("SKIP_OUT: {skipout} ({:.2}%)", 100.0 * skipout as f64 / nf);
        println!("MELTED_SURFACE: {melted_s} ({:.2}%)", 100.0 * melted_s as f64 / nf);
        println!("MELTED_INTERNALS: {melted_i} ({:.2}%)", 100.0 * melted_i as f64 / nf);
        println!("CRUSHED: {crushed} ({:.2}%)", 100.0 * crushed as f64 / nf);
        println!("CRASHED: {crashed} ({:.2}%)", 100.0 * crashed as f64 / nf);
    }
    println!("seal: {seal}");
    println!("parquet: {out}");
    println!("wall_s: {:.3}", elapsed.as_secs_f64());
}
