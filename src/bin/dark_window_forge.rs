//! Dark Window Forge — invert a shear-catch, print the Sheet Music, stamp `.soma.bin`.
//!
//! Survival horizon: a hand about to take a kinetic shear load (slipping heavy
//! object). The Cultivator shoots [`GraspHoldAttractor`] on the 45 N grasp plant
//! for a dt = 0.001 s catch window, sustains the last hold to 5–10 s, and compresses
//! each hold into a last-state hand frame (`pack_hand`, body 31). Peek is the
//! last 64 bytes.
//!
//! ```text
//! cargo run --release --bin dark_window_forge
//! cargo run --release --bin dark_window_forge -- 8.0 path/to/hand_dark_window.soma.bin
//! ```

use genesis_core::last_state::{BODY_HAND, FLAG_HAND_OVERSTRETCH, FLAG_HAND_PAD_SLIP};
use genesis_core::proof::ProofChain;
use genesis_core::syntropy::dynamics::{
    GRASP_CLAMP_N, GRASP_DT, GRASP_FORCE, GRASP_MARGIN, GRASP_SLIP_V,
};
use genesis_core::syntropy::stamp::{
    default_soma_path, forge_dark_window, ShearCatchScenario, HAND_SOMA_RESERVED,
    SHEAR_CATCH_HORIZON_MAX_S, SHEAR_CATCH_HORIZON_MIN_S,
};
use genesis_core::syntropy::StrangeAttractor;
use std::env;
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut scenario = ShearCatchScenario::default();
    if let Some(h) = args.get(1) {
        if h == "-h" || h == "--help" {
            eprintln!(
 "dark_window_forge [horizon_s] [out.soma.bin]\n horizon in [{SHEAR_CATCH_HORIZON_MIN_S}, {SHEAR_CATCH_HORIZON_MAX_S}] s (default {})",
 scenario.horizon_s
 );
            return;
        }
        scenario.horizon_s = h.parse().unwrap_or(scenario.horizon_s);
    }
    let out_path = args
        .get(2)
        .map(PathBuf::from)
        .unwrap_or_else(default_soma_path);

    let t0 = Instant::now();
    let (inverted, frames, bin) =
        forge_dark_window(&scenario, &out_path).expect("write hand_dark_window.soma.bin");
    let elapsed = t0.elapsed();
    let out_path = out_path.canonicalize().unwrap_or(out_path);

    let mut proof = ProofChain::new();
    proof.seed(b"DARK_WINDOW_FORGE");
    proof.feed_f64(scenario.clamped_horizon_s());
    proof.feed_f64(scenario.mass_kg);
    proof.feed_f64(scenario.mu);
    proof.feed_f64(scenario.disturbance_n);
    proof.feed_f64(scenario.slip0_m_s);
    for u in &inverted.sheet.samples {
        proof.feed_f64(*u);
    }
    proof.feed(&bin[24..56]);
    let seal = proof.seal();

    let peak: f64 = inverted.sheet.samples.iter().copied().fold(0.0, f64::max);
    let last = frames.last().expect("hollow soma is invalid");
    let over = last.flags & FLAG_HAND_OVERSTRETCH != 0;
    let slip_flag = last.flags & FLAG_HAND_PAD_SLIP != 0;
    let attr = scenario.attractor();

    println!("====================================================================");
    println!(" DARK WINDOW FORGE · SHEAR CATCH · GRASP HOLD ATTRACTOR");
    println!(
        " horizon {:.3} s · {} ticks @ {:.0} Hz · clamp {:.0} N",
        inverted.sheet.n_steps as f64 * inverted.sheet.dt_s,
        inverted.sheet.n_steps,
        1.0 / GRASP_DT,
        GRASP_CLAMP_N
    );
    println!(
        " mass {:.3} kg · mu {:.3} · shear {:.2} N (mg+kinetic) · slip0 {:.3} m/s",
        scenario.mass_kg,
        scenario.mu,
        scenario.total_shear_n(),
        scenario.slip0_m_s
    );
    println!(
        " target margin {:.2} · iters {} · J {:.4e} · terminal J {:.4e}",
        attr.target()[0],
        inverted.iters,
        inverted.cost,
        inverted.terminal_cost
    );
    println!(
        " terminal F={:.2} N margin={:.3} slip={:.4} m/s peak F={:.2} N",
        inverted.final_state[GRASP_FORCE],
        inverted.final_state[GRASP_MARGIN],
        inverted.final_state[GRASP_SLIP_V],
        peak
    );
    println!("====================================================================");
    print!("{}", inverted.sheet.format_score());
    println!(" -----------------------------------------");
    println!(
        " soma {} {} B body {} reserved {}",
        out_path.display(),
        bin.len(),
        BODY_HAND,
        String::from_utf8_lossy(&HAND_SOMA_RESERVED)
    );
    println!(
        " frames {} peek t={:.3} s tension={:.2} N pad={:.2} N stretch={:.5} m",
        frames.len(),
        last.t,
        last.pos[0],
        last.pos[1],
        last.pos[2]
    );
    println!(
        " q_mcp={:.3} rad slip={:.4} m/s margin={:.3} span={:.3} m",
        last.vel[1], last.vel[2], last.force_torque, last.residual
    );
    println!(" flags tendon_overstretch={} pad_slip={}", over, slip_flag);
    println!(" digest {}", hex::encode(&bin[24..56]));
    println!(" seal {seal}");
    println!(" {:?}", elapsed);
}
