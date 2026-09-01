//! Hypersonic aerothermal ablation. Sutton-Graves heating, CoG migration, static-margin inversion.
//! Integrator is 1 kHz Euler on the Forge body — not the Reflex grasp loop, not reentry plasma's 20 Hz.
//! Dual-regime exclusive: live · asymmetric ablation · CoG past AC · plasma-density spike.
//! Unnamed remainder is named (attitude departure) and re-sealed. No flutter. No products path.
//! 128-byte HypersonicDynamicsState. Inline: no physics module `use`.

use genesis_core::output;
use genesis_core::proof::{self, ProofChain};
use genesis_core::rng::Rng;
use rayon::prelude::*;
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{BooleanArray, Float64Array, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::ArrowWriter;

const DEFAULT_N: usize = 2500;
const MACH_1: f32 = 295.0;
const PLASMA_BLACKOUT_MIN_VEL: f32 = MACH_1 * 10.0;
const AC_MARGIN_M: f32 = 0.95; // CoG past aerodynamic center
const DT: f32 = 0.001;
const MAX_TIME_S: f32 = 360.0;
const PROOF_STRIDE: usize = 250; // 4 Hz into the chain

#[allow(dead_code)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Serialize)]
struct HypersonicDynamicsState {
    timestamp: f32,
    pos: [f32; 3],
    vel: [f32; 3],
    quat: [f32; 4],
    ang_vel: [f32; 3],
    actuator_deflections: [f32; 4],
    stability_jacobians: [f32; 8],
    cog_migration: [f32; 3],
    aeroshell_thickness: f32,
    freestream_density: f32,
    thermal_accumulated: f32,
}

const _: () = assert!(std::mem::size_of::<HypersonicDynamicsState>() == 128);

#[derive(Debug, Clone, Copy, PartialEq)]
enum FailureMode {
    Nominal,
    AsymmetricAblation,
    AeroshellErosion,
    PlasmaDensitySpike,
}

struct HgvObserver {
    est_pitch: f32,
    est_pitch_rate: f32,
    assumed_cog_offset: f32,
    p_cov: [f32; 4],
}

impl HgvObserver {
    fn new() -> Self {
        HgvObserver {
            est_pitch: 0.0,
            est_pitch_rate: 0.0,
            assumed_cog_offset: 1.2,
            p_cov: [0.1, 0.0, 0.0, 0.1],
        }
    }

    fn predict_and_update(
        &mut self,
        dt: f32,
        measured_pitch_rate: f32,
        control_deflection: f32,
        dynamic_pressure: f32,
        has_gps: bool,
    ) {
        let deflection_rad = control_deflection.to_radians();
        let expected_moment = dynamic_pressure * 0.5 * deflection_rad * self.assumed_cog_offset;
        let assumed_i_yy = 2000.0;
        let expected_accel = expected_moment / assumed_i_yy;
        self.est_pitch_rate += expected_accel * dt;
        self.est_pitch += self.est_pitch_rate * dt;

        let p00 = self.p_cov[0] + 2.0 * self.p_cov[1] * dt + self.p_cov[3] * dt * dt;
        let p01 = self.p_cov[1] + self.p_cov[3] * dt;
        let p11 = self.p_cov[3];
        self.p_cov = [p00 + 1e-4 * dt, p01, p01, p11 + 1e-3 * dt];

        if has_gps {
            let r_noise = 0.05;
            let s_val = self.p_cov[3] + r_noise;
            let k0 = self.p_cov[1] / s_val;
            let k1 = self.p_cov[3] / s_val;
            let innovation = measured_pitch_rate - self.est_pitch_rate;
            self.est_pitch_rate += k1 * innovation;
            self.est_pitch += k0 * innovation;
            let new_p00 = self.p_cov[0] - k0 * self.p_cov[2];
            let new_p01 = self.p_cov[1] - k0 * self.p_cov[3];
            let new_p10 = self.p_cov[2] - k1 * self.p_cov[2];
            let new_p11 = self.p_cov[3] - k1 * self.p_cov[3];
            self.p_cov = [new_p00, new_p01, new_p10, new_p11];
        } else {
            let innovation = measured_pitch_rate - self.est_pitch_rate;
            self.est_pitch_rate += 0.02 * innovation;
            self.est_pitch += self.est_pitch_rate * dt;
        }
    }

    fn get_control_command(&self, target_pitch: f32) -> f32 {
        let error = target_pitch - self.est_pitch;
        let cmd_rad = error * 12.0 - self.est_pitch_rate * 4.0;
        cmd_rad.to_degrees()
    }
}

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    scenario: String,
    mach_start: f64,
    min_cog_offset_m: f64,
    mass_final_kg: f64,
    max_surface_temp_k: f64,
    max_attitude_error_deg: f64,
    aeroshell_thickness_m: f64,
    is_asymmetric_ablation: bool,
    is_cog_past_ac: bool,
    is_plasma_density_spike: bool,
    is_live: bool,
    is_attitude_departure: bool,
    proof_hash: String,
}

fn run_one(index: usize, seed: u64, scenario: &str) -> Run {
    let mut rng = Rng::new(seed);
    let short_id = output::short_id(&mut rng);

    let mut true_vel = rng.range((MACH_1 * 8.0) as f64, (MACH_1 * 18.0) as f64) as f32;
    let mach_start = (true_vel / MACH_1) as f64;
    let initial_mass = 1500.0f32;
    let mut true_mass = initial_mass;
    let mut pos = [0.0f32, 0.0f32, 80_000.0f32];
    let mut vel = [true_vel, 0.0f32, -150.0f32];
    let mut true_pitch = 0.0f32;
    let mut true_pitch_rate = 0.0f32;
    let mut true_cog_offset = 1.2f32;
    let material_density = rng.range(1.7, 2.1) as f32;

    let failure = match scenario {
        "asymmetric_ablation" | "asymmetric" => FailureMode::AsymmetricAblation,
        "aeroshell_erosion" | "erosion" => FailureMode::AeroshellErosion,
        "plasma_density_spike" | "spike" => FailureMode::PlasmaDensitySpike,
        "nominal" => FailureMode::Nominal,
        _ => {
            if rng.chance(0.25) {
                FailureMode::AsymmetricAblation
            } else if rng.chance(0.33) {
                FailureMode::AeroshellErosion
            } else if rng.chance(0.50) {
                FailureMode::PlasmaDensitySpike
            } else {
                FailureMode::Nominal
            }
        }
    };
    let scenario_name = match failure {
        FailureMode::AsymmetricAblation => "asymmetric_ablation",
        FailureMode::AeroshellErosion => "aeroshell_erosion",
        FailureMode::PlasmaDensitySpike => "plasma_density_spike",
        FailureMode::Nominal => "nominal",
    };

    let mut observer = HgvObserver::new();
    let mut last_control_deflection = 0.0f32;
    let mut proof = ProofChain::new();
    proof.seed(&index.to_le_bytes());
    proof.feed_f64(mach_start);
    proof.feed_str(scenario_name);

    let max_steps = (MAX_TIME_S / DT) as usize;
    let mut min_cog = true_cog_offset;
    let mut max_temp = 300.0f32;
    let mut max_att_err = 0.0f32;
    let mut aeroshell_thickness = 0.05f32;
    let mut departure = false;

    for step in 0..max_steps {
        let t = step as f32 * DT;
        let altitude_km = pos[2] / 1000.0;
        let temp_k = if altitude_km > 51.0 {
            270.65 - 2.8 * (altitude_km - 51.0)
        } else if altitude_km > 47.0 {
            270.65
        } else {
            228.65 + 2.8 * (altitude_km - 32.0)
        };
        let _local_speed_of_sound = (1.4 * 287.05 * temp_k).sqrt();

        let mut rho = (-altitude_km / 7.0).exp() * 1.225;
        if failure == FailureMode::PlasmaDensitySpike && t > 40.0 && t < 45.0 {
            rho *= 2.5;
        }

        let q_pressure = 0.5 * rho * true_vel * true_vel;
        let is_plasma = true_vel > PLASMA_BLACKOUT_MIN_VEL && altitude_km < 85.0;
        let has_gps = !is_plasma;
        let ekf_trace = observer.p_cov[0] + observer.p_cov[3];
        let guidance_lockout = ekf_trace > 0.5;

        let measured_pitch_rate = true_pitch_rate + rng.range(-0.01, 0.01) as f32;
        let target_pitch = 5.0f32.to_radians();
        let mut control_deflection = if guidance_lockout {
            last_control_deflection
        } else {
            let cmd = observer.get_control_command(target_pitch);
            last_control_deflection = cmd;
            cmd
        };
        observer.predict_and_update(DT, measured_pitch_rate, control_deflection, q_pressure, has_gps);

        let attitude_error = (true_pitch - observer.est_pitch).abs();
        if failure == FailureMode::Nominal && attitude_error > 2.0f32.to_radians() {
            let trim_compensate = 8.0 * (0.95 - true_cog_offset).max(0.0);
            control_deflection += if true_pitch > target_pitch {
                -trim_compensate
            } else {
                trim_compensate
            };
        }
        control_deflection = control_deflection.clamp(-25.0, 25.0);

        let nose_radius = 0.15 + 0.50 * (1.0 - (true_mass / initial_mass));
        let cl_a = 2.0 * true_pitch.sin().powi(2) * true_pitch.cos();
        let cd_a = 2.0 * true_pitch.sin().powi(3);
        let cl_d = 0.8 * control_deflection.to_radians().sin();
        let cd_d = 0.1 * control_deflection.to_radians().sin().powi(2);
        let cd_dynamic = 0.10 + 0.05 * (nose_radius - 0.15) / 0.15;

        let mut surface_temp = 300.0;
        if is_plasma {
            let heat_flux_q = 1.7415e-4 * (rho / nose_radius).sqrt() * true_vel.powi(3);
            let epsilon = 0.85;
            let sigma_sb = 5.670374e-8;
            surface_temp = (heat_flux_q / (epsilon * sigma_sb)).powf(0.25);
            max_temp = max_temp.max(surface_temp);

            let max_density = 2.26;
            let porosity = 1.0 - (material_density / max_density);
            let open_porosity_fraction =
                1.0 / (1.0 + (150.0 * (material_density - 1.98)).exp());
            let k_boundary = 0.8e-7;
            let k_open = 4.5e-7 * (1.7 / material_density).powi(2);
            let ablation_constant = k_boundary + (k_open - k_boundary) * open_porosity_fraction;
            let mechanical_erosion_rate =
                0.05 * q_pressure * (surface_temp / 3000.0).powi(2) * porosity;
            let mut mass_burn_rate =
                1.0 * ((heat_flux_q * ablation_constant) + mechanical_erosion_rate);
            if failure == FailureMode::AeroshellErosion {
                mass_burn_rate *= 2.2;
            }
            true_mass = (true_mass - mass_burn_rate * DT).max(100.0);
            true_cog_offset = 1.2 - 0.31 * (1.0 - (true_mass / initial_mass));
            if failure == FailureMode::AsymmetricAblation {
                true_pitch_rate += 0.5 * (heat_flux_q * 1.0e-6) * DT;
            }
        }

        min_cog = min_cog.min(true_cog_offset);

        let fin_authority = if altitude_km > 75.0 { 0.0 } else { 1.0 };
        let mut true_moment =
            q_pressure * 0.5 * (control_deflection.to_radians() * fin_authority) * true_cog_offset;
        if true_cog_offset < AC_MARGIN_M {
            true_moment += q_pressure * 0.5 * true_pitch * (AC_MARGIN_M - true_cog_offset) * 25.0;
        }
        let i_yy = (2000.0 * (true_mass / initial_mass)).max(200.0);
        true_pitch_rate += (true_moment / i_yy) * DT;
        true_pitch += true_pitch_rate * DT;

        let drag_coefficient = cd_dynamic + cd_a + cd_d;
        let drag_accel = (q_pressure * drag_coefficient) / true_mass;
        true_vel -= drag_accel * DT;
        let lift_coefficient = cl_a + cl_d;
        let lift_accel = (q_pressure * lift_coefficient) / true_mass;
        vel[0] = true_vel;
        vel[2] += (lift_accel - 9.81) * DT;
        pos[0] += vel[0] * DT;
        pos[2] += vel[2] * DT;

        let attitude_error_deg = attitude_error * (180.0 / std::f32::consts::PI);
        max_att_err = max_att_err.max(attitude_error_deg);
        aeroshell_thickness =
            (0.05 - (initial_mass - true_mass) / (material_density * 1000.0)).max(0.005);

        if step % PROOF_STRIDE == 0 {
            proof.feed_f64(true_cog_offset as f64);
            proof.feed_f64(true_mass as f64);
            proof.feed_f64(surface_temp as f64);
        }

        let _state = HypersonicDynamicsState {
            timestamp: t,
            pos,
            vel,
            quat: [1.0, 0.0, 0.0, 0.0],
            ang_vel: [0.0, true_pitch_rate, 0.0],
            actuator_deflections: [control_deflection, 0.0, 0.0, 0.0],
            stability_jacobians: [cl_a, cd_a, cl_d, cd_d, 0.1 * cl_a, 0.1 * cd_a, 0.0, 0.0],
            cog_migration: [true_cog_offset, 0.0, 0.0],
            aeroshell_thickness,
            freestream_density: rho,
            thermal_accumulated: 0.0,
        };
        let _ = _state;

        let is_unstable = true_cog_offset < AC_MARGIN_M;
        let is_spinning = attitude_error_deg > 15.0;
        if is_unstable && is_spinning {
            departure = true;
            break;
        }
        if pos[2] <= 0.0 {
            break;
        }
    }

    // Exclusive organ-first (same order as the proof string).
    // Asymmetric and spike keep their injected organ even if CoG also walks aft
    // (measured min_cog_offset_m still carries the inversion). CoG-past-AC is the
    // erosion organ plus any unnamed mass-loss inversion. Remainder is attitude
    // departure (spin without those organs) — named, not a footnote.
    let saw_asymmetric = failure == FailureMode::AsymmetricAblation;
    let saw_spike = failure == FailureMode::PlasmaDensitySpike;
    let saw_erosion = failure == FailureMode::AeroshellErosion;
    let cog_inverted = min_cog < AC_MARGIN_M;
    let is_asymmetric_ablation = saw_asymmetric;
    let is_plasma_density_spike = !saw_asymmetric && saw_spike;
    let is_cog_past_ac = !saw_asymmetric && !saw_spike && (saw_erosion || cog_inverted);
    let is_attitude_departure =
        !saw_asymmetric && !saw_spike && !is_cog_past_ac && departure;
    let is_live = !saw_asymmetric && !saw_spike && !is_cog_past_ac && !departure;

    proof.feed_f64(min_cog as f64);
    proof.feed_str(if is_cog_past_ac {
        "COG_PAST_AC"
    } else if is_asymmetric_ablation {
        "ASYMMETRIC_ABLATION"
    } else if is_plasma_density_spike {
        "PLASMA_DENSITY_SPIKE"
    } else if is_attitude_departure {
        "ATTITUDE_DEPARTURE"
    } else {
        "LIVE"
    });

    Run {
        id: index as u32,
        short_id,
        scenario: scenario_name.to_string(),
        mach_start: (mach_start * 100.0).round() / 100.0,
        min_cog_offset_m: (min_cog as f64 * 1000.0).round() / 1000.0,
        mass_final_kg: (true_mass as f64 * 10.0).round() / 10.0,
        max_surface_temp_k: (max_temp as f64).round(),
        max_attitude_error_deg: (max_att_err as f64 * 10.0).round() / 10.0,
        aeroshell_thickness_m: (aeroshell_thickness as f64 * 10000.0).round() / 10000.0,
        is_asymmetric_ablation,
        is_cog_past_ac,
        is_plasma_density_spike,
        is_live,
        is_attitude_departure,
        proof_hash: proof.seal(),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(DEFAULT_N);
    let out = args
        .iter()
        .position(|a| a == "--parquet")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| {
            format!(
                "{}/../../data/exports/sovereign/hgv_plasma.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    let scenario = args
        .iter()
        .position(|a| a == "--scenario")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "envelope".to_string());

    println!("====================================================================");
    println!("  G^G: HGV ABLATION  (Sutton-Graves, CoG, 1 kHz Euler — not Reflex)");
    println!("  n={n}  scenario={scenario}  AC={AC_MARGIN_M} m");
    println!("====================================================================\n");

    let t0 = Instant::now();
    let base_seed = 0xDECD_BAFC_A0FE_1337u64;
    let seed_multiplier = 0x9E37_79B1_85EB_CA87u64;
    let rows: Vec<Run> = (0..n)
        .into_par_iter()
        .map(|i| {
            let seed = base_seed ^ (i as u64).wrapping_mul(seed_multiplier);
            let scenario_for_traj = if scenario == "envelope" || scenario == "sweep" {
                match i % 4 {
                    0 => "nominal",
                    1 => "asymmetric_ablation",
                    2 => "aeroshell_erosion",
                    3 => "plasma_density_spike",
                    _ => "nominal",
                }
            } else {
                scenario.as_str()
            };
            run_one(i, seed, scenario_for_traj)
        })
        .collect();

    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let unique: std::collections::HashSet<&String> = proofs.iter().collect();
    assert_eq!(unique.len(), n, "proof_hash must be unique per trajectory");
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("scenario", DataType::Utf8, false),
        Field::new("mach_start", DataType::Float64, false),
        Field::new("min_cog_offset_m", DataType::Float64, false),
        Field::new("mass_final_kg", DataType::Float64, false),
        Field::new("max_surface_temp_k", DataType::Float64, false),
        Field::new("max_attitude_error_deg", DataType::Float64, false),
        Field::new("aeroshell_thickness_m", DataType::Float64, false),
        Field::new("is_asymmetric_ablation", DataType::Boolean, false),
        Field::new("is_cog_past_ac", DataType::Boolean, false),
        Field::new("is_plasma_density_spike", DataType::Boolean, false),
        Field::new("is_live", DataType::Boolean, false),
        Field::new("is_attitude_departure", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(
                rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter().map(|r| r.scenario.as_str()).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.mach_start).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.min_cog_offset_m).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.mass_final_kg).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.max_surface_temp_k).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter()
                    .map(|r| r.max_attitude_error_deg)
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter()
                    .map(|r| r.aeroshell_thickness_m)
                    .collect::<Vec<_>>(),
            )),
            Arc::new(BooleanArray::from(
                rows.iter()
                    .map(|r| r.is_asymmetric_ablation)
                    .collect::<Vec<_>>(),
            )),
            Arc::new(BooleanArray::from(
                rows.iter().map(|r| r.is_cog_past_ac).collect::<Vec<_>>(),
            )),
            Arc::new(BooleanArray::from(
                rows.iter()
                    .map(|r| r.is_plasma_density_spike)
                    .collect::<Vec<_>>(),
            )),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.is_live).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(
                rows.iter()
                    .map(|r| r.is_attitude_departure)
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>(),
            )),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G HGV ablation dual-regime v1.0");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();

    let nf = n as f64;
    let live = rows.iter().filter(|r| r.is_live).count();
    let asym = rows.iter().filter(|r| r.is_asymmetric_ablation).count();
    let cog = rows.iter().filter(|r| r.is_cog_past_ac).count();
    let spike = rows.iter().filter(|r| r.is_plasma_density_spike).count();
    let att = rows.iter().filter(|r| r.is_attitude_departure).count();
    let exclusive = live + asym + cog + spike + att;
    println!(
        "  exclusive: live {live} ({:.1}%)  asymmetric {asym} ({:.1}%)  CoG-past-AC {cog} ({:.1}%)  spike {spike} ({:.1}%)  attitude-departure {att} ({:.1}%)  sum {exclusive}",
        100.0 * live as f64 / nf,
        100.0 * asym as f64 / nf,
        100.0 * cog as f64 / nf,
        100.0 * spike as f64 / nf,
        100.0 * att as f64 / nf
    );
    println!("  seal {seal}");
    println!("  parquet {out}");
    println!("  {:?}", t0.elapsed());
}
