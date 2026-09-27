use genesis_core::physics::reactor::ReactorState;

fn main() {
    println!("=== G^G KERNEL: XENON-135 PROMPT CRITICALITY SIMULATOR ===");
    println!("Operating mode: 11/10 BONKERS THERMODYNAMICS.\n");

    // Initialize the reactor physics state
    let mut reactor = ReactorState::new();
    
    // Total simulation time (seconds)
    let total_time = 48.0 * 3600.0; // 48 hours
    let dt = 1.0; // 1 second timestep
    let mut current_time = 0.0;

    println!("T=0h: Reactor operating at nominal power (flux: {:.1e}). Base Rho: {:.2}, Control Rho: {:.2}", reactor.flux, reactor.base_rho, reactor.control_rods_rho);

    let mut printed_12h = false;
    let mut printed_24h = false;
    let mut printed_power_drop = false;
    let mut printed_rod_pull = false;

    while current_time < total_time {
        // Event Schedule
        if current_time >= 12.0 * 3600.0 && !printed_12h {
            println!("T=12h: Steady state reached. Iodine and Xenon levels stable.");
            println!("      Iodine-135: {:.2e} atoms/cm^3", reactor.iodine_135);
            println!("      Xenon-135: {:.2e} atoms/cm^3", reactor.xenon_135);
            println!("      Reactivity penalty: {:.4}", reactor.xenon_reactivity_worth());
            printed_12h = true;
        }

        if current_time >= 24.0 * 3600.0 && !printed_power_drop {
            println!("T=24h: GRID DEMAND DROPS. Operators drop power by inserting control rods.");
            reactor.control_rods_rho -= 0.02; // Insert rods
            println!("      Flux dropping... The Xenon Pit begins.");
            printed_power_drop = true;
        }

        if current_time >= 32.0 * 3600.0 && !printed_rod_pull {
            println!("T=32h: XENON PIT DEEPENS. Operators fight the poison.");
            println!("      Xenon Reactivity: {:.4}", reactor.xenon_reactivity_worth());
            println!("      Operators pull rods beyond safe limits to maintain power...");
            reactor.pull_control_rods(0.04); // Pull rods dangerously far out
            printed_rod_pull = true;
        }

        reactor.step(dt);
        
        if reactor.prompt_critical {
            println!("\n!!! FATAL ALARM: PROMPT CRITICALITY DETECTED !!!");
            println!("T={:.2}h: Reactor net reactivity ({:.4}) exceeded delayed neutron fraction (BETA={:.4})", reactor.time_s / 3600.0, reactor.net_reactivity(), genesis_core::physics::reactor::BETA);
            println!("      Xenon burned out faster than expected while rods were fully extracted.");
            println!("      The thermal beast has awakened. Core destruction imminent.");
            break;
        }

        current_time += dt;
    }

    println!("\n=== SIMULATION TERMINATED ===");
    println!("Sealing timeline configuration and state hashes.");
    let proof_hash = reactor.get_sealed_hash();
    println!("SHA-256 PROOF SEAL: {}", proof_hash);
    println!("The math is locked. The physics are sovereign.");
}
