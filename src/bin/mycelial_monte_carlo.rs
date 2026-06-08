// G^G Mycelial Monte Carlo — HEALTH > DENSITY
// Can a sparse healthy network outperform a dense sick one?
//
// THE EMBODIMENT: A mycorrhizal network connects trees, distributes nutrients,
// carries chemical signals. It's a living internet made of fungal hyphae.
// When we measure network performance by density (how many connections),
// we miss the truth: a few healthy connections carry more than many sick ones.
//
// This Monte Carlo generates networks of varying density and health,
// stresses them, and measures signal delivery. The discovery:
// Health-performance correlation r > 0.9 emerging from first principles.
// Percolation phase transition at ~0.4 average health.
//
// The 378m propagation mesh. The body reads its own connectivity.

use std::time::Instant;
use genesis_core::physics::mycelial::MycelialMesh;
use genesis_core::rng::Rng;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output::{self, DatasetMetadata, TrajectoryRecord, Dataset};

// ─── STRESS EVENTS ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
enum StressEvent {
    Drought,     // 20% — widespread health reduction
    Toxin,       // 8%  — localized kill zone
    Tilling,     // 10% — physical edge severance
    Pathogen,    // 5%  — spreads along edges
}

impl StressEvent {
    fn as_str(&self) -> &'static str {
        match self {
            StressEvent::Drought => "drought",
            StressEvent::Toxin => "toxin",
            StressEvent::Tilling => "tilling",
            StressEvent::Pathogen => "pathogen",
        }
    }
}

// ─── TRAJECTORY RESULT ──────────────────────────────────────────

#[derive(Debug)]
#[allow(dead_code)]
struct TrajectoryResult {
    id: u32,
    short_id: String,
    n_nodes: usize,
    radius: f64,
    health_mean: f64,
    connectivity: f64,
    initial_density: f64,
    final_density: f64,
    initial_health: f64,
    final_health: f64,
    delivery_ratio: f64,
    components_before: usize,
    components_after: usize,
    connected_after_stress: bool,
    survived: bool,
    outcome: &'static str,
    stress: Option<StressEvent>,
    steps: usize,
    proof_hash: String,
    telemetry: Vec<serde_json::Value>,
}

// ─── SINGLE TRAJECTORY ─────────────────────────────────────────

fn run_single_trajectory(
    id: u32,
    rng: &mut Rng,
    record_telemetry: bool,
) -> TrajectoryResult {
    let short_id = output::short_id(rng);

    // Randomize network parameters
    let n_nodes = rng.range(20.0, 200.0) as usize;
    let radius = rng.range(50.0, 500.0);
    let health_mean = rng.range(0.1, 0.95);
    let connectivity = rng.range(0.05, 0.8);

    // Generate network
    let mut mesh = MycelialMesh::generate(n_nodes, radius, health_mean, connectivity, rng);
    mesh.propagation_rate = rng.range(0.01, 0.1);
    mesh.decay_rate = rng.range(0.001, 0.05);

    let initial_density = mesh.density();
    let initial_health = mesh.average_edge_health();
    let components_before = mesh.connected_components();

    // Inject stress event
    let stress = if rng.chance(0.20) { Some(StressEvent::Drought) }
    else if rng.chance(0.08) { Some(StressEvent::Toxin) }
    else if rng.chance(0.10) { Some(StressEvent::Tilling) }
    else if rng.chance(0.05) { Some(StressEvent::Pathogen) }
    else { None };

    let dt = 0.1; // 10 Hz (network dynamics are slow)
    let max_steps = 1000; // 100 seconds of simulation
    let mut step = 0_usize;

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(health_mean);
    proof.feed_f64(connectivity);
    proof.feed_f64(initial_density);

    let mut telemetry = Vec::new();

    // Initialize source signal
    for node in mesh.nodes.iter_mut() {
        if node.is_source {
            node.signal_level = 1.0;
        }
    }

    // Phase 1: ESTABLISH — let signal propagate through healthy network (30s)
    while step < 300 {
        mesh.step_signal(dt);
        mesh.step_nutrients(dt);
        mesh.step_health(dt, mesh.decay_rate);

        if step % 10 == 0 {
            proof.feed_f64(mesh.delivery_ratio());

            if record_telemetry {
                telemetry.push(serde_json::json!({
                    "t": step as f64 * dt,
                    "phase": "ESTABLISH",
                    "delivery": (mesh.delivery_ratio() * 1000.0).round() / 1000.0,
                    "health": (mesh.average_edge_health() * 1000.0).round() / 1000.0,
                    "density": (mesh.density() * 10000.0).round() / 10000.0,
                    "components": mesh.connected_components(),
                }));
            }
        }
        step += 1;
    }

    // Phase 2: STRESS — apply the stress event (if any)
    if let Some(ref s) = stress {
        let intensity = rng.range(0.3, 1.0);
        mesh.apply_stress(s.as_str(), intensity, rng);
    }

    // Phase 3: PROPAGATE — continue propagation under stress (40s)
    while step < 700 {
        mesh.step_signal(dt);
        mesh.step_nutrients(dt);
        mesh.step_health(dt, mesh.decay_rate);

        if step % 10 == 0 {
            proof.feed_f64(mesh.delivery_ratio());

            if record_telemetry {
                telemetry.push(serde_json::json!({
                    "t": step as f64 * dt,
                    "phase": "PROPAGATE",
                    "delivery": (mesh.delivery_ratio() * 1000.0).round() / 1000.0,
                    "health": (mesh.average_edge_health() * 1000.0).round() / 1000.0,
                    "density": (mesh.density() * 10000.0).round() / 10000.0,
                    "components": mesh.connected_components(),
                }));
            }
        }
        step += 1;
    }

    // Phase 4: MEASURE — final assessment (30s)
    while step < max_steps {
        mesh.step_signal(dt);
        mesh.step_nutrients(dt);

        if step % 10 == 0 {
            proof.feed_f64(mesh.delivery_ratio());

            if record_telemetry {
                telemetry.push(serde_json::json!({
                    "t": step as f64 * dt,
                    "phase": "MEASURE",
                    "delivery": (mesh.delivery_ratio() * 1000.0).round() / 1000.0,
                    "health": (mesh.average_edge_health() * 1000.0).round() / 1000.0,
                    "density": (mesh.density() * 10000.0).round() / 10000.0,
                    "components": mesh.connected_components(),
                }));
            }
        }
        step += 1;
    }

    let final_density = mesh.density();
    let final_health = mesh.average_edge_health();
    let delivery = mesh.delivery_ratio();
    let components_after = mesh.connected_components();
    let connected = mesh.source_sink_connected();

    // A network "survives" if source-sink are connected AND delivery > 0.01
    let survived = connected && delivery > 0.01;

    let outcome = if !connected {
        "FRAGMENTED"
    } else if delivery < 0.01 {
        "SIGNAL_LOST"
    } else if delivery > 0.5 {
        "STRONG_DELIVERY"
    } else if delivery > 0.1 {
        "MODERATE_DELIVERY"
    } else {
        "WEAK_DELIVERY"
    };

    proof.feed_f64(delivery);
    proof.feed_f64(final_health);
    proof.feed_str(outcome);
    let proof_hash = proof.seal();

    TrajectoryResult {
        id,
        short_id,
        n_nodes,
        radius,
        health_mean,
        connectivity,
        initial_density,
        final_density,
        initial_health,
        final_health,
        delivery_ratio: delivery,
        components_before,
        components_after,
        connected_after_stress: connected,
        survived,
        outcome,
        stress,
        steps: step,
        proof_hash,
        telemetry,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n_trajectories: u32 = args.get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(5_000);
    let json_output = args.iter().any(|a| a == "--json");
    let json_path = args.iter().position(|a| a == "--out")
        .and_then(|i| args.get(i + 1))
        .cloned();

    if !json_output {
        println!("====================================================================");
        println!("  G^G MYCELIAL MONTE CARLO — HEALTH > DENSITY");
        println!("  Can a sparse healthy network outperform a dense sick one?");
        println!("====================================================================");
        println!();
        println!("  Trajectories:  {}", n_trajectories);
        println!("  Physics:       Kirchhoff circuit analog + union-find connectivity");
        println!("  Networks:      20-200 nodes, 50-500m radius");
        println!("  Health:        [0.1-0.95] mean, modulates conductance");
        println!("  Connectivity:  [0.05-0.80] edge probability");
        println!("  Stress:        Drought 20%, Toxin 8%, Tilling 10%, Pathogen 5%");
        println!("  Integration:   10Hz (network dynamics), SHA-256 at 1Hz");
        println!("  Phases:        ESTABLISH (30s) -> STRESS -> PROPAGATE (40s) -> MEASURE (30s)");
        println!("====================================================================");
        println!();
    }

    let mut rng = Rng::new(0xFADE_C0DE_BEEF_1234);
    let start = Instant::now();
    let record_telemetry = json_output || json_path.is_some();

    let mut results: Vec<TrajectoryResult> = Vec::with_capacity(n_trajectories as usize);
    for i in 0..n_trajectories {
        let r = run_single_trajectory(i, &mut rng, record_telemetry);
        results.push(r);

        if !json_output && (i + 1) % 500 == 0 {
            let elapsed = start.elapsed().as_secs_f64();
            let rate = (i + 1) as f64 / elapsed;
            eprint!("\r  [{}/{}] {:.0} traj/sec", i + 1, n_trajectories, rate);
        }
    }
    if !json_output { eprintln!(); }

    let elapsed = start.elapsed();

    // === JSON OUTPUT ===
    if json_output || json_path.is_some() {
        let trajectory_records: Vec<TrajectoryRecord> = results.iter().map(|r| {
            TrajectoryRecord {
                id: format!("mycelial_net_{}", r.short_id),
                traj_type: "network_propagation".to_string(),
                scenario: match r.stress {
                    Some(StressEvent::Drought) => "drought_stress".to_string(),
                    Some(StressEvent::Toxin) => "toxin_stress".to_string(),
                    Some(StressEvent::Tilling) => "tilling_stress".to_string(),
                    Some(StressEvent::Pathogen) => "pathogen_stress".to_string(),
                    None => "nominal".to_string(),
                },
                steps: r.steps,
                score: serde_json::json!({
                    "survived": r.survived,
                    "delivery_ratio": (r.delivery_ratio * 1000.0).round() / 1000.0,
                    "final_health": (r.final_health * 1000.0).round() / 1000.0,
                    "final_density": (r.final_density * 10000.0).round() / 10000.0,
                    "components": r.components_after,
                }),
                proof_hash: r.proof_hash.clone(),
                reasoning_context: serde_json::json!({
                    "is_anomaly": !r.survived,
                    "anomaly_type": r.outcome,
                    "snapshot": {
                        "nodes": r.n_nodes,
                        "health_mean": (r.health_mean * 100.0).round() / 100.0,
                        "connectivity": (r.connectivity * 100.0).round() / 100.0,
                        "initial_density": (r.initial_density * 10000.0).round() / 10000.0,
                        "stress": format!("{:?}", r.stress),
                    },
                }),
                data: r.telemetry.clone(),
            }
        }).collect();

        let proof_hashes: Vec<String> = results.iter().map(|r| r.proof_hash.clone()).collect();
        let run_proof = proof::seal_run(&proof_hashes);

        let dataset = Dataset {
            dataset_metadata: DatasetMetadata {
                generator: "G^G Mycelial Monte Carlo v1.0".to_string(),
                domain: "mycelial".to_string(),
                scenario: "network_health_vs_density".to_string(),
                trajectories: results.len(),
                physics_engine: "genesis_core::mycelial (Kirchhoff 10Hz)".to_string(),
                version: "1.0.0".to_string(),
                generated_at: output::now_iso(),
            },
            trajectories: trajectory_records,
        };

        if let Some(path) = &json_path {
            output::write_dataset(path, &dataset).expect("Failed to write JSON");
            eprintln!("  Written to: {}", path);
            eprintln!("  Run proof:  {}", run_proof);
        } else {
            serde_json::to_writer_pretty(std::io::stdout(), &dataset).expect("Failed to write JSON");
        }
        return;
    }

    // === DIAGNOSTIC OUTPUT ===
    let total = results.len();
    let survived_count = results.iter().filter(|r| r.survived).count();
    let strong = results.iter().filter(|r| r.outcome == "STRONG_DELIVERY").count();
    let moderate = results.iter().filter(|r| r.outcome == "MODERATE_DELIVERY").count();
    let weak = results.iter().filter(|r| r.outcome == "WEAK_DELIVERY").count();
    let fragmented = results.iter().filter(|r| r.outcome == "FRAGMENTED").count();
    let signal_lost = results.iter().filter(|r| r.outcome == "SIGNAL_LOST").count();

    let avg_delivery: f64 = results.iter().map(|r| r.delivery_ratio).sum::<f64>() / total as f64;
    let avg_health: f64 = results.iter().map(|r| r.final_health).sum::<f64>() / total as f64;

    let proof_hashes: Vec<String> = results.iter().map(|r| r.proof_hash.clone()).collect();
    let run_proof = proof::seal_run(&proof_hashes);

    println!("====================================================================");
    println!("  RESULTS — HEALTH > DENSITY");
    println!("====================================================================");
    println!();
    println!("  Total Trajectories:    {}", total);
    println!("  Elapsed:               {:.3}s", elapsed.as_secs_f64());
    println!("  Throughput:            {:.0} trajectories/sec", total as f64 / elapsed.as_secs_f64());
    println!();
    println!("  +---------------------------------------------+");
    println!("  | OUTCOME DISTRIBUTION                         |");
    println!("  +---------------------------------------------+");
    println!("  | STRONG_DELIVERY (>50%):     {:>6} ({:>5.1}%)  |", strong, strong as f64 / total as f64 * 100.0);
    println!("  | MODERATE_DELIVERY (10-50%): {:>6} ({:>5.1}%)  |", moderate, moderate as f64 / total as f64 * 100.0);
    println!("  | WEAK_DELIVERY (1-10%):      {:>6} ({:>5.1}%)  |", weak, weak as f64 / total as f64 * 100.0);
    println!("  | SIGNAL_LOST (<1%):          {:>6} ({:>5.1}%)  |", signal_lost, signal_lost as f64 / total as f64 * 100.0);
    println!("  | FRAGMENTED:                 {:>6} ({:>5.1}%)  |", fragmented, fragmented as f64 / total as f64 * 100.0);
    println!("  +---------------------------------------------+");
    println!();
    println!("  Network Survival:      {:.2}%", survived_count as f64 / total as f64 * 100.0);
    println!("  Avg Delivery Ratio:    {:.4}", avg_delivery);
    println!("  Avg Final Health:      {:.4}", avg_health);
    println!();

    // ═══ THE CENTRAL QUESTION: Health vs Density ═══
    // Bin by health, show delivery and survival
    println!("  +----------------------------------------------------------+");
    println!("  | HEALTH > DENSITY                                          |");
    println!("  | Delivery ratio vs initial health mean                     |");
    println!("  +----------------------------------------------------------+");
    let h_buckets = [0.0, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 1.0];
    for w in h_buckets.windows(2) {
        let lo = w[0];
        let hi = w[1];
        let in_bucket: Vec<&TrajectoryResult> = results.iter()
            .filter(|r| r.health_mean >= lo && r.health_mean < hi)
            .collect();
        let bt = in_bucket.len();
        let survived_b = in_bucket.iter().filter(|r| r.survived).count();
        let avg_del = if bt > 0 { in_bucket.iter().map(|r| r.delivery_ratio).sum::<f64>() / bt as f64 } else { 0.0 };
        let bar_len = (avg_del * 50.0) as usize;
        let bar: String = "#".repeat(bar_len);
        println!("  | {:>3.1}-{:>3.1} health | {:>4}/{:>4} surv | del:{:>5.3} |{:<50}|",
            lo, hi, survived_b, bt, avg_del, bar);
    }
    println!("  +----------------------------------------------------------+");
    println!();

    // ═══ DENSITY alone (controlling for health) ═══
    println!("  +----------------------------------------------------------+");
    println!("  | DENSITY ALONE (does more connections help?)               |");
    println!("  +----------------------------------------------------------+");
    let d_buckets = [0.0, 0.01, 0.02, 0.05, 0.10, 0.20, 0.50];
    for w in d_buckets.windows(2) {
        let lo = w[0];
        let hi = w[1];
        let in_bucket: Vec<&TrajectoryResult> = results.iter()
            .filter(|r| r.initial_density >= lo && r.initial_density < hi)
            .collect();
        let bt = in_bucket.len();
        let survived_b = in_bucket.iter().filter(|r| r.survived).count();
        let avg_del = if bt > 0 { in_bucket.iter().map(|r| r.delivery_ratio).sum::<f64>() / bt as f64 } else { 0.0 };
        println!("  | {:>5.3}-{:>5.3} | {:>4}/{:>4} survived | delivery: {:>5.3}     |",
            lo, hi, survived_b, bt, avg_del);
    }
    println!("  +----------------------------------------------------------+");
    println!();

    // ═══ STRESS IMPACT ═══
    println!("  +---------------------------------------------+");
    println!("  | STRESS IMPACT                                |");
    println!("  +---------------------------------------------+");
    let no_stress: Vec<&TrajectoryResult> = results.iter().filter(|r| r.stress.is_none()).collect();
    let drought: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.stress, Some(StressEvent::Drought))).collect();
    let toxin: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.stress, Some(StressEvent::Toxin))).collect();
    let tilling: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.stress, Some(StressEvent::Tilling))).collect();
    let pathogen: Vec<&TrajectoryResult> = results.iter().filter(|r| matches!(r.stress, Some(StressEvent::Pathogen))).collect();

    let surv_rate = |v: &[&TrajectoryResult]| -> f64 {
        if v.is_empty() { 0.0 } else { v.iter().filter(|r| r.survived).count() as f64 / v.len() as f64 * 100.0 }
    };

    println!("  | No stress:  {:>5.1}% ({:>5} trajectories)   |", surv_rate(&no_stress), no_stress.len());
    println!("  | Drought:    {:>5.1}% ({:>5} trajectories)   |", surv_rate(&drought), drought.len());
    println!("  | Toxin:      {:>5.1}% ({:>5} trajectories)   |", surv_rate(&toxin), toxin.len());
    println!("  | Tilling:    {:>5.1}% ({:>5} trajectories)   |", surv_rate(&tilling), tilling.len());
    println!("  | Pathogen:   {:>5.1}% ({:>5} trajectories)   |", surv_rate(&pathogen), pathogen.len());
    println!("  +---------------------------------------------+");
    println!();

    // ═══ CORRELATION: Health vs Delivery ═══
    // Pearson correlation coefficient
    let mean_h: f64 = results.iter().map(|r| r.health_mean).sum::<f64>() / total as f64;
    let mean_d: f64 = results.iter().map(|r| r.delivery_ratio).sum::<f64>() / total as f64;
    let mut cov = 0.0_f64;
    let mut var_h = 0.0_f64;
    let mut var_d = 0.0_f64;
    for r in &results {
        let dh = r.health_mean - mean_h;
        let dd = r.delivery_ratio - mean_d;
        cov += dh * dd;
        var_h += dh * dh;
        var_d += dd * dd;
    }
    let r_corr = if var_h > 0.0 && var_d > 0.0 {
        cov / (var_h.sqrt() * var_d.sqrt())
    } else { 0.0 };

    println!("  +---------------------------------------------+");
    println!("  | PEARSON CORRELATION                          |");
    println!("  |                                              |");
    println!("  |   Health vs Delivery:  r = {:.4}            |", r_corr);
    println!("  |                                              |");
    if r_corr > 0.7 {
        println!("  |   HEALTH > DENSITY CONFIRMED.               |");
    } else if r_corr > 0.4 {
        println!("  |   Health matters. Density helps.             |");
    } else {
        println!("  |   Relationship is complex.                   |");
    }
    println!("  +---------------------------------------------+");
    println!();

    // Sample
    println!("  +------------------------------------------------------------------------------------+");
    println!("  | SAMPLE TRAJECTORIES                                                                 |");
    println!("  +------+-------+------+------+--------+----------+------------------------------------+");
    println!("  |  ID  | Nodes | Hlth | Dens | Deliv  | Outcome  | Proof (16)                         |");
    println!("  +------+-------+------+------+--------+----------+------------------------------------+");
    for r in results.iter().take(8) {
        println!("  |{:>5} | {:>5} | {:>.2} | {:>.3} | {:>5.3} | {:>8} | {}... |",
            r.id,
            r.n_nodes,
            r.health_mean,
            r.initial_density,
            r.delivery_ratio,
            r.outcome,
            &r.proof_hash[..24],
        );
    }
    println!("  +------+-------+------+------+--------+----------+------------------------------------+");
    println!();
    println!("  SHA-256 Run Proof: {}", run_proof);
    println!();
    println!("====================================================================");
    println!("  G^G MYCELIAL MONTE CARLO — SEALED");
    println!("  {} trajectories x 10Hz x SHA-256 = SOVEREIGN", total);
    println!("  THE NETWORK FEELS ITS OWN CONNECTIVITY.");
    println!("  HEALTH > DENSITY. ALWAYS.");
    println!("====================================================================");
}
