use genesis_core::last_state::{LastStateFrame64, BODY_HAND, FLAG_HAND_PAD_SLIP};
use genesis_core::physics::dexterous::{
    evaluate_hand_tendon_dynamics, C_HandTendonState, N_HAND_FINGERS, THUMB_OPPOSITION_RAD,
};
use genesis_core::sweeper::{self, OracleVerdict};
use std::path::Path;

fn holding_hand() -> C_HandTendonState {
    C_HandTendonState {
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
    }
}

fn main() {
    println!("═══════════════════════════════════════════════════════════");
    println!("  REFLEX: tendon step, lock-free enqueue, sweeper hash");
    println!("  Domain: {}", sweeper::soma_frame_domain(BODY_HAND));
    println!("═══════════════════════════════════════════════════════════\n");

    let (tx, rx) = sweeper::frame_channel(64);
    let failed = Path::new(sweeper::FAILED_ANCHORS_DIR);
    let sweeper = std::thread::spawn(move || {
        let raw = rx.recv().expect("frame");
        sweeper::anchor_frame(BODY_HAND, raw, failed)
    });

    let mut state = holding_hand();
    let dt = 0.001f32;
    for _ in 0..2_000 {
        let step = evaluate_hand_tendon_dynamics(&mut state, dt);
        if step.pad_slip {
            eprintln!("pad_slip before the fault");
            std::process::exit(1);
        }
    }

    state.object_span_m = 0.022;
    state.commanded_close_rad = 0.42;
    state.object_mass = 2.4;
    state.static_friction_coeff = 0.11;
    state.dynamic_friction_coeff = 0.09;
    state.slip_velocity = 0.0;

    let mut interrupt = None;
    for step in 0..500 {
        let result = evaluate_hand_tendon_dynamics(&mut state, dt);
        if result.pad_slip {
            let frame = LastStateFrame64 {
                t: step as f64 * f64::from(dt),
                pos: [result.tendon_tension_n, result.pad_normal_n, result.stretch_m],
                vel: [state.opposition_rad, state.q_mcp[1], state.slip_velocity],
                force_torque: result.margin,
                residual: state.object_span_m,
                flags: FLAG_HAND_PAD_SLIP,
                proof: [0; 16],
            };
            let elapsed = sweeper::enqueue_frame(&tx, frame.to_bytes()).expect("enqueue");
            interrupt = Some(elapsed);
            break;
        }
    }
    let interrupt = interrupt.expect("tendon step did not return pad_slip");
    println!(
        "  Physics interruption {:.3} µs (enqueue only, no hash).",
        interrupt.as_secs_f64() * 1.0e6
    );
    if interrupt.as_micros() >= 10 {
        eprintln!("  interruption exceeded 10 µs");
    }

    match sweeper.join().expect("sweeper") {
        OracleVerdict::Recorded { raw, .. } => println!("  recorded: {raw}"),
        OracleVerdict::Rejected { reason, .. } => {
            println!("  canister rejected: {reason}");
            println!("  frame queued under {}", sweeper::FAILED_ANCHORS_DIR);
        }
        OracleVerdict::Transport { reason } => {
            println!("  transport: {reason}");
            println!("  frame queued under {}", sweeper::FAILED_ANCHORS_DIR);
        }
    }
}
