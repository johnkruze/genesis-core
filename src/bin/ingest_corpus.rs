use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use serde_json::Value;
use flatbuffers::FlatBufferBuilder;
use genesis_core::schema::trajectory_generated::kid_cosmo::*;

fn main() -> std::io::Result<()> {
    // Scaffold logic to ingest the large trajectory JSON into a FlatBuffer .fb file
    let json_path = Path::new("../KidCosmo_Physics_Trajectories.json");
    if !json_path.exists() {
        println!("JSON corpus not found locally. Please ensure KidCosmo_Physics_Trajectories.json is present in G^G.");
        return Ok(());
    }

    println!("Loading JSON Corpus from disk...");
    let mut file = File::open(json_path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;

    println!("Parsing JSON strings... (may take substantial RAM)");
    let v: Value = serde_json::from_str(&contents).expect("Failed to parse Corpus JSON");

    println!("Initializing FlatBuffer Memory Arena...");
    let mut builder = FlatBufferBuilder::with_capacity(1024 * 1024 * 1024); // 1GB pre-alloc

    let mut flat_trajectories = Vec::new();

    let domains_array = v.get("domains").and_then(|d| d.as_array()).expect("No domains array found in JSON");
    
    for domain_json in domains_array {
        let traj_arr = domain_json.get("trajectories").and_then(|t| t.as_array()).expect("No trajectories array in domain");
        
        for traj_json in traj_arr {
            // Map Telemetry vector from "data" instead of a single object, since "data" is an array
            let data_arr = traj_json.get("data").and_then(|v| v.as_array());
            let mut telemetries = Vec::new();
            
            if let Some(da) = data_arr {
                for t in da {
                    let phase_str = builder.create_string(t.get("phase").and_then(|v| v.as_str()).unwrap_or("UNKNOWN"));
                    
                    let mut px = 0.0; let mut py = 0.0; let mut pz = 0.0;
                    if let Some(pos_arr) = t.get("pos").and_then(|v| v.as_array()) {
                        px = pos_arr.get(0).and_then(|v| v.as_f64()).unwrap_or(0.0);
                        py = pos_arr.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);
                        pz = pos_arr.get(2).and_then(|v| v.as_f64()).unwrap_or(0.0);
                    }
                    let pos_struct = Vector3::new(px, py, pz);

                    let telemetry = TelemetrySnapshot::create(
                        &mut builder,
                        &TelemetrySnapshotArgs {
                            t: t.get("t").and_then(|v| v.as_f64()).unwrap_or(0.0),
                            alt: t.get("alt").and_then(|v| v.as_f64()).unwrap_or(0.0),
                            vel: t.get("vel").and_then(|v| v.as_f64()).unwrap_or(0.0),
                            phase: Some(phase_str),
                            pos: Some(&pos_struct),
                            rho: t.get("rho").and_then(|v| v.as_f64()).unwrap_or(0.0),
                        }
                    );
                    telemetries.push(telemetry);
                }
            }
            let t_vec = builder.create_vector(&telemetries);

            let r = traj_json.get("reasoning_context").unwrap_or(&Value::Null);
            let anom_str = builder.create_string(r.get("anomaly_type").and_then(|v| v.as_str()).unwrap_or(""));
            
            let r_telemetry = if r.is_object() && r.get("snapshot").is_some() && !r.get("snapshot").unwrap().is_null() {
                let r_snap = r.get("snapshot").unwrap();
                let phase_r_str = builder.create_string(r_snap.get("phase").and_then(|v| v.as_str()).unwrap_or("UNKNOWN"));
                
                let mut rx = 0.0; let mut ry = 0.0; let mut rz = 0.0;
                if let Some(r_pos_arr) = r_snap.get("pos").and_then(|v| v.as_array()) {
                    rx = r_pos_arr.get(0).and_then(|v| v.as_f64()).unwrap_or(0.0);
                    ry = r_pos_arr.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);
                    rz = r_pos_arr.get(2).and_then(|v| v.as_f64()).unwrap_or(0.0);
                }
                let r_pos_struct = Vector3::new(rx, ry, rz);
                
                Some(TelemetrySnapshot::create(&mut builder, &TelemetrySnapshotArgs {
                    t: r_snap.get("t").and_then(|v| v.as_f64()).unwrap_or(0.0),
                    alt: r_snap.get("alt").and_then(|v| v.as_f64()).unwrap_or(0.0),
                    vel: r_snap.get("vel").and_then(|v| v.as_f64()).unwrap_or(0.0),
                    phase: Some(phase_r_str),
                    pos: Some(&r_pos_struct),
                    rho: r_snap.get("rho").and_then(|v| v.as_f64()).unwrap_or(0.0),
                }))
            } else {
                None
            };

            let reasoning = ReasoningContext::create(
                &mut builder,
                &ReasoningContextArgs {
                    is_anomaly: r.get("is_anomaly").and_then(|v| v.as_bool()).unwrap_or(false),
                    anomaly_type: Some(anom_str),
                    snapshot: r_telemetry,
                }
            );

            let score_val = traj_json.get("score").unwrap_or(&Value::Null);
            let score = TrajectoryScore::create(
                &mut builder,
                &TrajectoryScoreArgs {
                    mission_success: score_val.get("mission_success").and_then(|v| v.as_bool()).unwrap_or(false),
                    final_velocity: score_val.get("final_velocity").and_then(|v| v.as_f64()).unwrap_or(0.0),
                    dispersion: score_val.get("dispersion").and_then(|v| v.as_f64()).unwrap_or(0.0),
                }
            );

            let uid = traj_json.get("id").and_then(|v| v.as_str()).unwrap_or("M_UNKNOWN");
            let uid_str = builder.create_string(uid);

            let typ = traj_json.get("type").and_then(|v| v.as_str()).unwrap_or("TERRAN");
            let typ_str = builder.create_string(typ);
            
            let hash = traj_json.get("trajectory_hash").and_then(|v| v.as_str()).unwrap_or("");
            let hash_str = builder.create_string(hash);

            let fb_traj = Trajectory::create(
                &mut builder,
                &TrajectoryArgs {
                    id: Some(uid_str),
                    type_: Some(typ_str), 
                    odin_backtest: traj_json.get("odin_backtest").and_then(|v| v.as_bool()).unwrap_or(false),
                    score: Some(score),
                    trajectory_hash: Some(hash_str),
                    reasoning_context: Some(reasoning),
                    data: Some(t_vec),
                }
            );

            flat_trajectories.push(fb_traj);
        }
    }

    let trajectory_vector = builder.create_vector(&flat_trajectories);
    
    // Map Meta
    let doc_str = builder.create_string(v.get("document_type").and_then(|v| v.as_str()).unwrap_or(""));
    let title_str = builder.create_string(v.get("title").and_then(|v| v.as_str()).unwrap_or(""));
    let author_str = builder.create_string(v.get("author").and_then(|v| v.as_str()).unwrap_or(""));
    let date_str = builder.create_string(v.get("date").and_then(|v| v.as_str()).unwrap_or(""));
    let thesis_str = builder.create_string(v.get("thesis_reference").and_then(|v| v.as_str()).unwrap_or(""));
    let hardware_str = builder.create_string(v.get("hardware").and_then(|v| v.as_str()).unwrap_or(""));
    let integrity_str = builder.create_string(v.get("integrity").and_then(|v| v.as_str()).unwrap_or(""));
    let k_str = builder.create_string(v.get("corpus_summary").unwrap().get("k_index").and_then(|v| v.as_str()).unwrap_or(""));


    let collection = TrajectoryCollection::create(
        &mut builder,
        &TrajectoryCollectionArgs {
            document_type: Some(doc_str),
            title: Some(title_str),
            author: Some(author_str),
            date: Some(date_str),
            thesis_reference: Some(thesis_str),
            hardware: Some(hardware_str),
            integrity: Some(integrity_str),
            total_trajectories: flat_trajectories.len() as i32,
            domains: domains_array.len() as i32,
            k_index: Some(k_str),
            trajectories: Some(trajectory_vector),
        }
    );

    builder.finish(collection, None);

    let buf = builder.finished_data();
    println!("FlatBuffer Binary Corpus Generated. Total Size: {} bytes. Writing to disk...", buf.len());

    let mut out_file = File::create("../KidCosmo_Physics_Trajectories.fb")?;
    out_file.write_all(buf)?;

    println!("Substrate Binary Sealing Complete. {} targeted in zero-copy paradigm.", flat_trajectories.len());
    
    Ok(())
}
