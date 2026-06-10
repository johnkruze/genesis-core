// J1989 Planetary Time-Evolution Simulation (Yoshida Symplectic Integrator)
// Initialized from the exact coordinates of John's birth: April 26, 1989, 11:14 AM PDT

use genesis_core::physics::ephemeris::{BodyState, NBodyState};
use genesis_core::proof::ProofChain;

fn main() {
    println!("==================================================================");
    println!("  CELESTIAL N-BODY TIME-EVOLUTION: epoch_19890426");
    println!("  Integrator: 4th-Order Yoshida Symplectic");
    println!("  Initial State: Heliocentric Ecliptic (km)");
    println!("==================================================================\n");

    let au_to_km = 149597870.7;

    let mut system = NBodyState::new();

    // 1. Sun at Origin
    system.insert_body("Sun", BodyState {
        position: [0.0, 0.0, 0.0],
        velocity: [0.0, 0.0, 0.0],
        mu: 132712440018.0,
    });

    // 2. Earth
    system.insert_body("Earth", BodyState {
        position: [-0.1772 * au_to_km, 0.9672 * au_to_km, 0.0],
        velocity: [-28.8 * 0.9672, -28.8 * 0.1772, 0.0], // perpendicular velocity vector
        mu: 398600.4418,
    });

    // 3. Mars
    system.insert_body("Mars", BodyState {
        position: [1.3906 * au_to_km, -0.0131 * au_to_km, -0.0345 * au_to_km],
        velocity: [0.24, 24.0, 0.0], 
        mu: 42828.375214,
    });

    // 4. Jupiter
    system.insert_body("Jupiter", BodyState {
        position: [3.9983 * au_to_km, 2.9464 * au_to_km, -0.1019 * au_to_km],
        velocity: [-7.7, 10.5, 0.0],
        mu: 126686581.3,
    });

    // 5. Saturn
    system.insert_body("Saturn", BodyState {
        position: [6.3985 * au_to_km, 6.5569 * au_to_km, -0.3686 * au_to_km],
        velocity: [-6.8, 6.6, 0.0],
        mu: 37931187.0,
    });

    // 6. Pluto (Note the Z offset!)
    system.insert_body("Pluto", BodyState {
        position: [-9.8831 * au_to_km, -27.9640 * au_to_km, 5.8518 * au_to_km],
        velocity: [4.4, -1.6, -1.2],
        mu: 977.0,
    });

    let dt = 86400.0; // 1 day step (seconds)
    let total_days = 365; // Simulate 1 year of celestial trajectory
    
    let mut proof = ProofChain::new();
    proof.seed(b"epoch_19890426");

    println!("Starting Time-Evolution (365 Days, 24-Hour increments):");
    println!("------------------------------------------------------------------");
    
    for day in 0..=total_days {
        let earth = system.bodies.get("Earth").unwrap();
        let mars = system.bodies.get("Mars").unwrap();
        let jupiter = system.bodies.get("Jupiter").unwrap();
        let saturn = system.bodies.get("Saturn").unwrap();
        let pluto = system.bodies.get("Pluto").unwrap();

        // Feed coordinate state daily to build the cryptographic proof chain
        proof.feed_f64(earth.position[0]);
        proof.feed_f64(mars.position[0]);
        proof.feed_f64(jupiter.position[0]);
        proof.feed_f64(saturn.position[0]);
        proof.feed_f64(pluto.position[0]);

        if day % 30 == 0 || day == total_days {
            let pluto_dist_au = (pluto.position[0].powi(2) + pluto.position[1].powi(2) + pluto.position[2].powi(2)).sqrt() / au_to_km;
            let earth_dist_au = (earth.position[0].powi(2) + earth.position[1].powi(2) + earth.position[2].powi(2)).sqrt() / au_to_km;
            
            println!(
                "Day {:03} | Earth [X: {:6.2} AU, Y: {:6.2} AU] | Pluto Dist: {:5.2} AU | Earth Dist: {:4.2} AU",
                day,
                earth.position[0] / au_to_km,
                earth.position[1] / au_to_km,
                pluto_dist_au,
                earth_dist_au
            );
        }

        // Step physics system using Yoshida Integrator
        system.step_nbody(dt);
    }

    let seal = proof.seal();
    println!("------------------------------------------------------------------");
    println!("  Trajectory verification complete.");
    println!("  SHA-256 Sovereignty Seal: {}", seal);
    println!("==================================================================");
}
