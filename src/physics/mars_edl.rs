//! genesis_core mars edl dynamics
//! Implements 0.38g, CO2 atmosphere (scale height 11.1km) drag, and mass parameters

#[derive(Debug, Clone, Copy)]
pub struct MarsPhysics {
    pub gravity: f64,
}

impl Default for MarsPhysics {
    fn default() -> Self {
        Self {
            gravity: 3.721, // m/s^2
        }
    }
}

impl MarsPhysics {
    /// rho = 0.020 * exp(-0.00009 * altitude_m)
    pub fn atmosphere_density(&self, altitude: f64) -> f64 {
        0.020 * (-0.00009 * altitude).exp()
    }

    /// F_d = 0.5 * rho * v^2 * C_d * A
    /// Returns [x, y, z] drag force opposing velocity
    pub fn calculate_drag(
        &self,
        velocity: [f64; 3],
        altitude: f64,
        area: f64,
        cd: f64,
    ) -> [f64; 3] {
        let speed = (velocity[0].powi(2) + velocity[1].powi(2) + velocity[2].powi(2)).sqrt();
        if speed < 1e-6 {
            return [0.0, 0.0, 0.0];
        }
        let rho = self.atmosphere_density(altitude);
        let drag_mag = 0.5 * rho * speed.powi(2) * cd * area;
        [
            -drag_mag * (velocity[0] / speed),
            -drag_mag * (velocity[1] / speed),
            -drag_mag * (velocity[2] / speed),
        ]
    }
}

#[derive(Debug, Clone, Copy)]
pub struct EdlVehicle {
    pub mass: f64,
    pub drag_area: f64,
    pub cd: f64,
}

impl Default for EdlVehicle {
    fn default() -> Self {
        Self {
            mass: 1000.0,    // kg
            drag_area: 10.0, // m^2
            cd: 1.2,
        }
    }
}

impl EdlVehicle {
    pub fn get_net_force(&self, altitude: f64, velocity: [f64; 3], physics: &MarsPhysics) -> [f64; 3] {
        let weight: f64 = self.mass * physics.gravity; // Downward (negative Z)
        let mut force = physics.calculate_drag(velocity, altitude, self.drag_area, self.cd);
        
        force[2] -= weight;
        force
    }
}
