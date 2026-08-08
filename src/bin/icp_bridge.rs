use std::process::Command;
use std::time::{Duration, Instant};
use std::fs;
use serde_json::Value;

const CANISTER_ID: &str = "ad7wi-4aaaa-aaaad-aeijq-cai";
const CALL_DELAY_MS: u64 = 200;

#[allow(dead_code)]
struct IcpCall {
    domain: String,
    file_name: String,
    proof_hash: String,
    latency_ms: u64,
    success: bool,
}

fn call_canister(domain: &str, proof_hash: &str) -> Result<(String, u64), String> {
    let candid_arg = format!("(\"SOLIS_PRIME\", \"{}\", \"{}\")", domain, proof_hash);

    let start = Instant::now();

    let output = Command::new("dfx")
        .args([
            "canister", "call",
            CANISTER_ID,
            "record_trajectory_proof",
            &candid_arg,
            "--network", "ic",
        ])
        .env("DFX_WARNING", "-mainnet_plaintext_identity")
        .output()
        .map_err(|e| format!("dfx exec failed: {}", e))?;

    let latency = start.elapsed().as_millis() as u64;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("dfx error: {}", stderr.trim()));
    }

    let raw = String::from_utf8_lossy(&output.stdout).to_string();
    Ok((raw, latency))
}

fn main() {
    println!("═══════════════════════════════════════════════════════════");
    println!("  G^G ICP BRIDGE — THE SOVEREIGN LEDGER (WRITE MODE)");
    println!("  Canister: {}", CANISTER_ID);
    println!("  Network:  ic (mainnet)");
    println!("  Action:   Scanning /corpus/ for generated proofs...");
    println!("═══════════════════════════════════════════════════════════");
    println!();

    let corpus_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/corpus");
    let mut all_calls: Vec<IcpCall> = Vec::new();
    let start = Instant::now();

    let domains = match fs::read_dir(corpus_dir) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Failed to read corpus directory: {}", e);
            return;
        }
    };

    for entry in domains.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            let domain_name = path.file_name().unwrap().to_string_lossy().to_uppercase();
            if domain_name == "PRODUCTS" { continue; }

            if let Ok(files) = fs::read_dir(&path) {
                for file_entry in files.filter_map(Result::ok) {
                    let file_path = file_entry.path();
                    if file_path.extension().and_then(|e| e.to_str()) == Some("json") {
                        // Read JSON to get the first trajectory's proof hash as a representative sample
                        let content = match fs::read_to_string(&file_path) {
                            Ok(c) => c,
                            Err(_) => continue,
                        };

                        let parsed: Value = match serde_json::from_str(&content) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };

                        let proof_hash = parsed.get("trajectories")
                            .and_then(|t| t.as_array())
                            .and_then(|arr| arr.first())
                            .and_then(|first| first.get("proof_hash"))
                            .and_then(|hash| hash.as_str())
                            .unwrap_or("UNKNOWN_HASH")
                            .to_string();

                        if proof_hash == "UNKNOWN_HASH" { continue; }

                        let file_name = file_path.file_name().unwrap().to_string_lossy().to_string();
                        println!("  ┌─ Committing {} ({})", domain_name, file_name);

                        std::thread::sleep(Duration::from_millis(CALL_DELAY_MS));

                        let (_response, latency, success) = match call_canister(&domain_name, &proof_hash) {
                            Ok((resp, lat)) => {
                                println!("  │  {}ms — {}", lat, resp.replace('\n', ""));
                                (resp, lat, true)
                            }
                            Err(e) => {
                                eprintln!("  │  ERROR: {}", e);
                                (e, 0, false)
                            }
                        };

                        all_calls.push(IcpCall {
                            domain: domain_name.clone(),
                            file_name,
                            proof_hash,
                            latency_ms: latency,
                            success,
                        });
                    }
                }
            }
        }
    }

    let total_success = all_calls.iter().filter(|c| c.success).count();
    
    println!("═══════════════════════════════════════════════════════════");
    println!("  ICP BRIDGE COMPLETE");
    println!("  Total Proofs Secured: {}/{}", total_success, all_calls.len());
    println!("  Elapsed Time: {:.1}s", start.elapsed().as_secs_f64());
    println!("═══════════════════════════════════════════════════════════");
}
