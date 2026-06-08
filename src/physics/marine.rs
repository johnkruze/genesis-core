//! Marine substrate physics — the body feels pressure.
//!
//! Seawater dynamics for autonomous underwater vehicles.
//! Buoyancy, drag, currents, hydrostatic pressure, GPS-denied navigation.
//! The Fukuoka question: can you navigate by feeling alone?

use crate::rng::Rng;

// ─── CONSTANTS ───

pub const RHO_SEAWATER: f64 = 1025.0;    // kg/m^3
pub const GRAVITY: f64 = 9.81;            // m/s^2
pub const ATM_PRESSURE: f64 = 101325.0;   // Pa
pub const SOUND_SPEED: f64 = 1500.0;      // m/s in seawater (approx)

// ─── PHYSICS ENGINE ───

#[derive(Debug, Clone, Copy)]
pub struct MarinePhysics {
    pub gravity: f64,
    pub rho_water: f64,
}

impl Default for MarinePhysics {
    fn default() -> Self {
        Self {
            gravity: GRAVITY,
            rho_water: RHO_SEAWATER,
        }
    }
}

impl MarinePhysics {
    /// Archimedes: F_buoyancy = rho * g * V (upward)
    pub fn buoyancy(&self, volume: f64) -> f64 {
        self.rho_water * self.gravity * volume
    }

    /// Hydrodynamic drag: F = 0.5 * rho * |v|^2 * Cd * A, opposing velocity
    pub fn drag(&self, velocity: [f64; 3], area: f64, cd: f64) -> [f64; 3] {
        let speed = vec3_mag(velocity);
        if speed < 1e-8 { return [0.0; 3]; }
        let mag = 0.5 * self.rho_water * speed * speed * cd * area;
        [
            -mag * velocity[0] / speed,
            -mag * velocity[1] / speed,
            -mag * velocity[2] / speed,
        ]
    }

    /// P = P_atm + rho * g * depth
    pub fn hydrostatic_pressure(&self, depth: f64) -> f64 {
        ATM_PRESSURE + self.rho_water * self.gravity * depth
    }

    /// Depth from pressure (inverse of hydrostatic_pressure)
    /// This is the pressure gradient navigation primitive.
    /// Accuracy: ~0.01m at depth (pressure sensors are precise)
    pub fn depth_from_pressure(&self, pressure: f64) -> f64 {
        (pressure - ATM_PRESSURE) / (self.rho_water * self.gravity)
    }
}

// ─── CURRENT FIELD ───

/// 3D ocean current model with depth shear, tidal oscillation, and turbulence.
#[derive(Debug, Clone)]
pub struct CurrentField {
    /// Base current velocity [m/s] at surface
    pub base_speed: f64,
    /// Base current direction [radians]
    pub base_heading: f64,
    /// Depth-dependent shear rate [1/m] — current weakens with depth
    pub shear_rate: f64,
    /// Tidal oscillation amplitude [m/s]
    pub tidal_amplitude: f64,
    /// Tidal period [s] (compressed from 12.4h for sim timescales)
    pub tidal_period: f64,
    /// Turbulent noise standard deviation [m/s]
    pub turbulence_std: f64,
}

impl CurrentField {
    /// Sample the current at a given depth and time
    pub fn sample(&self, depth: f64, time: f64, rng: &mut Rng) -> [f64; 3] {
        // Depth shear: current weakens exponentially with depth
        let depth_factor = (-self.shear_rate * depth).exp();

        // Tidal oscillation
        let tidal_phase = 2.0 * std::f64::consts::PI * time / self.tidal_period;
        let tidal_factor = 1.0 + self.tidal_amplitude * tidal_phase.sin();

        let speed = self.base_speed * depth_factor * tidal_factor;

        // Direction drifts slightly with tidal phase
        let heading = self.base_heading + 0.3 * tidal_phase.cos();

        // Horizontal current
        let cx = speed * heading.cos();
        let cy = speed * heading.sin();

        // Small vertical component from upwelling/downwelling
        let cz = speed * 0.05 * (tidal_phase * 2.0).sin();

        // Turbulent perturbation (Gaussian noise)
        let nx = rng.gaussian(0.0, self.turbulence_std);
        let ny = rng.gaussian(0.0, self.turbulence_std);
        let nz = rng.gaussian(0.0, self.turbulence_std * 0.3); // less vertical turbulence

        [cx + nx, cy + ny, cz + nz]
    }

    /// Force on AUV from current: drag equation using RELATIVE velocity
    pub fn current_force(
        &self,
        auv_vel: [f64; 3],
        current: [f64; 3],
        drag_area: f64,
        cd: f64,
    ) -> [f64; 3] {
        // Relative velocity = AUV velocity minus current velocity
        let rel = [
            auv_vel[0] - current[0],
            auv_vel[1] - current[1],
            auv_vel[2] - current[2],
        ];
        let speed = vec3_mag(rel);
        if speed < 1e-8 { return [0.0; 3]; }
        let mag = 0.5 * RHO_SEAWATER * speed * speed * cd * drag_area;
        [
            -mag * rel[0] / speed,
            -mag * rel[1] / speed,
            -mag * rel[2] / speed,
        ]
    }
}

// ─── AUV MODEL ───

#[derive(Debug, Clone)]
pub struct AuvModel {
    pub mass: f64,           // kg
    pub volume: f64,         // m^3 (displacement)
    pub drag_area: f64,      // m^2 (frontal cross-section)
    pub cd: f64,             // drag coefficient
    pub max_thrust: f64,     // N per thruster axis
    pub n_thrusters: u8,     // thrusters per axis (for failure mode)
    pub battery_wh: f64,     // Watt-hours total
    pub battery_remaining: f64, // Wh remaining
    pub reserve_fraction: f64,  // emergency reserve (fraction)
}

impl AuvModel {
    /// Power consumed by thrust [Watts].
    /// P proportional to thrust^2 (propeller physics: P = T^(3/2) / sqrt(2*rho*A))
    /// Simplified: P = k * |thrust|^1.5 where k scales to max power at max thrust
    pub fn thrust_power(&self, thrust_mag: f64) -> f64 {
        let max_power = 200.0; // Watts at max thrust
        max_power * (thrust_mag / self.max_thrust).powf(1.5)
    }

    /// Consume energy. Returns false if battery is dead.
    pub fn consume_energy(&mut self, power_watts: f64, dt: f64) -> bool {
        let wh = power_watts * dt / 3600.0;
        self.battery_remaining -= wh;
        self.battery_remaining > self.battery_wh * self.reserve_fraction
    }

    /// Battery fraction remaining
    pub fn battery_fraction(&self) -> f64 {
        self.battery_remaining / self.battery_wh
    }
}

// ─── GPS-DENIED NAVIGATION ───

/// Dead reckoning state with accumulating drift.
#[derive(Debug, Clone)]
pub struct DeadReckoning {
    /// Believed position (diverges from actual over time)
    pub believed_pos: [f64; 3],
    /// Drift rate [m/s] — IMU integration error accumulation
    pub drift_rate: f64,
    /// Accumulated drift [m]
    pub total_drift: f64,
    /// Pressure sensor noise std dev [Pa]
    pub pressure_noise_std: f64,
}

impl DeadReckoning {
    /// Update believed position from IMU (with drift)
    pub fn update_imu(&mut self, velocity: [f64; 3], dt: f64, rng: &mut Rng) {
        // Dead reckoning: integrate velocity with accumulating bias
        let drift_x = rng.gaussian(0.0, self.drift_rate * dt);
        let drift_y = rng.gaussian(0.0, self.drift_rate * dt);

        self.believed_pos[0] += velocity[0] * dt + drift_x;
        self.believed_pos[1] += velocity[1] * dt + drift_y;

        self.total_drift += (drift_x * drift_x + drift_y * drift_y).sqrt();
    }

    /// Correct vertical position using pressure sensor.
    /// Pressure is precise. This is the one axis GPS denial doesn't blind.
    pub fn correct_from_pressure(
        &mut self,
        actual_depth: f64,
        physics: &MarinePhysics,
        rng: &mut Rng,
    ) {
        // Simulate pressure measurement with noise
        let true_pressure = physics.hydrostatic_pressure(actual_depth);
        let measured_pressure = true_pressure + rng.gaussian(0.0, self.pressure_noise_std);
        let estimated_depth = physics.depth_from_pressure(measured_pressure);

        // Vertical correction (depth is negative z in our frame, but we track depth as positive)
        self.believed_pos[2] = -estimated_depth;
    }

    /// Position error magnitude (horizontal only — vertical is corrected by pressure)
    pub fn horizontal_error(&self, actual_pos: [f64; 3]) -> f64 {
        let dx = self.believed_pos[0] - actual_pos[0];
        let dy = self.believed_pos[1] - actual_pos[1];
        (dx * dx + dy * dy).sqrt()
    }
}

// ─── WAYPOINT NAVIGATION ───

#[derive(Debug, Clone)]
pub struct Waypoint {
    pub pos: [f64; 3],
    pub radius: f64, // acceptance radius [m]
}

/// Generate a set of random waypoints within a mission area
pub fn generate_waypoints(count: usize, depth_range: [f64; 2], spread: f64, rng: &mut Rng) -> Vec<Waypoint> {
    let mut waypoints = Vec::with_capacity(count);
    for _ in 0..count {
        waypoints.push(Waypoint {
            pos: [
                rng.range(-spread, spread),
                rng.range(-spread, spread),
                -rng.range(depth_range[0], depth_range[1]), // negative z = depth
            ],
            radius: rng.range(3.0, 10.0),
        });
    }
    waypoints
}

/// Navigation command: thrust vector toward next waypoint using BELIEVED position
pub fn navigate_to_waypoint(
    believed_pos: [f64; 3],
    waypoint: &Waypoint,
    max_thrust: f64,
) -> [f64; 3] {
    let dx = waypoint.pos[0] - believed_pos[0];
    let dy = waypoint.pos[1] - believed_pos[1];
    let dz = waypoint.pos[2] - believed_pos[2];
    let dist = (dx * dx + dy * dy + dz * dz).sqrt();
    if dist < 0.1 { return [0.0; 3]; }

    // Proportional control: more thrust when far, taper when close
    let gain = (dist / 20.0).min(1.0); // full thrust beyond 20m
    let thrust = max_thrust * gain;

    [
        thrust * dx / dist,
        thrust * dy / dist,
        thrust * dz / dist,
    ]
}

/// Check if actual position is within waypoint acceptance radius
pub fn waypoint_reached(actual_pos: [f64; 3], waypoint: &Waypoint) -> bool {
    let dx = actual_pos[0] - waypoint.pos[0];
    let dy = actual_pos[1] - waypoint.pos[1];
    let dz = actual_pos[2] - waypoint.pos[2];
    (dx * dx + dy * dy + dz * dz).sqrt() < waypoint.radius
}

// ─── HELPERS ───

fn vec3_mag(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}
