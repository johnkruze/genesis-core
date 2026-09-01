//! Tesseract nonlinear IMU. Duffing drive + Coriolis scale + hold.
//! Clock: 100 Hz, 0.60 s. dt = 400 µs (25 samples / period).
//! Mix: is_nonlinear_drive (tether + α ≠ 0 vs linear plateau) vs
//! is_bias_floor_broken (½ |b| t² ≥ lock-loss). Independent.
//! Bias is a continuous draw that straddles lock-loss — no rng.chance
//! on the hard column. The well runs under that bias: step(a + b, …).
//! Coriolis column is sense-axis drive_v, not well ẋ.
//! Hold law is the same on both mixes.
//! Organ: physics::tesseract.

use genesis_core::output;
use genesis_core::physics::tesseract::{
    bias_floor_m_or_rad, control_hold_attractor, coriolis_scale_n, BiasKind, Tesseract, HOLD_KD,
    HOLD_KP_SUFFICIENT, HOLD_U_SAT_M_S2, V_LINEAR_M_S, V_TETHER_M_S,
};
use genesis_core::proof::{self, ProofChain};
use genesis_core::rng::Rng;
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{BooleanArray, Float64Array, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::ArrowWriter;

const DEFAULT_N: usize = 2500;
const FN_HZ: f64 = 100.0;
const DT: f64 = 0.0004;
const HORIZON_S: f64 = 0.60;
const MASS_KG: f64 = 1.0e-7;
const LOCK_LOSS_M: f64 = 0.05;
const FOLD_M: f64 = 0.006;
/// ½ |b| t² ≥ 0.05 m at t = 0.60 s  ⇒  |b| ≥ 0.05 / 0.18 ≈ 0.278 m/s².
/// Single continuous range that straddles that cliff.
const BIAS_LO: f64 = 0.05;
const BIAS_HI: f64 = 0.50;

#[derive(Debug, Serialize)]
struct Run {
    id: u32,
    short_id: String,
    alpha: f64,
    drive_velocity_m_s: f64,
    peak_disp_m: f64,
    scale_n: f64,
    bias_floor_m: f64,
    residual_max: f64,
    is_nonlinear_drive: bool,
    is_bias_floor_broken: bool,
    proof_hash: String,
}

fn pearson(xs: &[f64], ys: &[f64]) -> f64 {
    let n = xs.len() as f64;
    if n < 3.0 {
        return 0.0;
    }
    let mx = xs.iter().sum::<f64>() / n;
    let my = ys.iter().sum::<f64>() / n;
    let mut cov = 0.0;
    let mut vx = 0.0;
    let mut vy = 0.0;
    for i in 0..xs.len() {
        let dx = xs[i] - mx;
        let dy = ys[i] - my;
        cov += dx * dy;
        vx += dx * dx;
        vy += dy * dy;
    }
    if vx > 0.0 && vy > 0.0 {
        cov / (vx.sqrt() * vy.sqrt())
    } else {
        0.0
    }
}

fn median(xs: &mut [f64]) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = xs.len();
    if n % 2 == 1 {
        xs[n / 2]
    } else {
        0.5 * (xs[n / 2 - 1] + xs[n / 2])
    }
}

fn run_one(id: u32, rng: &mut Rng) -> Run {
    let short_id = output::short_id(rng);
    let nonlinear = rng.chance(0.36);

    let alpha = if nonlinear {
        rng.range(4.0e8, 2.4e9)
    } else {
        0.0
    };
    let drive_v = if nonlinear {
        rng.range(V_TETHER_M_S * 0.85, V_TETHER_M_S * 1.15)
    } else {
        rng.range(V_LINEAR_M_S * 0.7, V_LINEAR_M_S * 1.3)
    };
    let zeta = rng.range(0.02, 0.08);
    let a_drive = rng.range(25.0, 80.0);
    let omega_ext = rng.range(0.15, 1.2);
    let a_cmd = rng.range(0.008, 0.018);
    let bias = rng.range(BIAS_LO, BIAS_HI);

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(alpha);
    proof.feed_f64(drive_v);
    proof.feed_f64(bias);
    proof.feed_f64(a_drive);
    proof.feed_f64(omega_ext);

    let mut tes = Tesseract::new(FN_HZ, zeta, MASS_KG, alpha);
    tes.fold_m = FOLD_M;

    let w = tes.omega_n_rad_s;
    let steps = (HORIZON_S / DT) as usize;
    let mut peak = 0.0_f64;
    let mut residual_max = 0.0_f64;
    let scale_n = coriolis_scale_n(MASS_KG, drive_v, omega_ext);
    for k in 0..steps {
        let t = k as f64 * DT;
        let x_cmd = a_cmd * (w * t).sin();
        let v_cmd = a_cmd * w * (w * t).cos();
        tes.control_u = control_hold_attractor(
            tes.displacement_m,
            tes.velocity_m_s,
            x_cmd,
            v_cmd,
            HOLD_KP_SUFFICIENT,
            HOLD_KD,
            HOLD_U_SAT_M_S2,
        );
        let a_in = a_drive * (w * t).sin();
        let tick = tes.step(a_in + bias, omega_ext, DT);
        peak = peak.max(tick.displacement_m.abs());
        residual_max = residual_max.max(tick.residual);
        if k % 50 == 0 {
            proof.feed_f64(tick.displacement_m);
        }
    }

    let floor = bias_floor_m_or_rad(bias, HORIZON_S, BiasKind::Accel);
    let broken = floor >= LOCK_LOSS_M;
    proof.feed_f64(peak);
    proof.feed_f64(floor);
    proof.feed_str(if nonlinear && broken {
        "TETHER_BIAS_CLIFF"
    } else if nonlinear {
        "TETHER_IN_BUDGET"
    } else if broken {
        "PLATEAU_BIAS_CLIFF"
    } else {
        "PLATEAU_IN_BUDGET"
    });

    Run {
        id,
        short_id,
        alpha,
        drive_velocity_m_s: (drive_v * 100.0).round() / 100.0,
        peak_disp_m: peak,
        scale_n,
        bias_floor_m: floor,
        residual_max,
        is_nonlinear_drive: nonlinear,
        is_bias_floor_broken: broken,
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
                "{}/../../data/exports/sovereign/tesseract_monte_carlo.parquet",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    println!("====================================================================");
    println!("  G^G: TESSERACT  (Duffing + Coriolis, 100 Hz, 0.60 s)  v1.1");
    println!("  n={n}  lock-loss {LOCK_LOSS_M} m  dt={DT} s");
    println!("====================================================================\n");
    let t0 = Instant::now();
    let mut rng = Rng::new(0x7E55_E4A7);
    let rows: Vec<Run> = (0..n as u32).map(|i| run_one(i, &mut rng)).collect();
    let proofs: Vec<String> = rows.iter().map(|r| r.proof_hash.clone()).collect();
    let unique = {
        let mut s = std::collections::HashSet::new();
        proofs.iter().filter(|h| s.insert(h.as_str())).count()
    };
    assert_eq!(unique, n, "proof_hash must be unique per row");
    let seal = proof::seal_run(&proofs);
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::UInt32, false),
        Field::new("short_id", DataType::Utf8, false),
        Field::new("alpha", DataType::Float64, false),
        Field::new("drive_velocity_m_s", DataType::Float64, false),
        Field::new("peak_disp_m", DataType::Float64, false),
        Field::new("scale_n", DataType::Float64, false),
        Field::new("bias_floor_m", DataType::Float64, false),
        Field::new("residual_max", DataType::Float64, false),
        Field::new("is_nonlinear_drive", DataType::Boolean, false),
        Field::new("is_bias_floor_broken", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>())),
            Arc::new(StringArray::from(
                rows.iter().map(|r| r.short_id.as_str()).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.alpha).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.drive_velocity_m_s).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.peak_disp_m).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.scale_n).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.bias_floor_m).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.residual_max).collect::<Vec<_>>(),
            )),
            Arc::new(BooleanArray::from(
                rows.iter().map(|r| r.is_nonlinear_drive).collect::<Vec<_>>(),
            )),
            Arc::new(BooleanArray::from(
                rows.iter()
                    .map(|r| r.is_bias_floor_broken)
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter().map(|r| r.proof_hash.as_str()).collect::<Vec<_>>(),
            )),
        ],
    )
    .expect("batch");
    let file = std::fs::File::create(&out).unwrap();
    let props = output::parquet_receipt_properties(&seal, "G^G tesseract Duffing IMU dual-regime v1.1");
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let nf = n as f64;
    let a = rows.iter().filter(|r| r.is_nonlinear_drive).count();
    let b = rows.iter().filter(|r| r.is_bias_floor_broken).count();
    let both = rows
        .iter()
        .filter(|r| r.is_nonlinear_drive && r.is_bias_floor_broken)
        .count();
    let drive_v: Vec<f64> = rows.iter().map(|r| r.drive_velocity_m_s).collect();
    let scale: Vec<f64> = rows.iter().map(|r| r.scale_n).collect();
    let floor: Vec<f64> = rows.iter().map(|r| r.bias_floor_m).collect();
    let peak: Vec<f64> = rows.iter().map(|r| r.peak_disp_m).collect();
    let resid: Vec<f64> = rows.iter().map(|r| r.residual_max).collect();
    let alpha: Vec<f64> = rows.iter().map(|r| r.alpha).collect();
    let mut scale_nl: Vec<f64> = rows
        .iter()
        .filter(|r| r.is_nonlinear_drive)
        .map(|r| r.scale_n)
        .collect();
    let mut scale_lin: Vec<f64> = rows
        .iter()
        .filter(|r| !r.is_nonlinear_drive)
        .map(|r| r.scale_n)
        .collect();
    let med_nl = median(&mut scale_nl);
    let med_lin = median(&mut scale_lin);
    let scale_ratio = if med_lin > 0.0 { med_nl / med_lin } else { 0.0 };
    println!(
        "  nonlinear {a} ({:.1}%)  bias_floor_broken {b} ({:.1}%)  both {both} ({:.1}%)",
        100.0 * a as f64 / nf,
        100.0 * b as f64 / nf,
        100.0 * both as f64 / nf
    );
    println!(
        "  corr(drive_v, scale_n)={:.3}  scale median tether/plateau={:.1}×",
        pearson(&drive_v, &scale),
        scale_ratio
    );
    println!(
        "  corr(bias_floor, peak)={:.3}  corr(bias_floor, residual)={:.3}  corr(alpha, peak)={:.3}",
        pearson(&floor, &peak),
        pearson(&floor, &resid),
        pearson(&alpha, &peak)
    );
    println!("  unique proofs {unique}/{n}");
    println!("  seal {seal}\n  parquet {out}\n  {:?}", t0.elapsed());
}
