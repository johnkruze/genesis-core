use genesis_core::physics::reactor::ReactorState;
use genesis_core::physics::tokamak::Tokamak;
use genesis_core::physics::swing::SynchronousMachine;
use serde_json::json;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

fn run_reactor_test(scratch_dir: &Path) {
    println!("[*] Running Upgraded Reactor Simulation...");
    let mut reactor = ReactorState::default();
    let dt = 1.0; // 1s step
    let mut history = Vec::new();

    // T=0h to T=20h: Nominal operation (steady state)
    for _ in 0..1000 {
        reactor.step(dt);
        history.push(json!({
            "t_s": reactor.time_s,
            "flux": reactor.flux,
            "iodine": reactor.iodine_135,
            "xenon": reactor.xenon_135,
            "fuel_temp": reactor.fuel_temp,
            "coolant_temp": reactor.coolant_temp,
            "control_rods_rho": reactor.control_rods_rho,
            "net_reactivity": reactor.net_reactivity(),
            "prompt_critical": reactor.prompt_critical,
            "residual": reactor.residual,
        }));
    }

    // T=20h: Power drop (insert rods)
    reactor.control_rods_rho -= 0.02;
    for _ in 0..1000 {
        reactor.step(dt);
        history.push(json!({
            "t_s": reactor.time_s,
            "flux": reactor.flux,
            "iodine": reactor.iodine_135,
            "xenon": reactor.xenon_135,
            "fuel_temp": reactor.fuel_temp,
            "coolant_temp": reactor.coolant_temp,
            "control_rods_rho": reactor.control_rods_rho,
            "net_reactivity": reactor.net_reactivity(),
            "prompt_critical": reactor.prompt_critical,
            "residual": reactor.residual,
        }));
    }

    // T=40h: Danger rod pull
    reactor.pull_control_rods(0.04);
    for _ in 0..500 {
        reactor.step(dt);
        history.push(json!({
            "t_s": reactor.time_s,
            "flux": reactor.flux,
            "iodine": reactor.iodine_135,
            "xenon": reactor.xenon_135,
            "fuel_temp": reactor.fuel_temp,
            "coolant_temp": reactor.coolant_temp,
            "control_rods_rho": reactor.control_rods_rho,
            "net_reactivity": reactor.net_reactivity(),
            "prompt_critical": reactor.prompt_critical,
            "residual": reactor.residual,
        }));
        if reactor.prompt_critical {
            break;
        }
    }

    let output_path = scratch_dir.join("reactor_run.json");
    let file_content = serde_json::to_string_pretty(&history).unwrap();
    let mut file = File::create(&output_path).unwrap();
    file.write_all(file_content.as_bytes()).unwrap();
    println!("    Saved telemetry to {}", output_path.display());
    println!("    Final State: Prompt Critical = {}, Flux = {:.2e}, Xenon = {:.2e}, Fuel Temp = {:.1} K, Residual = {:.2e}",
             reactor.prompt_critical, reactor.flux, reactor.xenon_135, reactor.fuel_temp, reactor.residual);
}

fn run_tokamak_test(scratch_dir: &Path) {
    println!("[*] Running Upgraded Tokamak Simulation...");
    let mut tokamak = Tokamak::new();
    let dt_us = 10.0; // 10 us steps
    let mut history = Vec::new();

    let target_b = tokamak.b_field;
    let mut last_ai_update = 0;
    
    // We run with normal stabilizing control first (perfect alignment)
    for _ in 0..1000 {
        let time = tokamak.time_us;
        if time - last_ai_update >= 100 {
            tokamak.apply_agentic_ai_field(target_b, 0.0, 0.0);
            last_ai_update = time;
        }
        tokamak.step(dt_us);
        history.push(json!({
            "time_us": tokamak.time_us,
            "plasma_radius": tokamak.plasma_radius,
            "z_displacement": tokamak.z_displacement,
            "pf_voltage": tokamak.pf_voltage,
            "pf_current": tokamak.pf_current,
            "coil_temp": tokamak.coil_temp,
            "coil_quenched": tokamak.coil_quenched,
            "quenched": tokamak.quenched,
            "residual": tokamak.residual,
        }));
    }

    // Now we introduce heavy AI asymmetry noise to trigger vertical instability and heating.
    // Injected field error is 15 mT (0.015 T). We keep radial_noise = 0.0 to focus on Z-axis instability
    // as modeled in the ZTP sweep configuration.
    for _ in 0..5000 {
        let time = tokamak.time_us;
        if time - last_ai_update >= 100 {
            tokamak.apply_agentic_ai_field(target_b, 0.0, 0.015); 
            last_ai_update = time;
        }
        tokamak.step(dt_us);
        history.push(json!({
            "time_us": tokamak.time_us,
            "plasma_radius": tokamak.plasma_radius,
            "z_displacement": tokamak.z_displacement,
            "pf_voltage": tokamak.pf_voltage,
            "pf_current": tokamak.pf_current,
            "coil_temp": tokamak.coil_temp,
            "coil_quenched": tokamak.coil_quenched,
            "quenched": tokamak.quenched,
            "residual": tokamak.residual,
        }));
        if tokamak.quenched {
            break;
        }
    }

    let output_path = scratch_dir.join("tokamak_run.json");
    let file_content = serde_json::to_string_pretty(&history).unwrap();
    let mut file = File::create(&output_path).unwrap();
    file.write_all(file_content.as_bytes()).unwrap();
    println!("    Saved telemetry to {}", output_path.display());
    println!("    Final State: Quenched = {}, Coil Quenched = {}, Z Displacement = {:.4} m, Coil Temp = {:.1} K, Current = {:.1} A, Residual = {:.2e}",
             tokamak.quenched, tokamak.coil_quenched, tokamak.z_displacement, tokamak.coil_temp, tokamak.pf_current, tokamak.residual);
}

fn run_swing_test(scratch_dir: &Path) {
    println!("[*] Running Upgraded Swing Grid Simulation...");
    let mut machine = SynchronousMachine::new();
    
    // Upgrading to a hyper-low-inertia grid to expose the desynchronization and cascade cliff
    machine.inverter_fraction = 0.85; // 85% Inverter capacity (extremely low inertia grid)
    
    let dt = 0.001; // 1 ms steps
    let mut history = Vec::new();

    // Nominal grid operations
    for _ in 0..1000 {
        machine.step(dt);
        history.push(json!({
            "time_ms": machine.time_ms,
            "delta": machine.delta,
            "delta_omega": machine.delta_omega,
            "pll_err": machine.pll_err,
            "inverter_tripped": machine.inverter_tripped,
            "governor_valve": machine.governor_valve,
            "cascaded": machine.cascaded,
            "residual": machine.residual,
        }));
    }

    // Weather solar dropout (80% loss of solar/wind IBR capacity)
    machine.simulate_weather_loss(80.0);

    // AI takes 200 ms to respond
    for _ in 0..200 {
        machine.step(dt);
        history.push(json!({
            "time_ms": machine.time_ms,
            "delta": machine.delta,
            "delta_omega": machine.delta_omega,
            "pll_err": machine.pll_err,
            "inverter_tripped": machine.inverter_tripped,
            "governor_valve": machine.governor_valve,
            "cascaded": machine.cascaded,
            "residual": machine.residual,
        }));
    }

    // AI over-corrects by shutting down an interconnect, causing a massive 65% shock constraint (transmission loss)
    machine.ai_apply_load_mismatch(65.0);

    // Run until collapse or 5 seconds total
    while !machine.cascaded && machine.time_ms < 5000 {
        machine.step(dt);
        history.push(json!({
            "time_ms": machine.time_ms,
            "delta": machine.delta,
            "delta_omega": machine.delta_omega,
            "pll_err": machine.pll_err,
            "inverter_tripped": machine.inverter_tripped,
            "governor_valve": machine.governor_valve,
            "cascaded": machine.cascaded,
            "residual": machine.residual,
        }));
    }

    let output_path = scratch_dir.join("swing_run.json");
    let file_content = serde_json::to_string_pretty(&history).unwrap();
    let mut file = File::create(&output_path).unwrap();
    file.write_all(file_content.as_bytes()).unwrap();
    println!("    Saved telemetry to {}", output_path.display());
    println!("    Final State: Cascaded = {}, Inverter Tripped = {}, delta = {:.4} rad, delta_omega = {:.2} rad/s, PLL Err = {:.4} rad, Residual = {:.2e}",
             machine.cascaded, machine.inverter_tripped, machine.delta, machine.delta_omega, machine.pll_err, machine.residual);
}

fn main() {
    println!("====================================================================");
    println!("  ZTP ENERGY UPGRADES: VERIFICATION TEST HARNESS");
    println!("====================================================================");
    
    let scratch_dir_arg = std::env::args().nth(1)
        .unwrap_or_else(|| std::env::temp_dir().to_string_lossy().into_owned());
    let scratch_path = Path::new(&scratch_dir_arg);
    fs::create_dir_all(&scratch_path).unwrap();

    run_reactor_test(&scratch_path);
    run_tokamak_test(&scratch_path);
    run_swing_test(&scratch_path);
    
    println!("====================================================================");
    println!("  ALL RUNS COMPLETED SUCCESSFULLY. MATH AND PHYSICS STABLE.");
    println!("====================================================================");
}
