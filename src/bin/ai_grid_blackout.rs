use genesis_core::physics::swing::SynchronousMachine;

fn main() {
    println!("=== G^G KERNEL: AC POWER GRID CASCADE SIMULATOR ===");
    println!("Target: Smart Grid Load Balancing AI.");
    println!("Physics: The Swing Equation.\n");

    let mut grid = SynchronousMachine::new();
    let dt = 0.001; // 1 millisecond timestep

    println!("T=0ms: Grid synchronized at nominal frequency 60Hz.");
    println!("      Mechanical power P_m = {:.2} pu", grid.p_mech);
    println!("      Electrical power capacity P_max = {:.2} pu", grid.p_max);
    println!("      Rotor angle delta = {:.2} degrees", grid.delta.to_degrees());

    // Advance safely for a bit
    for _ in 0..100 {
        grid.step(dt);
    }
    
    println!("\nT={}ms: AGENTIC AI INGESTS SMART GRID STATE.", grid.time_ms);
    println!("      AI hallucinates an optimization target. 5% load mismatch injected.");
    grid.ai_apply_load_mismatch(5.0); // 5% mismatch
    println!("      New Electrical power capacity P_max = {:.3} pu", grid.p_max);
    println!("      WARNING: The mechanical power exceeds electrical grip. Rotor is accelerating.\n");

    // Advance until cascade
    while !grid.cascaded && grid.time_ms < 10000 { // fallback timeout 10 seconds
        grid.step(dt);
        if grid.cascaded {
            println!("!!! BREAKER TRIP !!! LOSS OF SYNCHRONISM DETECTED !!!");
            println!("T={}ms: Rotor angle breached 90 degrees ({:.2} deg).", grid.time_ms, grid.delta.to_degrees());
            println!("      Generators desynchronized. Regional grid cascade initiated.");
            break;
        }
    }

    if !grid.cascaded {
        println!("Grid stabilized. AI hallucination survived (unexpected physics result).");
    }

    println!("\n=== SIMULATION TERMINATED ===");
    println!("Sealing timeline configuration and exactly timed grid death.");
    let proof_hash = grid.get_sealed_hash();
    println!("SHA-256 PROOF SEAL: {}", proof_hash);
    println!("The math is locked. The physics are sovereign.");
}
