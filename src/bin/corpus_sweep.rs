use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Instant, SystemTime};
use genesis_core::proof;

// Measured rates from 100x sweep (traj/sec per core)
const SUBSTRATES: &[(&str, &str, f64)] = &[
    ("atheric_monte_carlo", "Atheric (Signal)", 21_476.0),
    ("terran_monte_carlo", "Terran (Mycelium)", 14_039.0),
    ("mycelial_monte_carlo", "Mycelial (Network)", 747.0),
    ("mars_monte_carlo", "Mars (CO₂)", 146.0),
    ("orbital_monte_carlo", "Orbital (Vacuum)", 116.0),
    ("marine_monte_carlo", "Marine (Seawater)", 11.0),
    // Newly integrated domains
    ("plutonian_monte_carlo", "Plutonian (Core)", 105.0),
    ("asteroid_monte_carlo", "Asteroid (NEO)", 135.0),
    ("celestial_monte_carlo", "Celestial (Astro)", 150.0),
    ("energy_monte_carlo", "Energy (Grid)", 200.0),
];

struct SubstrateResult {
    binary: String,
    domain: String,
    count: u64,
    elapsed_secs: f64,
    rate: f64,
    proof_hash: Option<String>,
    success: bool,
}

/// Extract SHA-256 Run Proof from binary output.
/// Handles both diagnostic mode ("SHA-256 Run Proof:") and export mode ("Run proof:").
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

/// Run a single Monte Carlo binary, blocking until complete.
fn run_binary(binary: &str, count: u64, export_dir: Option<&str>) -> (bool, f64, Option<String>, String, String) {
    let start = Instant::now();
    let mut args = vec![
        "run".to_string(), "--release".to_string(), "--bin".to_string(), binary.to_string(), "--".to_string(),
        count.to_string(),
    ];
    if let Some(dir) = export_dir {
        if binary != "icp_bridge" {
            let ts = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
            let sub_dir = binary.split('_').next().unwrap_or(binary);
            let outfile = format!("{}/{}/{}_{}_{}.json", dir, sub_dir, binary, ts, count);
            args.push("--out".to_string());
            args.push(outfile);
        }
    }

    let output = Command::new("cargo")
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();
    let elapsed = start.elapsed().as_secs_f64();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            (out.status.success(), elapsed, extract_proof(&stdout, &stderr), stdout, stderr)
        }
        Err(e) => (false, elapsed, None, String::new(), e.to_string()),
    }
}

// ═══════════════════════════════════════════════════════════════
//  BURST MODE — All cores fire, speed-proportional distribution
// ═══════════════════════════════════════════════════════════════

fn burst_mode(target: u64, export_dir: Option<&str>) {
    let total_rate: f64 = SUBSTRATES.iter().map(|(_, _, r)| r).sum();

    // Speed-proportional distribution — each substrate gets trajectories
    // proportional to its speed so they all finish at the same time
    let assignments: Vec<(&str, &str, u64)> = SUBSTRATES
        .iter()
        .map(|(bin, domain, rate)| {
            let count = ((rate / total_rate) * target as f64).round() as u64;
            (*bin, *domain, count)
        })
        .collect();

    let actual_total: u64 = assignments.iter().map(|(_, _, c)| c).sum();
    let est_seconds = target as f64 / total_rate;

    println!("═══════════════════════════════════════════════════════════");
    println!("  G^G CORPUS BURST — ALL CORES FIRE");
    println!("  Target:  {} trajectories", target);
    println!("  Mode:    PARALLEL (6 cores, speed-proportional)");
    println!("  Est:     {:.1} minutes", est_seconds / 60.0);
    println!("═══════════════════════════════════════════════════════════");
    println!();

    println!("  ┌───────────────────┬────────────┬──────────┬──────────────┐");
    println!("  │ SUBSTRATE         │ COUNT      │ RATE     │ EST TIME     │");
    println!("  ├───────────────────┼────────────┼──────────┼──────────────┤");
    for (_, domain, count) in &assignments {
        let rate = SUBSTRATES.iter().find(|(_, d, _)| *d == *domain).unwrap().2;
        let est = *count as f64 / rate;
        println!(
            "  │ {:>17} │ {:>10} │ {:>7.0}/s│ {:>8.1} min │",
            domain, count, rate, est / 60.0
        );
    }
    println!("  ├───────────────────┼────────────┼──────────┼──────────────┤");
    println!(
        "  │ TOTAL             │ {:>10} │          │ {:>8.1} min │",
        actual_total, est_seconds / 60.0
    );
    println!("  └───────────────────┴────────────┴──────────┴──────────────┘");
    println!();

    // Channel for receiving results as substrates complete
    let (tx, rx) = mpsc::channel::<SubstrateResult>();

    let sweep_start = Instant::now();

    // Spawn all 6 substrates in parallel — one thread per binary
    println!("  Launching 6 substrates on 6 cores...");
    for (binary, domain, count) in &assignments {
        let tx = tx.clone();
        let binary = binary.to_string();
        let domain = domain.to_string();
        let count = *count;

        let exp_dir = export_dir.map(|s| s.to_string());

        thread::spawn(move || {
            let (success, elapsed, proof_hash, _, _) = run_binary(&binary, count, exp_dir.as_deref());
            let rate = if elapsed > 0.0 { count as f64 / elapsed } else { 0.0 };
            tx.send(SubstrateResult {
                binary,
                domain,
                count,
                elapsed_secs: elapsed,
                rate,
                proof_hash,
                success,
            })
            .ok();
        });
    }
    drop(tx); // drop sender so rx iterator ends when all threads finish

    println!();

    // Collect results in completion order (fastest finish first)
    let mut results: Vec<SubstrateResult> = Vec::new();
    let mut completed = 0;
    for result in rx {
        completed += 1;
        let wall = sweep_start.elapsed().as_secs_f64();
        let status = if result.success { "SEALED" } else { "FAILED" };
        println!(
            "  [{}/{}] {:>17} — {} | {:>10} traj | {:.1}s | {:.0}/sec | wall {:.1}s",
            completed, assignments.len(), result.domain, status, result.count, result.elapsed_secs, result.rate, wall,
        );
        results.push(result);
    }

    let sweep_elapsed = sweep_start.elapsed();

    // ═══════════════════════════════════════════════════════════
    //  BURST RESULTS
    // ═══════════════════════════════════════════════════════════

    println!();
    println!("═══════════════════════════════════════════════════════════");
    println!("  CORPUS BURST RESULTS");
    println!("═══════════════════════════════════════════════════════════");
    println!();

    let total_generated: u64 = results.iter().filter(|r| r.success).map(|r| r.count).sum();
    let total_failed: usize = results.iter().filter(|r| !r.success).count();

    // Sort by rate (fastest first) for display
    results.sort_by(|a, b| b.rate.partial_cmp(&a.rate).unwrap_or(std::cmp::Ordering::Equal));

    println!("  ┌───────────────────┬────────────┬──────────┬──────────┬────────────────────────────────────┐");
    println!("  │ SUBSTRATE         │ COUNT      │ TIME     │ RATE     │ PROOF (first 32)                   │");
    println!("  ├───────────────────┼────────────┼──────────┼──────────┼────────────────────────────────────┤");

    for r in &results {
        let proof_str = match &r.proof_hash {
            Some(h) if h.len() >= 32 => format!("{}…", &h[..32]),
            Some(h) => h.clone(),
            None => "—".to_string(),
        };
        let fail = if r.success { "" } else { " FAIL" };
        println!(
            "  │ {:>17} │ {:>10} │ {:>7.1}s │ {:>7.0}/s│ {:>34} │{}",
            r.domain, r.count, r.elapsed_secs, r.rate, proof_str, fail,
        );
    }

    println!("  ├───────────────────┼────────────┼──────────┼──────────┼────────────────────────────────────┤");
    println!(
        "  │ TOTAL             │ {:>10} │ {:>7.1}s │          │                                    │",
        total_generated,
        sweep_elapsed.as_secs_f64(),
    );
    println!("  └───────────────────┴────────────┴──────────┴──────────┴────────────────────────────────────┘");
    println!();

    if total_failed > 0 {
        println!("  WARNING: {} substrate(s) failed", total_failed);
        println!();
    }

    // Master proof: seal all sub-proofs
    let sub_proofs: Vec<String> = results
        .iter()
        .filter_map(|r| r.proof_hash.clone())
        .collect();
    let master_proof = proof::seal_run(&sub_proofs);

    println!("  MASTER SHA-256 CORPUS PROOF: {}", master_proof);
    println!("  ({} sub-proofs sealed)", sub_proofs.len());
    println!();

    // Combined total with previous sweep
    println!("  ┌─────────────────────────────────────────────┐");
    println!("  │ CORPUS STATUS                               │");
    println!("  ├─────────────────────────────────────────────┤");
    println!("  │ This burst:     {:>12} trajectories  │", total_generated);
    println!("  │ Previous sweep: {:>12} trajectories  │", 5_509_600u64);
    println!(
        "  │ GRAND TOTAL:    {:>12} trajectories  │",
        total_generated + 5_509_600
    );
    println!("  │ Wall time:      {:>12.1} minutes      │", sweep_elapsed.as_secs_f64() / 60.0);
    println!("  └─────────────────────────────────────────────┘");
    println!();

    println!("═══════════════════════════════════════════════════════════");
    println!("  G^G CORPUS BURST — SEALED");
    println!("  {} trajectories across {} substrates in {:.1} minutes",
        total_generated, assignments.len(), sweep_elapsed.as_secs_f64() / 60.0);
    println!("  {} cores, speed-proportional, parallel", assignments.len());
    println!("  Master proof: {}…", &master_proof[..32]);
    println!("  The corpus breathes.");
    println!("═══════════════════════════════════════════════════════════");
}

// ═══════════════════════════════════════════════════════════════
//  SEQUENTIAL MODE — Original sweep behavior
// ═══════════════════════════════════════════════════════════════

fn sequential_mode(args: &[String], export_dir: Option<&str>) {
    let multiplier: f32 = args
        .get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0);

    let base_counts: &[(&str, &str, u32)] = &[
        ("mars_monte_carlo", "Mars (CO₂)", 10_000),
        ("marine_monte_carlo", "Marine (Seawater)", 10_000),
        ("orbital_monte_carlo", "Orbital (Vacuum)", 10_000),
        ("terran_monte_carlo", "Terran (Mycelium)", 10_000),
        ("mycelial_monte_carlo", "Mycelial (Network)", 5_000),
        ("atheric_monte_carlo", "Atheric (Signal)", 10_000),
        ("plutonian_monte_carlo", "Plutonian (Core)", 10_000),
        ("asteroid_monte_carlo", "Asteroid (NEO)", 10_000),
        ("celestial_monte_carlo", "Celestial (Astro)", 10_000),
        ("energy_monte_carlo", "Energy (Grid)", 10_000),
    ];

    let icp_per = (12.0 * multiplier).ceil() as u32;
    let icp_total = icp_per * 8;

    let counts: Vec<(&str, &str, u32)> = base_counts
        .iter()
        .map(|(b, d, c)| (*b, *d, (*c as f32 * multiplier).ceil() as u32))
        .collect();
    let total_local: u32 = counts.iter().map(|(_, _, c)| c).sum();
    let total_trajectories = total_local + icp_total;

    println!("═══════════════════════════════════════════════════════════");
    println!("  G^G CORPUS SWEEP — THE CORPUS BREATHES");
    println!("  Multiplier: {}x", multiplier);
    println!("  Total Target: {} trajectories", total_trajectories);
    println!("═══════════════════════════════════════════════════════════");
    println!();
    println!("  ┌─────────────────┬───────────┐");
    println!("  │ SUBSTRATE       │ COUNT     │");
    println!("  ├─────────────────┼───────────┤");
    for (_, domain, count) in &counts {
        println!("  │ {:>15} │ {:>9} │", domain, count);
    }
    println!("  │ ICP Bridge      │ {:>9} │", icp_total);
    println!("  ├─────────────────┼───────────┤");
    println!("  │ TOTAL           │ {:>9} │", total_trajectories);
    println!("  └─────────────────┴───────────┘");
    println!();

    let sweep_start = Instant::now();
    let mut results: Vec<SubstrateResult> = Vec::new();

    for (i, (binary, domain, count)) in counts.iter().enumerate() {
        println!("  [{}/{}] {}...", i + 1, counts.len() + 1, domain);
        let (success, elapsed, proof_hash, _, _) = run_binary(binary, *count as u64, export_dir);
        let rate = if elapsed > 0.0 { *count as f64 / elapsed } else { 0.0 };
        if success {
            println!(
                "         {} trajectories in {:.2}s ({:.0}/sec)",
                count, elapsed, rate,
            );
        }
        results.push(SubstrateResult {
            binary: binary.to_string(),
            domain: domain.to_string(),
            count: *count as u64,
            elapsed_secs: elapsed,
            rate,
            proof_hash,
            success,
        });
    }

    // ICP Bridge
    println!("  [{}/{}] ICP Bridge...", counts.len() + 1, counts.len() + 1);
    let (success, elapsed, proof_hash, _, _) = run_binary("icp_bridge", icp_per as u64, None);
    let rate = if elapsed > 0.0 { icp_total as f64 / elapsed } else { 0.0 };
    if success {
        println!("         {} calls in {:.2}s ({:.1}/sec)", icp_total, elapsed, rate);
    }
    results.push(SubstrateResult {
        binary: "icp_bridge".to_string(),
        domain: "ICP (8 domains)".to_string(),
        count: icp_total as u64,
        elapsed_secs: elapsed,
        rate,
        proof_hash,
        success,
    });

    let sweep_elapsed = sweep_start.elapsed();

    println!();
    println!("═══════════════════════════════════════════════════════════");
    println!("  CORPUS SWEEP RESULTS");
    println!("═══════════════════════════════════════════════════════════");
    println!();

    let total_generated: u64 = results.iter().filter(|r| r.success).map(|r| r.count).sum();
    let total_failed: usize = results.iter().filter(|r| !r.success).count();

    println!("  ┌───────────────────┬───────────┬──────────┬──────────┬────────────────────────────────────┐");
    println!("  │ SUBSTRATE         │ COUNT     │ TIME     │ RATE     │ PROOF (first 32)                   │");
    println!("  ├───────────────────┼───────────┼──────────┼──────────┼────────────────────────────────────┤");

    for r in &results {
        let proof_str = match &r.proof_hash {
            Some(h) if h.len() >= 32 => format!("{}…", &h[..32]),
            Some(h) => h.clone(),
            None => "—".to_string(),
        };
        let fail = if r.success { "" } else { " FAIL" };
        println!(
            "  │ {:>17} │ {:>9} │ {:>7.2}s │ {:>7.0}/s│ {:>34} │{}",
            r.domain, r.count, r.elapsed_secs, r.rate, proof_str, fail,
        );
    }

    println!("  ├───────────────────┼───────────┼──────────┼──────────┼────────────────────────────────────┤");
    println!(
        "  │ TOTAL             │ {:>9} │ {:>7.1}s │          │                                    │",
        total_generated,
        sweep_elapsed.as_secs_f64(),
    );
    println!("  └───────────────────┴───────────┴──────────┴──────────┴────────────────────────────────────┘");
    println!();

    if total_failed > 0 {
        println!("  WARNING: {} substrate(s) failed", total_failed);
        println!();
    }

    let sub_proofs: Vec<String> = results.iter().filter_map(|r| r.proof_hash.clone()).collect();
    let master_proof = proof::seal_run(&sub_proofs);

    println!("  MASTER SHA-256 CORPUS PROOF: {}", master_proof);
    println!("  ({} sub-proofs sealed)", sub_proofs.len());
    println!();

    let local_rate: f64 = results
        .iter()
        .filter(|r| r.success && r.binary != "icp_bridge")
        .map(|r| r.rate)
        .sum();
    let local_per_sweep: u64 = results
        .iter()
        .filter(|r| r.success && r.binary != "icp_bridge")
        .map(|r| r.count)
        .sum();

    if local_per_sweep > 0 && local_rate > 0.0 {
        let sweeps_to_100m = (100_000_000.0 / local_per_sweep as f64).ceil() as u64;
        let time_to_100m_hours = (100_000_000.0 / local_rate) / 3600.0;
        println!("  ┌─────────────────────────────────────────────┐");
        println!("  │ PROJECTION TO 100M                          │");
        println!("  ├─────────────────────────────────────────────┤");
        println!("  │ Per sweep:     {:>9} local trajectories │", local_per_sweep);
        println!("  │ Sweep rate:    {:>9.0} traj/sec           │", local_rate);
        println!("  │ Sweeps needed: {:>9}                    │", sweeps_to_100m);
        println!("  │ Time to 100M:  {:>9.1} hours              │", time_to_100m_hours);
        println!("  └─────────────────────────────────────────────┘");
        println!();
    }

    println!("═══════════════════════════════════════════════════════════");
    println!("  G^G CORPUS SWEEP — SEALED");
    println!("  {} trajectories across {} substrates", total_generated, counts.len() + 1);
    println!("  {} local Monte Carlos + 1 ICP Bridge", counts.len());
    println!("  Master proof: {}…", &master_proof[..32]);
    println!("  The corpus breathes.");
    println!("═══════════════════════════════════════════════════════════");
}

// ═══════════════════════════════════════════════════════════════
//  MAIN — dispatch to burst or sequential
// ═══════════════════════════════════════════════════════════════

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let export_dir = if let Some(pos) = args.iter().position(|a| a == "--export") {
        args.get(pos + 1).cloned()
    } else {
        None
    };

    if let Some(pos) = args.iter().position(|a| a == "--burst") {
        let target: u64 = args
            .get(pos + 1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(94_500_000); // default: what's needed to reach 100M
        burst_mode(target, export_dir.as_deref());
    } else {
        sequential_mode(&args, export_dir.as_deref());
    }
}
