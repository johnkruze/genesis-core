//! Kinematic Hand Forge — shoot close + opposition on the 5-finger tendon plant.
//!
//! Same Cultivator as the 1D force invert. Plant is `HandTendonDynamics`
//! (`evaluate_hand_tendon_dynamics`). Attractor is `KinematicGraspAttractor`.
//!
//! ```text
//! cargo run --release --bin kinematic_hand_forge
//! ```

use genesis_core::last_state::{BODY_HAND, FLAG_HAND_OVERSTRETCH, FLAG_HAND_PAD_SLIP};
use genesis_core::physics::dexterous::{N_HAND_FINGERS, TENDON_STRAIN_WARN};
use genesis_core::proof::ProofChain;
use genesis_core::syntropy::dynamics::{
    HAND_BLOCKED_STRAIN, HAND_CLOSE, HAND_MARGIN, HAND_OPPOSITION, HAND_OVERSTRETCH, HAND_PAD_N,
    HAND_Q_DIP, HAND_Q_MCP, HAND_Q_PIP, HAND_SLIP_V, HAND_STRAIN, HAND_TENSION,
};
use genesis_core::syntropy::stamp::{
    pack_hand_from_kinematic, write_hand_soma, HAND_SOMA_RESERVED,
};
use genesis_core::syntropy::{invert_kinematic_grasp, StrangeAttractor};
use std::time::Instant;

fn main() {
    let mass = 0.35;
    let mu = 0.28;
    let span = 0.028;
    let close0 = 0.40;
    let opp0 = 0.80;
    let slip0 = 0.003;
    let horizon = 80;

    let t0 = Instant::now();
    let out = invert_kinematic_grasp(mass, mu, span, close0, opp0, slip0, horizon);
    let elapsed = t0.elapsed();

    let mut proof = ProofChain::new();
    proof.seed(b"KINEMATIC_HAND_FORGE");
    proof.feed_f64(span);
    proof.feed_f64(mass);
    proof.feed_f64(mu);
    for u in &out.sheet.samples {
        proof.feed_f64(*u);
    }
    let seal = proof.seal();

    let attr = genesis_core::syntropy::KinematicGraspAttractor::wrap(span);
    let x = &out.final_state;
    let over = x[HAND_OVERSTRETCH] > 0.5;
    let frame = pack_hand_from_kinematic(horizon as f64 * 0.001, x);
    let bin = write_hand_soma(&[frame]);
    let soma = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/hand_kinematic.soma.bin");
    if let Some(p) = soma.parent() {
        std::fs::create_dir_all(p).ok();
    }
    std::fs::write(&soma, &bin).expect("write hand_kinematic.soma.bin");
    let soma = soma.canonicalize().unwrap_or(soma);

    println!("====================================================================");
    println!(" KINEMATIC HAND FORGE · 5 FINGERS × MCP/PIP/DIP");
    println!(
        " horizon {} ticks @ dt = 0.001 s · u = [close_rad, opposition_rad]",
        out.sheet.n_steps
    );
    println!(
        " mass {:.3} kg · mu {:.3} · span {:.3} m · close0 {:.3} opp0 {:.3}",
        mass, mu, span, close0, opp0
    );
    println!(
        " target span={:.3} blocked_strain={:.3} slip=0 overstretch=0 margin={:.2}",
        attr.target()[0],
        attr.target()[1],
        attr.target()[4]
    );
    println!(
        " iters {} · J {:.4e} · terminal J {:.4e}",
        out.iters, out.cost, out.terminal_cost
    );
    println!(
        " terminal close={:.3} rad opp={:.3} rad pad={:.2} N tension={:.2} N",
        x[HAND_CLOSE], x[HAND_OPPOSITION], x[HAND_PAD_N], x[HAND_TENSION]
    );
    println!(
        " strain={:.4} blocked={:.4} (warn {:.3}) margin={:.3} slip={:.4} m/s",
        x[HAND_STRAIN], x[HAND_BLOCKED_STRAIN], TENDON_STRAIN_WARN, x[HAND_MARGIN], x[HAND_SLIP_V]
    );
    println!(
        " flags tendon_overstretch={} pad_slip={}",
        over,
        x[HAND_SLIP_V].abs() > 0.005
    );
    println!("--------------------------------------------------------------------");
    println!(" finger q_mcp q_pip q_dip chain");
    for f in 0..N_HAND_FINGERS {
        let mcp = x[HAND_Q_MCP + f];
        let pip = x[HAND_Q_PIP + f];
        let dip = x[HAND_Q_DIP + f];
        let name = if f == 0 { "thumb" } else { "finger" };
        println!(
            " {f} {name:<7} {mcp:8.4} {pip:8.4} {dip:8.4} {:>6.3}",
            mcp + pip + dip
        );
    }
    println!("====================================================================");
    print!("{}", out.sheet.format_score());
    println!(" -----------------------------------------");
    println!(
        " soma {} {} B body {} reserved {}",
        soma.display(),
        bin.len(),
        BODY_HAND,
        String::from_utf8_lossy(&HAND_SOMA_RESERVED)
    );
    println!(
        " peek tension={:.2} N pad={:.2} N q_mcp={:.3} margin={:.3} span={:.3} m",
        frame.pos[0], frame.pos[1], frame.vel[1], frame.force_torque, frame.residual
    );
    println!(
        " flags overstretch={} pad_slip={}",
        frame.flags & FLAG_HAND_OVERSTRETCH != 0,
        frame.flags & FLAG_HAND_PAD_SLIP != 0
    );
    println!(" digest {}", hex::encode(&bin[24..56]));
    println!(" seal {seal}");
    println!(" {:?}", elapsed);
}
