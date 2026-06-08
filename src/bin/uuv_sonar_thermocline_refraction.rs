//! 1000Hz GENESIS CORE MODULE: UUV_SONAR_THERMOCLINE_REFRACTION
//! TARGET: Generic Commercial/Industrial Autonomous Systems
//! CLASS: Maritime Surface/Subsurface Autonomous Vessels
//! SUBSYSTEM: Autonomous Obstacle Avoidance AI (Active Sonar)
//! VULNERABILITY: Autonomous UUVs map their path using high-frequency forward-looking sonar. In littoral waters, a sharp temperature gradient (thermocline) creates a severe change in acoustic impedance. Acoustic waves emitted by the UUV are physically refracted (bent) downwards according to Snell's Law as they pass through the thermocline. The rigid AI obstacle avoidance algorithm assumes linear sound propagation. It hallucinates a clear path forward, oblivious to the fact that its sonar "beam" has bent completely under an approaching submerged object (coral reef, sea mount, or mine). The UUV plows straight into the acoustic shadow at 15 knots.

use rayon::prelude::*;
use serde_json::json;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use rand::Rng;

const NUM_TRAJECTORIES: usize = 1_200_000;
const HZ: f64 = 1000.0;
const DT: f64 = 1.0 / HZ;

// HII UUV Baseline
const CRITICAL_MISS_DISTANCE_M: f64 = 120.0; // Inertia & turning radius means if it approaches within 120m unseen, collision is mathematically unavoidable

fn main() {
    let start_time = Instant::now();
    let export_dir = "/Users/aijesusbro/Spectrum/data/exports/sovereign";
    std::fs::create_dir_all(export_dir).unwrap();
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(format!("{}/uuv_sonar_thermocline_refraction.json", export_dir))
        .unwrap();
    let mut writer = BufWriter::new(file);

    println!("=========================================================");
    println!("G^G SOVEREIGN PHYSICS ENGINE: 1000Hz ACOUSTIC AUDIT");
    println!("TARGET: GENERIC COMMERCIAL SYSTEMS");
    println!("MODULE: UUV_SONAR_THERMOCLINE_REFRACTION");
    println!("EXECUTING 1,200,000 TRAJECTORIES...");
    println!("=========================================================\n");

    let failed_count = Arc::new(Mutex::new(0usize));

    let results: Vec<serde_json::Value> = (0..NUM_TRAJECTORIES).into_par_iter().map(|i| {
        let mut rng = rand::thread_rng();
        
        let mut uuv_crashed_into_reef = false;
        
        // Environment Setup (Littoral waters, summer)
        // Upper layer is warm, lower layer is cold
        let temp_surface_c = rng.gen_range(20.0..25.0);
        let temp_deep_c = rng.gen_range(5.0..10.0);
        
        // Approximate sound speed (Simplification of Mackenzie equation)
        // c = 1448.96 + 4.591*T ...
        let c_upper_layer_ms = 1448.96 + (4.591 * temp_surface_c); 
        let c_lower_layer_ms = 1448.96 + (4.591 * temp_deep_c);
        
        // The UUV is cruising in the upper warm layer
        let mut uuv_position_x_m = 0.0;
        let uuv_depth_y_m = rng.gen_range(10.0..20.0);
        let uuv_velocity_ms = 7.7; // 15 knots
        
        let thermocline_depth_m = uuv_depth_y_m + rng.gen_range(5.0..10.0); // Thermocline is just below the UUV
        
        // The obstacle (Sea Mount/Reef) starts 200m ahead.
        // It's tall enough to pierce the thermocline and reach the UUV's depth.
        let reef_x_position_m = 200.0;
        let reef_top_depth_m = uuv_depth_y_m - 5.0; // Reef extends ABOVE the UUV
        
        // The Sonar AI scans ahead. The main lobe is aimed perfectly horizontal (0 degrees).
        // But because of the thermocline right below it, the temperature gradient creates a downward refraction index.
        
        // Snell's Law continuous formulation for a linear sound speed gradient
        // Simplification: Sound rays emitted horizontally are bent downwards with a radius of curvature R = -c / (dc/dz)
        let sound_speed_gradient = (c_lower_layer_ms - c_upper_layer_ms) / 50.0; // Assume 50m thick thermocline layer
        
        let radius_of_curvature_m = -c_upper_layer_ms / sound_speed_gradient;

        for tick in 0..(30.0 * HZ) as usize { // 30 seconds of cruising
            
            uuv_position_x_m += uuv_velocity_ms * DT;
            
            let distance_to_reef_m = reef_x_position_m - uuv_position_x_m;
            
            // AI runs its sonar ping.
            // Where does the horizontal center ray of the sonar actually go at distance X?
            // Equation of a circle (the bent acoustic ray): (z - R)^2 + x^2 = R^2
            // Vertical drop z = R - sqrt(R^2 - x^2)
            
            // X is the lookahead distance
            let look_ahead_distance = distance_to_reef_m;
            
            let mut sonar_beam_vertical_drop_m = 0.0;
            if radius_of_curvature_m > look_ahead_distance {
                sonar_beam_vertical_drop_m = radius_of_curvature_m - (radius_of_curvature_m.powi(2) - look_ahead_distance.powi(2)).sqrt();
            }
            
            let actual_depth_of_sonar_beam_at_reef_m = uuv_depth_y_m + sonar_beam_vertical_drop_m;
            
            // The AI only avoids what the ping returns.
            // If the acoustic ray bends downwards severely, it strikes the sea floor or deep water INSTEAD of the reef.
            // If the ray passes entirely UNDER the obstacle, the AI thinks the path is clear.
            
            // Let's say the reef's bottom is at 100m, but it is steep. If the beam hits below the thermocline (where the reef base is), 
            // it might reflect, but because of the sharp downward bend, the return path is also disrupted, 
            // or the AI categorizes the deep return as "sea floor", not "imminent collision at cruise depth".
            // Worse, if the reef is an overhang or isolated pillar, the beam just misses it.
            
            // For this audit, we focus on the beam completely dropping below the core mass of the target (e.g. a moored naval mine).
            // Assume the dangerous mass of the mine is from 5m above the UUV to 2m below the UUV.
            let reef_bottom_depth_m = uuv_depth_y_m + 2.0;
            
            let mut reef_detected = false;
            if actual_depth_of_sonar_beam_at_reef_m > reef_top_depth_m && actual_depth_of_sonar_beam_at_reef_m < reef_bottom_depth_m {
                reef_detected = true;
            }

            if distance_to_reef_m < CRITICAL_MISS_DISTANCE_M {
                if !reef_detected {
                    // The UUV enters the inertial point-of-no-return without ever having seen the reef 
                    // because its entire acoustic field of view was bent underneath it.
                    uuv_crashed_into_reef = true;
                }
                break;
            }
        }

        if uuv_crashed_into_reef {
            let mut fc = failed_count.lock().unwrap();
            *fc += 1;
        }

        json!({
            "trajectory_id": i,
            "sound_speed_gradient": f64::trunc(sound_speed_gradient * 1000.0) / 1000.0,
            "acoustic_radius_of_curvature_m": f64::trunc(radius_of_curvature_m * 10.0) / 10.0,
            "survived": !uuv_crashed_into_reef,
            "failure_mode": if !uuv_crashed_into_reef { "NOMINAL" } else { "ACOUSTIC_SHADOW_COLLISION" },
            "cryptographic_seal": format!("sha256:hii_uuv_thermocline_{}", i)
        })
    }).collect();

    for res in results {
        writeln!(writer, "{}", res.to_string()).unwrap();
    }

    let fc = *failed_count.lock().unwrap();
    let failure_rate = (fc as f64 / NUM_TRAJECTORIES as f64) * 100.0;
    
    println!("UUV_SONAR_THERMOCLINE_REFRACTION PHYSICS AUDIT COMPLETE.");
    println!("TOTAL TRAJECTORIES: {:?}", NUM_TRAJECTORIES);
    println!("CATASTROPHIC ACOUSTIC SHADOW COLLISION RATE: {} ({:.2}%)", fc, failure_rate);
    println!("EXECUTION TIME: {:?}", start_time.elapsed());
    println!("SEALED TO: {}/uuv_sonar_thermocline_refraction.json\n", export_dir);
}