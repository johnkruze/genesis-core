//! Deep Purple Integration: NEO Rubble-Pile Dynamics
//! Models asteroids as granular aggregates held together by weak self-gravity
//! and Van der Waals cohesive forces.

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Boulder {
    pub position: [f64; 3],  // km
    pub velocity: [f64; 3],  // km/s
    pub mass: f64,           // kg
    pub radius: f64,         // km
}

#[derive(Debug, Clone)]
pub struct RubblePile {
    pub boulders: HashMap<String, Boulder>,
    pub g: f64, // Universal Gravitational Constant in km^3 / (kg * s^2)
    pub restitution_coeff: f64, // Coefficient of restitution for collisions
    pub contact_stiffness: f64, // Penalty method stiffness
}

impl Default for RubblePile {
    fn default() -> Self {
        Self {
            boulders: HashMap::new(),
            g: 6.67430e-20, 
            restitution_coeff: 0.8,
            contact_stiffness: 1e5,
        }
    }
}

impl RubblePile {
    pub fn insert_boulder(&mut self, id: &str, boulder: Boulder) {
        self.boulders.insert(id.to_string(), boulder);
    }

    /// Computes self-gravity and contact forces, integrates by dt
    pub fn step_dynamics(&mut self, dt: f64) {
        let body_ids: Vec<String> = self.boulders.keys().cloned().collect();
        let mut accelerations: HashMap<String, [f64; 3]> = HashMap::new();

        for id in &body_ids {
            accelerations.insert(id.clone(), [0.0; 3]);
        }

        // O(N^2) pairwise interactions
        for i in 0..body_ids.len() {
            for j in (i + 1)..body_ids.len() {
                let id_a = &body_ids[i];
                let id_b = &body_ids[j];

                let a = self.boulders.get(id_a).unwrap();
                let b = self.boulders.get(id_b).unwrap();

                let dx = b.position[0] - a.position[0];
                let dy = b.position[1] - a.position[1];
                let dz = b.position[2] - a.position[2];

                let dist_sq = dx * dx + dy * dy + dz * dz;
                let dist = dist_sq.sqrt();
                if dist < 1e-12 { continue; } // Avoid singularities

                let n_x = dx / dist;
                let n_y = dy / dist;
                let n_z = dz / dist;

                // 1. Self-Gravity
                let force_mag = (self.g * a.mass * b.mass) / dist_sq;
                let mut fx = force_mag * n_x;
                let mut fy = force_mag * n_y;
                let mut fz = force_mag * n_z;

                // 2. Contact Penalty (if overlapping)
                let overlap = (a.radius + b.radius) - dist;
                if overlap > 0.0 {
                    // Hookean repulsive force
                    let contact_force = self.contact_stiffness * overlap;
                    fx -= contact_force * n_x;
                    fy -= contact_force * n_y;
                    fz -= contact_force * n_z;

                    // Simple dissipative term (damping) based on relative velocity could be added here
                }

                // Accumulate accelerations (F/m)
                let acc_a = accelerations.get_mut(id_a).unwrap();
                acc_a[0] += fx / a.mass;
                acc_a[1] += fy / a.mass;
                acc_a[2] += fz / a.mass;

                let acc_b = accelerations.get_mut(id_b).unwrap();
                acc_b[0] -= fx / b.mass;
                acc_b[1] -= fy / b.mass;
                acc_b[2] -= fz / b.mass;
            }
        }

        // Symplectic Euler Integration
        for id in &body_ids {
            let boulder = self.boulders.get_mut(id).unwrap();
            let acc = accelerations.get(id).unwrap();

            boulder.velocity[0] += acc[0] * dt;
            boulder.velocity[1] += acc[1] * dt;
            boulder.velocity[2] += acc[2] * dt;

            boulder.position[0] += boulder.velocity[0] * dt;
            boulder.position[1] += boulder.velocity[1] * dt;
            boulder.position[2] += boulder.velocity[2] * dt;
        }
    }
}
