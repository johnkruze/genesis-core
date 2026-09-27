//! Cardiac Tissue Verification Monte Carlo
//!
//! Physical Verification Layer for AI-driven cardiac surgery.
//! 100,000 trajectories of an AI robotic surgeon manipulating delicate
//! cardiac tissue. Maxwell viscoelastic ODE (`maxwell_jaw_step`) governs
//! tissue response. Cardiac-specific stiffness/yield boundaries.
//!
//! Sweep axes:
//! - AI planner grip velocity (jaw close rate, m/s)
//! - Slip noise (stochastic perturbation on the jaw position)
//! - Tissue degradation over time (perfusion loss → stiffness decay)
//!
//! The simulation checks when the AI's applied force causes cellular
//! rupture (micro-tearing) based on the Maxwell yield surface.
//!
//! Output: Snappy-compressed Parquet with SHA-256 proof chain in footer.
//!
//! Architecture note: This binary is G^G — the offline Simulator.
//! It tests the safety rules that would enforce at dt = 0.001 s
//! on a physical robot. We prove the failure boundaries computationally
//! so the edge brain never encounters an uncharted regime.

use genesis_core::output;
use genesis_core::physics::dexterous::{
    evaluate_surgical_grasp_dynamics, maxwell_jaw_step, C_SurgicalTissueAuditor,
};
use genesis_core::proof::{self, ProofChain};
use genesis_core::rng::Rng;

use std::sync::Arc;
use std::time::Instant;

use arrow::array::{BooleanArray, Float64Array, StringArray, UInt32Array, UInt64Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::ArrowWriter;

// ─── CARDIAC TISSUE CONSTITUTIVE PARAMETERS ───────────────────────────
//
// Human myocardium is significantly softer than bowel/vessel tissue and
// far softer than bone/tendon. Published stiffness ranges:
// - Passive myocardium: 20–80 kPa (Holzapfel & Ogden, 2009)
// - Active contraction adds ~40 kPa
//
// We express stiffness in N/m for the 1D Maxwell element jaw model.
// Cardiac tissue type_id = 3 (extending the existing 0/1/2 registry).

/// Tissue type ID for cardiac myocardium in the dexterous registry.
const CARDIAC_TISSUE_ID: u32 = 3;

/// Stiffness of passive cardiac myocardium in the Maxwell jaw element (N/m).
/// Softer than liver/spleen (800 N/m) — myocardium is ~350 N/m.
/// This is the spring constant k in: dF = k·dx − F·dt/τ
const CARDIAC_STIFFNESS_NPM: f32 = 350.0;

/// Maxwell relaxation time constant for cardiac tissue (seconds).
/// Myocardium has significant viscoelastic relaxation — longer τ than
/// liver (~0.05s) due to collagen crosslinking in the extracellular matrix.
const CARDIAC_TAU_S: f32 = 0.12;

/// Cellular rupture threshold for cardiac tissue (Newtons).
/// Micro-tearing onset in myocardium. Literature reports UTS ~100–300 kPa
/// for myocardium. Mapped to the 1D jaw element, this corresponds to
/// ~0.6 N before micro-tears propagate irreversibly.
const CARDIAC_YIELD_N: f32 = 0.60;

/// Hard tearing limit — complete structural failure (Newtons).
/// Beyond this, the tissue is macroscopically torn. The auditor gates
/// at this value. Below liver/spleen (1.2 N) — cardiac muscle is fragile.
const CARDIAC_TEAR_LIMIT_N: f32 = 0.85;

/// Simulation timestep (seconds). Matches 's dt = 0.001 s loop.
const DT: f32 = 0.001;

/// Number of integration steps per trajectory.
/// 500 steps = 0.5 s of simulated surgery time — a single grasp-and-hold.
const STEPS_PER_TRAJ: usize = 500;

/// Proof chain cadence: feed state every N steps.
const PROOF_CADENCE: usize = 25;

// ─── OUTCOME CLASSIFICATION ───────────────────────────────────────────

/// Exclusive outcome for each trajectory. Rupture is terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// Tissue survived — force stayed below yield throughout.
    TissueIntact,
    /// Cellular micro-tearing detected: force exceeded CARDIAC_YIELD_N
    /// but tissue was not macroscopically torn.
    CellularMicroTear,
    /// Macroscopic rupture: force exceeded CARDIAC_TEAR_LIMIT_N.
    /// Terminal event — tissue is destroyed.
    MacroscopicRupture,
    /// Cable/instrument slip fault detected while tissue was still alive.
    InstrumentSlipFault,
}

impl Outcome {
    fn label(self) -> &'static str {
        match self {
            Outcome::TissueIntact => "TISSUE_INTACT",
            Outcome::CellularMicroTear => "CELLULAR_MICROTEAR",
            Outcome::MacroscopicRupture => "MACROSCOPIC_RUPTURE",
            Outcome::InstrumentSlipFault => "INSTRUMENT_SLIP_FAULT",
        }
    }
}

// ─── ROW STRUCT ───────────────────────────────────────────────────────

struct CardiacRow {
    trajectory_id: String,
    run_index: u32,
    // Sweep parameters
    grip_velocity_mps: f64,
    slip_noise_sigma: f64,
    degradation_rate: f64,
    // Tissue parameters
    initial_stiffness_npm: f64,
    final_stiffness_npm: f64,
    tau_s: f64,
    yield_n: f64,
    tear_limit_n: f64,
    // Trajectory results
    peak_force_n: f64,
    final_force_n: f64,
    peak_displacement_m: f64,
    final_displacement_m: f64,
    accumulated_energy_j: f64,
    step_of_first_yield: u64,
    step_of_rupture: u64,
    // Maxwell viscoelastic state at end
    maxwell_force_n: f64,
    // Auditor flags
    tissue_overstress: bool,
    viscoelastic_rupture: bool,
    cable_slip_fault: bool,
    cellular_microtear: bool,
    macroscopic_rupture: bool,
    // Classified outcome
    outcome: String,
    // Proof
    proof_hash: String,
}

// ─── SINGLE TRAJECTORY ───────────────────────────────────────────────

fn run_one(index: u32, rng: &mut Rng) -> CardiacRow {
    let short_id = output::short_id(rng);
    let trajectory_id = format!("cardiac_{}", short_id);

    // ── Sweep axis 1: AI planner grip velocity (m/s) ──────────────
    // Range: 0.02 m/s (cautious) to 0.25 m/s (aggressive).
    // This is the jaw close rate — how fast the robotic gripper approaches.
    let grip_velocity = rng.range(0.02, 0.25) as f32;

    // ── Sweep axis 2: Slip noise sigma (m) ────────────────────────
    // Gaussian noise added to jaw position each step.
    // Models cable backlash, vibration, and sensor noise.
    // Range: 0 (perfect) to 0.0008 m (0.8 mm jitter — severe).
    let slip_noise_sigma = rng.range(0.0, 0.0008);

    // ── Sweep axis 3: Tissue degradation rate ─────────────────────
    // Stiffness decays exponentially: k(t) = k₀ · exp(-λt).
    // Models ischemia / loss of perfusion during surgery.
    // λ range: 0 (healthy) to 6.0 (severe ischemia, rapid degradation).
    let degradation_rate = rng.range(0.0, 6.0);

    // ── Derived tissue parameters ─────────────────────────────────
    // Small per-trajectory variation in baseline stiffness (±15%)
    let k0 = CARDIAC_STIFFNESS_NPM * rng.range(0.85, 1.15) as f32;
    let tau = CARDIAC_TAU_S * rng.range(0.85, 1.15) as f32;
    // Yield varies with collagen density (±12%)
    let yield_force = CARDIAC_YIELD_N * rng.range(0.88, 1.12) as f32;
    let tear_limit = CARDIAC_TEAR_LIMIT_N * rng.range(0.90, 1.10) as f32;

    // ── Integration state ─────────────────────────────────────────
    let mut displacement: f32 = 0.0;
    let mut last_displacement: f32 = displacement;
    let mut maxwell_force: f32 = 0.0;
    let mut last_force: f32 = maxwell_force;
    let mut accumulated_energy: f32 = 0.0;
    let mut peak_force: f32 = 0.0;
    let mut peak_displacement: f32 = 0.0;

    // Outcome tracking
    let mut first_yield_step: u64 = 0;
    let mut rupture_step: u64 = 0;
    let mut any_overstress = false;
    let mut any_visco_rupture = false;
    let mut any_cable_slip = false;
    let mut any_microtear = false;
    let mut any_macro_rupture = false;

    // ── Proof chain ───────────────────────────────────────────────
    let mut proof = ProofChain::new();
    proof.seed(&index.to_le_bytes());
    proof.feed_f64(grip_velocity as f64);
    proof.feed_f64(slip_noise_sigma);
    proof.feed_f64(degradation_rate);
    proof.feed_f64(k0 as f64);

    // ── Time integration loop ─────────────────────────────────────
    for step in 0..STEPS_PER_TRAJ {
        let t = step as f32 * DT;

        // Time-varying stiffness: ischemic degradation
        let k_t = k0 * (-(degradation_rate as f32) * t).exp();

        // Jaw displacement: velocity-driven approach + slip noise
        last_displacement = displacement;
        let noise = rng.gaussian(0.0, slip_noise_sigma) as f32;
        displacement += grip_velocity * DT + noise;
        displacement = displacement.max(0.0); // jaw can't go negative

        let dx = displacement - last_displacement;

        // ── Maxwell viscoelastic step ─────────────────────────────
        // dF = k(t)·dx − F·dt/τ
        // This is the core tissue constitutive law.
        last_force = maxwell_force;
        maxwell_force = maxwell_jaw_step(maxwell_force, dx, k_t, tau, DT);

        // Energy accumulation (trapezoidal integration)
        accumulated_energy += 0.5 * (last_force + maxwell_force) * dx.abs();

        // Track peaks
        if maxwell_force > peak_force {
            peak_force = maxwell_force;
        }
        if displacement > peak_displacement {
            peak_displacement = displacement;
        }

        // ── Cellular micro-tear check (Maxwell yield surface) ─────
        // The yield degrades with ischemia too: damaged tissue tears sooner.
        let effective_yield = yield_force * (-(degradation_rate as f32) * t * 0.3).exp();
        if maxwell_force > effective_yield && !any_microtear {
            any_microtear = true;
            if first_yield_step == 0 {
                first_yield_step = step as u64;
            }
        }

        // ── Macroscopic rupture check ─────────────────────────────
        let effective_tear = tear_limit * (-(degradation_rate as f32) * t * 0.25).exp();
        if maxwell_force > effective_tear && !any_macro_rupture {
            any_macro_rupture = true;
            rupture_step = step as u64;
            // Post-rupture: tissue can no longer bear load.
            // Force drops catastrophically.
            maxwell_force *= 0.05;
        }

        // If tissue is ruptured, subsequent steps see near-zero stiffness
        if any_macro_rupture && step as u64 > rupture_step {
            maxwell_force *= 0.90; // exponential decay of residual
        }

        // ── Surgical auditor (ztp rule check) ─────────────────────
        // We use the existing auditor infrastructure to cross-validate.
        // For cardiac tissue, the auditor's tissue_type_id=3 falls to
        // the default branch (1.0 N limit), which is close to our
        // CARDIAC_TEAR_LIMIT_N. This is the safety gate.
        let auditor = C_SurgicalTissueAuditor {
            tissue_type_id: CARDIAC_TISSUE_ID,
            max_tearing_force_n: tear_limit,
            measured_displacement_m: displacement,
            measured_force_n: maxwell_force,
            relaxation_tau: tau,
            last_displacement_m: last_displacement,
            last_force_n: last_force,
            accumulated_energy_j: accumulated_energy,
        };
        let res = evaluate_surgical_grasp_dynamics(&auditor, DT);
        if res.tissue_overstress_detected {
            any_overstress = true;
        }
        if res.viscoelastic_rupture_detected {
            any_visco_rupture = true;
        }
        if res.cable_slip_fault {
            any_cable_slip = true;
        }

        // Proof cadence
        if step % PROOF_CADENCE == 0 {
            proof.feed_f64(maxwell_force as f64);
            proof.feed_f64(displacement as f64);
            proof.feed_f64(k_t as f64);
        }
    }

    // ── Classify exclusive outcome ────────────────────────────────
    // Priority: macroscopic rupture > micro-tear > instrument slip > intact
    let outcome = if any_macro_rupture {
        Outcome::MacroscopicRupture
    } else if any_microtear {
        Outcome::CellularMicroTear
    } else if any_cable_slip {
        Outcome::InstrumentSlipFault
    } else {
        Outcome::TissueIntact
    };

    proof.feed_str(outcome.label());

    let final_k = k0 * (-(degradation_rate as f32) * (STEPS_PER_TRAJ as f32 * DT)).exp();

    CardiacRow {
        trajectory_id,
        run_index: index,
        grip_velocity_mps: grip_velocity as f64,
        slip_noise_sigma,
        degradation_rate,
        initial_stiffness_npm: k0 as f64,
        final_stiffness_npm: final_k as f64,
        tau_s: tau as f64,
        yield_n: yield_force as f64,
        tear_limit_n: tear_limit as f64,
        peak_force_n: peak_force as f64,
        final_force_n: maxwell_force as f64,
        peak_displacement_m: peak_displacement as f64,
        final_displacement_m: displacement as f64,
        accumulated_energy_j: accumulated_energy as f64,
        step_of_first_yield: first_yield_step,
        step_of_rupture: rupture_step,
        maxwell_force_n: maxwell_force as f64,
        tissue_overstress: any_overstress,
        viscoelastic_rupture: any_visco_rupture,
        cable_slip_fault: any_cable_slip && !any_macro_rupture,
        cellular_microtear: any_microtear && !any_macro_rupture,
        macroscopic_rupture: any_macro_rupture,
        outcome: outcome.label().to_string(),
        proof_hash: proof.seal(),
    }
}

// ─── MAIN ─────────────────────────────────────────────────────────────

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(100_000);
    let out = args
        .iter()
        .position(|a| a == "--parquet")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "../../data/exports/cardiac_tissue_verification.parquet".to_string());

    println!("════════════════════════════════════════════════════════════════════");
    println!(" G^G: CARDIAC TISSUE VERIFICATION MONTE CARLO");
    println!(" Physical Verification Layer for AI-driven cardiac surgery");
    println!(" n={} tissue: cardiac myocardium", n);
    println!(
        " stiffness: {} N/m yield: {} N tear: {} N τ: {} s",
        CARDIAC_STIFFNESS_NPM, CARDIAC_YIELD_N, CARDIAC_TEAR_LIMIT_N, CARDIAC_TAU_S
    );
    println!(" sweep: grip_velocity × slip_noise × degradation_rate");
    println!(
        " steps/traj: {} dt: {} s sim_time: {} s",
        STEPS_PER_TRAJ,
        DT,
        STEPS_PER_TRAJ as f32 * DT
    );
    println!("════════════════════════════════════════════════════════════════════\n");

    // Deterministic seed for reproducibility
    let mut rng = Rng::new(0xCAD1_AC_7155_0E_42);

    let t0 = Instant::now();
    let mut rows = Vec::with_capacity(n as usize);
    for i in 0..n {
        rows.push(run_one(i, &mut rng));
        if i > 0 && i % 10_000 == 0 {
            let elapsed = t0.elapsed().as_secs_f64();
            let rate = i as f64 / elapsed;
            println!("... {i}/{n} ({rate:.0} traj/s)");
        }
    }
    let elapsed = t0.elapsed();

    // ── Run-level cryptographic seal ──────────────────────────────
    let proof_hashes: Vec<_> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let run_seal = proof::seal_run(&proof_hashes);

    // ── Parquet export ────────────────────────────────────────────
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::Utf8, false),
        Field::new("run_index", DataType::UInt32, false),
        // Sweep parameters
        Field::new("grip_velocity_mps", DataType::Float64, false),
        Field::new("slip_noise_sigma", DataType::Float64, false),
        Field::new("degradation_rate", DataType::Float64, false),
        // Tissue parameters
        Field::new("initial_stiffness_npm", DataType::Float64, false),
        Field::new("final_stiffness_npm", DataType::Float64, false),
        Field::new("tau_s", DataType::Float64, false),
        Field::new("yield_n", DataType::Float64, false),
        Field::new("tear_limit_n", DataType::Float64, false),
        // Trajectory results
        Field::new("peak_force_n", DataType::Float64, false),
        Field::new("final_force_n", DataType::Float64, false),
        Field::new("peak_displacement_m", DataType::Float64, false),
        Field::new("final_displacement_m", DataType::Float64, false),
        Field::new("accumulated_energy_j", DataType::Float64, false),
        Field::new("step_of_first_yield", DataType::UInt64, false),
        Field::new("step_of_rupture", DataType::UInt64, false),
        Field::new("maxwell_force_n", DataType::Float64, false),
        // Auditor flags
        Field::new("tissue_overstress", DataType::Boolean, false),
        Field::new("viscoelastic_rupture", DataType::Boolean, false),
        Field::new("cable_slip_fault", DataType::Boolean, false),
        Field::new("cellular_microtear", DataType::Boolean, false),
        Field::new("macroscopic_rupture", DataType::Boolean, false),
        // Outcome
        Field::new("outcome", DataType::Utf8, false),
        // Proof
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(
                rows.iter()
                    .map(|r| Some(r.trajectory_id.as_str()))
                    .collect::<StringArray>(),
            ),
            Arc::new(
                rows.iter()
                    .map(|r| Some(r.run_index))
                    .collect::<UInt32Array>(),
            ),
            Arc::new(
                rows.iter()
                    .map(|r| Some(r.grip_velocity_mps))
                    .collect::<Float64Array>(),
            ),
            Arc::new(
                rows.iter()
                    .map(|r| Some(r.slip_noise_sigma))
                    .collect::<Float64Array>(),
            ),
            Arc::new(
                rows.iter()
                    .map(|r| Some(r.degradation_rate))
                    .collect::<Float64Array>(),
            ),
            Arc::new(
                rows.iter()
                    .map(|r| Some(r.initial_stiffness_npm))
                    .collect::<Float64Array>(),
            ),
            Arc::new(
                rows.iter()
                    .map(|r| Some(r.final_stiffness_npm))
                    .collect::<Float64Array>(),
            ),
            Arc::new(rows.iter().map(|r| Some(r.tau_s)).collect::<Float64Array>()),
            Arc::new(
                rows.iter()
                    .map(|r| Some(r.yield_n))
                    .collect::<Float64Array>(),
            ),
            Arc::new(
                rows.iter()
                    .map(|r| Some(r.tear_limit_n))
                    .collect::<Float64Array>(),
            ),
            Arc::new(
                rows.iter()
                    .map(|r| Some(r.peak_force_n))
                    .collect::<Float64Array>(),
            ),
            Arc::new(
                rows.iter()
                    .map(|r| Some(r.final_force_n))
                    .collect::<Float64Array>(),
            ),
            Arc::new(
                rows.iter()
                    .map(|r| Some(r.peak_displacement_m))
                    .collect::<Float64Array>(),
            ),
            Arc::new(
                rows.iter()
                    .map(|r| Some(r.final_displacement_m))
                    .collect::<Float64Array>(),
            ),
            Arc::new(
                rows.iter()
                    .map(|r| Some(r.accumulated_energy_j))
                    .collect::<Float64Array>(),
            ),
            Arc::new(
                rows.iter()
                    .map(|r| Some(r.step_of_first_yield))
                    .collect::<UInt64Array>(),
            ),
            Arc::new(
                rows.iter()
                    .map(|r| Some(r.step_of_rupture))
                    .collect::<UInt64Array>(),
            ),
            Arc::new(
                rows.iter()
                    .map(|r| Some(r.maxwell_force_n))
                    .collect::<Float64Array>(),
            ),
            Arc::new(
                rows.iter()
                    .map(|r| Some(r.tissue_overstress))
                    .collect::<BooleanArray>(),
            ),
            Arc::new(
                rows.iter()
                    .map(|r| Some(r.viscoelastic_rupture))
                    .collect::<BooleanArray>(),
            ),
            Arc::new(
                rows.iter()
                    .map(|r| Some(r.cable_slip_fault))
                    .collect::<BooleanArray>(),
            ),
            Arc::new(
                rows.iter()
                    .map(|r| Some(r.cellular_microtear))
                    .collect::<BooleanArray>(),
            ),
            Arc::new(
                rows.iter()
                    .map(|r| Some(r.macroscopic_rupture))
                    .collect::<BooleanArray>(),
            ),
            Arc::new(
                rows.iter()
                    .map(|r| Some(r.outcome.as_str()))
                    .collect::<StringArray>(),
            ),
            Arc::new(
                rows.iter()
                    .map(|r| Some(r.proof_hash.as_str()))
                    .collect::<StringArray>(),
            ),
        ],
    )
    .expect("RecordBatch construction failed — schema/column count mismatch");

    let props = output::parquet_receipt_properties(
        &run_seal,
        "G^G cardiac tissue verification MC v1.0 — maxwell_jaw_step viscoelastic ODE",
    );
    let file = std::fs::File::create(&out).expect("failed to create parquet file");
    let mut writer = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();

    // ── Summary statistics ────────────────────────────────────────
    let intact = rows.iter().filter(|r| r.outcome == "TISSUE_INTACT").count();
    let microtear = rows
        .iter()
        .filter(|r| r.outcome == "CELLULAR_MICROTEAR")
        .count();
    let rupture = rows
        .iter()
        .filter(|r| r.outcome == "MACROSCOPIC_RUPTURE")
        .count();
    let slip = rows
        .iter()
        .filter(|r| r.outcome == "INSTRUMENT_SLIP_FAULT")
        .count();
    let n_f = n as f64;

    assert_eq!(
        intact + microtear + rupture + slip,
        n as usize,
        "Outcome partition must be exhaustive and exclusive"
    );

    println!("════════════════════════════════════════════════════════════════════");
    println!(" RESULTS (n={})", n);
    println!("────────────────────────────────────────────────────────────────────");
    println!(
        " TISSUE_INTACT {:>7} ({:>5.1}%)",
        intact,
        100.0 * intact as f64 / n_f
    );
    println!(
        " CELLULAR_MICROTEAR {:>7} ({:>5.1}%)",
        microtear,
        100.0 * microtear as f64 / n_f
    );
    println!(
        " MACROSCOPIC_RUPTURE {:>7} ({:>5.1}%)",
        rupture,
        100.0 * rupture as f64 / n_f
    );
    println!(
        " INSTRUMENT_SLIP {:>7} ({:>5.1}%)",
        slip,
        100.0 * slip as f64 / n_f
    );
    println!("────────────────────────────────────────────────────────────────────");
    println!(" seal {}", run_seal);
    println!(" parquet {}", out);
    println!(" time {:?}", elapsed);
    println!(
        " rate {:.0} trajectories/sec",
        n as f64 / elapsed.as_secs_f64()
    );
    println!("════════════════════════════════════════════════════════════════════");
}
