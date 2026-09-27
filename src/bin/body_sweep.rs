use std::process::{Command, Stdio};
use std::time::Instant;
use genesis_core::proof;

// ═══════════════════════════════════════════════════════════════
//  FLEET ORCHESTRATOR — THE PRODUCT TIER
// ═══════════════════════════════════════════════════════════════

// Active Fleet Roster
const FLEET: &[(&str, &str, &str)] = &[
    ("titanhauler_monte_carlo", "Titanhauler", "titanhauler"),
    ("humanoid_monte_carlo", "Humanoid", "humanoid"),
    ("autonomous_car_monte_carlo", "Autonomous Car", "autonomous_car"),
    ("satellite_monte_carlo", "Satellite", "satellite"),
    ("maven_monte_carlo", "Maven Interceptor", "maven"),
    ("submarine_monte_carlo", "Submarine", "submarine"),
    ("autonomous_boat_monte_carlo", "Autonomous Boat", "autonomous_boat"),
];

/// Extract SHA-256 Run Proof from binary output.
fn extract_proof(stdout: &str, stderr: &str) -> Option<String> {
    stdout
        .lines()
        .chain(stderr.lines())
        .find(|l| l.contains("SHA-256 Run Proof:") || l.contains("Run proof:"))
        .and_then(|l| {
            l.split("SHA-256 Run Proof:").nth(1)
                .or_else(|| l.split("Run proof:").nth(1))
        })
        .map(|s| s.trim().to_string())
}

/// Run a single Fleet binary, saving to exact Body architecture paths
fn run_fleet_binary(binary: &str, body_name: &str, count: u64, root_dir: &str) -> (bool, f64, Option<String>) {
    let start = Instant::now();
    
    let body_root = format!("{}/{}", root_dir, body_name);
    let _ = std::fs::create_dir_all(&body_root);

    let args = vec![
        "run".to_string(), "--release".to_string(), "--bin".to_string(), binary.to_string(), "--".to_string(),
        count.to_string(), "--out-dir".to_string(), body_root.clone()
    ];

    let output = Command::new("cargo")
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();
        
    let elapsed = start.elapsed().as_secs_f64();

    match output {
        Ok(out) => {
            let _stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            (out.status.success(), elapsed, extract_proof(&_stdout, &stderr))
        }
        Err(_) => (false, elapsed, None),
    }
}

fn push_master_proof_to_icp(master_proof: &str, total_generated: u64) {
    println!("  [ICP BRIDGE] Sealing Master Fleet Proof to ICP for SOMA Generation...");
    
    // Default system principal
    let principal_cmd = Command::new("dfx").args(&["identity", "get-principal"]).output();
    let target_principal = match principal_cmd {
        Ok(out) => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        Err(_) => { println!("  [ICP BRIDGE] FAILED: dfx identity not found."); return; }
    };

    let current_dir = std::env::current_dir().unwrap();
    let spectra_gen_dir = current_dir.join("../spectra_genesis");
    
    let ledger_id_cmd = Command::new("dfx").args(&["canister", "id", "soma_ledger"]).current_dir(&spectra_gen_dir).output();
    let ledger_principal = match ledger_id_cmd {
        Ok(out) => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        Err(_) => { println!("  [ICP BRIDGE] FAILED: Could not locate soma_ledger canister ID."); return; }
    };

    let candid_arg = format!(
        "(\"SOLIS_PRIME\", \"{}\", {}, principal \"{}\", principal \"{}\")",
        master_proof, total_generated, target_principal, ledger_principal
    );

    let output = Command::new("dfx")
        .args(&["canister", "call", "spectra_genesis_backend", "mint_somatic_proof", &candid_arg, "--network", "local"])
        .current_dir(&spectra_gen_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let _stdout = String::from_utf8_lossy(&out.stdout);
            println!("  [ICP BRIDGE] SUCCESS: Minted SOMA native tokens to wallet ({})", target_principal);
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            println!("  [ICP BRIDGE] FAILED to mint SOMA: {}", stderr);
        }
        Err(e) => {
            println!("  [ICP BRIDGE] DFX execute failed: {}", e);
        }
    }
}

fn orchestrate_fleet(count_per_body: u64, root_dir: &str) {
    println!("═══════════════════════════════════════════════════════════");
    println!("  G^G FLEET ORCHESTRATOR — PRODUCT TIER GENERATION");
    println!("  Target:  {} trajectories per body", count_per_body);
    println!("  Mode:    SEQUENTIAL (Data layer population)");
    println!("═══════════════════════════════════════════════════════════\n");

    let sweep_start = Instant::now();
    let mut sub_proofs: Vec<String> = Vec::new();
    let mut total_generated = 0;

    for (binary, display_name, folder) in FLEET {
        print!("  Deploying {}... ", display_name);
        use std::io::Write;
        std::io::stdout().flush().unwrap();

        let (success, elapsed, proof_hash) = run_fleet_binary(binary, folder, count_per_body, root_dir);

        if success {
            println!("OK ({:.2}s)", elapsed);
            if let Some(hash) = proof_hash {
                sub_proofs.push(hash);
            }
            total_generated += count_per_body;
        } else {
            println!("FAILED ({:.2}s)", elapsed);
        }
    }

    let sweep_elapsed = sweep_start.elapsed();

    println!("\n═══════════════════════════════════════════════════════════");
    let master_proof = if sub_proofs.is_empty() { "N/A (JSON Output Mode)".to_string() } else { proof::seal_run(&sub_proofs) };
    println!("  FLEET ORCHESTRATOR COMPLETE");
    println!("  {} somatic trajectories pushed to `data/corpus/`", total_generated);
    println!("  Master Fleet Proof: {}", master_proof);
    
    if !sub_proofs.is_empty() {
        push_master_proof_to_icp(&master_proof, total_generated);
    }
    
    println!("  Total Time: {:.1}s", sweep_elapsed.as_secs_f64());
    println!("═══════════════════════════════════════════════════════════");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let target: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(10_000);
    
    // Default to the central `data/corpus` path
    let default_root = "../../data/corpus".to_string();
    let root_dir = args.iter().position(|a| a == "--root").and_then(|i| args.get(i + 1)).cloned().unwrap_or(default_root);
    
    orchestrate_fleet(target, &root_dir);
}
