// Orbital (Vacuum): N-Body Relativistic Engine Foundation + Attitude Dynamics
// Integrates 4th-order Yoshida Symplectic Integration with J2-J4 harmonics and 1PN corrections.
// Attitude: Euler's rotational equations, reaction wheels, thrusters, rate gyros, eclipse thermal.

/// Orbital period for circular orbit at given altitude (km) above Earth
pub fn orbital_period_s(altitude_km: f64) -> f64 {
    let r = 6378.137 + altitude_km;
    2.0 * std::f64::consts::PI * (r.powi(3) / 398600.4418).sqrt()
}

/// Eclipse fraction for circular orbit (simplified cylindrical shadow)
pub fn eclipse_fraction(altitude_km: f64) -> f64 {
    let r = 6378.137 + altitude_km;
    let beta = (6378.137 / r).asin();
    beta / std::f64::consts::PI
}

#[derive(Debug, Clone)]
pub struct OrbitalPhysics {
    pub mu: f64,       // Standard gravitational parameter (km^3/s^2)
    pub j2: f64,       // J2 perturbation coefficient
    pub j3: f64,       // J3 perturbation coefficient
    pub j4: f64,       // J4 perturbation coefficient
    pub r_equatorial: f64, // Equatorial radius of central body (km)
    pub c: f64,        // Speed of light (km/s)
}

impl Default for OrbitalPhysics {
    fn default() -> Self {
        Self {
            mu: 398600.4418, // Earth
            j2: 1.08262668e-3,
            j3: -2.532656e-6,
            j4: -1.6196215e-6,
            r_equatorial: 6378.137,
            c: 299792.458,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SatelliteState {
    pub position: [f64; 3], // ECI coordinates (km)
    pub velocity: [f64; 3], // ECI velocity (km/s)
    pub quaternion_attitude: [f64; 4], // [w, x, y, z]
    pub angular_velocity: [f64; 3],    // body frame (rad/s)
    pub inertia_tensor: [[f64; 3]; 3], // kg*m^2
}

// ─── REACTION WHEEL MODEL ───────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ReactionWheel {
    pub momentum: f64,       // current stored angular momentum (N*m*s)
    pub max_momentum: f64,   // saturation limit (N*m*s)
    pub max_torque: f64,     // max torque output (N*m)
    pub failed: bool,
}

impl ReactionWheel {
    pub fn new(max_torque: f64, max_momentum: f64) -> Self {
        Self { momentum: 0.0, max_momentum, max_torque, failed: false }
    }

    /// Apply a commanded torque, returns actual torque delivered
    pub fn command(&mut self, torque_cmd: f64, dt: f64) -> f64 {
        if self.failed { return 0.0; }
        let torque = torque_cmd.clamp(-self.max_torque, self.max_torque);
        let new_momentum = self.momentum + torque * dt;
        if new_momentum.abs() > self.max_momentum {
            // Saturated — can only deliver partial torque
            let available = (self.max_momentum * new_momentum.signum() - self.momentum) / dt;
            self.momentum = self.max_momentum * new_momentum.signum();
            available
        } else {
            self.momentum = new_momentum;
            torque
        }
    }

    pub fn is_saturated(&self) -> bool {
        self.momentum.abs() > self.max_momentum * 0.95
    }
}

// ─── THRUSTER MODEL ─────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ThrusterSet {
    pub max_torque: f64,      // max torque per axis (N*m)
    pub isp: f64,             // specific impulse (s)
    pub fuel_kg: f64,         // remaining fuel mass (kg)
    pub failed: bool,
}

impl ThrusterSet {
    pub fn new(max_torque: f64, isp: f64, fuel_kg: f64) -> Self {
        Self { max_torque, isp, fuel_kg, failed: false }
    }

    /// Fire thrusters for desaturation or emergency detumble.
    /// Returns actual torque delivered per axis, consumes fuel.
    pub fn fire(&mut self, torque_cmd: [f64; 3], dt: f64) -> [f64; 3] {
        if self.failed || self.fuel_kg <= 0.0 { return [0.0; 3]; }
        let mut actual = [0.0_f64; 3];
        let g0 = 9.81;
        for i in 0..3 {
            let t = torque_cmd[i].clamp(-self.max_torque, self.max_torque);
            // Fuel consumption: F = torque / moment_arm (assume 1m), mdot = F / (Isp * g0)
            let force = t.abs();
            let mdot = force / (self.isp * g0);
            let fuel_needed = mdot * dt;
            if fuel_needed > self.fuel_kg {
                actual[i] = 0.0;
            } else {
                self.fuel_kg -= fuel_needed;
                actual[i] = t;
            }
        }
        actual
    }
}

// ─── RATE GYRO MODEL ────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RateGyro {
    pub bias: [f64; 3],        // rad/s bias per axis
    pub noise_sigma: f64,      // measurement noise std dev (rad/s)
    pub bias_drift_rate: f64,  // bias random walk (rad/s / sqrt(s))
    pub failed: bool,
}

impl RateGyro {
    pub fn new(noise_sigma: f64, bias_drift_rate: f64) -> Self {
        Self { bias: [0.0; 3], noise_sigma, bias_drift_rate, failed: false }
    }

    /// Read angular velocity with noise and bias. Dark window: this is ALL you have.
    pub fn read(&mut self, true_omega: &[f64; 3], rng: &mut crate::rng::Rng, dt: f64) -> [f64; 3] {
        if self.failed { return [0.0; 3]; }
        let mut measured = [0.0_f64; 3];
        for i in 0..3 {
            // Bias random walk
            self.bias[i] += rng.gaussian(0.0, self.bias_drift_rate * dt.sqrt());
            // Measurement = truth + bias + noise
            measured[i] = true_omega[i] + self.bias[i] + rng.gaussian(0.0, self.noise_sigma);
        }
        measured
    }
}

// ─── ECLIPSE THERMAL / POWER MODEL ─────────────────────────────

#[derive(Debug, Clone)]
pub struct PowerSystem {
    pub battery_wh: f64,       // current charge (Wh)
    pub battery_max_wh: f64,   // max capacity (Wh)
    pub solar_power_w: f64,    // solar panel output in sunlight (W)
    pub heater_draw_w: f64,    // heater power in eclipse (W)
    pub avionics_draw_w: f64,  // base avionics power (W)
    pub wheel_draw_w: f64,     // reaction wheel power (W)
}

impl PowerSystem {
    pub fn new(battery_wh: f64, solar_w: f64) -> Self {
        Self {
            battery_wh,
            battery_max_wh: battery_wh,
            solar_power_w: solar_w,
            heater_draw_w: 15.0,
            avionics_draw_w: 30.0,
            wheel_draw_w: 10.0,
        }
    }

    /// Step the power system. in_sunlight=false means eclipse (dark window).
    pub fn step(&mut self, dt: f64, in_sunlight: bool, wheels_active: bool) -> bool {
        let draw = self.avionics_draw_w
            + if !in_sunlight { self.heater_draw_w } else { 0.0 }
            + if wheels_active { self.wheel_draw_w } else { 0.0 };
        let generation = if in_sunlight { self.solar_power_w } else { 0.0 };
        let net_w = generation - draw;
        self.battery_wh += net_w * dt / 3600.0;
        self.battery_wh = self.battery_wh.clamp(0.0, self.battery_max_wh);
        self.battery_wh > 0.0
    }

    pub fn charge_fraction(&self) -> f64 {
        self.battery_wh / self.battery_max_wh
    }
}

// ─── QUATERNION MATH ────────────────────────────────────────────

/// Normalize a quaternion [w, x, y, z]
pub fn quat_normalize(q: &mut [f64; 4]) {
    let n = (q[0]*q[0] + q[1]*q[1] + q[2]*q[2] + q[3]*q[3]).sqrt();
    if n > 1e-12 {
        q[0] /= n; q[1] /= n; q[2] /= n; q[3] /= n;
    }
}

/// Propagate quaternion by angular velocity omega (body frame, rad/s) over dt
pub fn quat_propagate(q: &mut [f64; 4], omega: &[f64; 3], dt: f64) {
    let ox = omega[0] * 0.5;
    let oy = omega[1] * 0.5;
    let oz = omega[2] * 0.5;
    let w = q[0]; let x = q[1]; let y = q[2]; let z = q[3];
    q[0] += (-x*ox - y*oy - z*oz) * dt;
    q[1] += ( w*ox + y*oz - z*oy) * dt;
    q[2] += ( w*oy + z*ox - x*oz) * dt;
    q[3] += ( w*oz + x*oy - y*ox) * dt;
    quat_normalize(q);
}

/// Angular magnitude (total body rate, rad/s)
pub fn omega_magnitude(omega: &[f64; 3]) -> f64 {
    (omega[0]*omega[0] + omega[1]*omega[1] + omega[2]*omega[2]).sqrt()
}

impl OrbitalPhysics {
    pub fn compute_acceleration(&self, pos: &[f64; 3], vel: &[f64; 3]) -> [f64; 3] {
        let x = pos[0];
        let y = pos[1];
        let z = pos[2];
        let r2 = x*x + y*y + z*z;
        let r = r2.sqrt();
        let r3 = r2 * r;

        // 1. Central Gravity
        let central = -self.mu / r3;
        let mut ax = x * central;
        let mut ay = y * central;
        let mut az = z * central;

        // 2. Zonal Harmonics (J2, J3, J4)
        let z2_r2 = (z / r).powi(2);
        let z_r = z / r;
        let r_eq_r = self.r_equatorial / r;
        
        // J2
        let j2_term = 1.5 * self.j2 * r_eq_r.powi(2) * (self.mu / r2);
        ax += (x / r) * j2_term * (5.0 * z2_r2 - 1.0);
        ay += (y / r) * j2_term * (5.0 * z2_r2 - 1.0);
        az += (z / r) * j2_term * (5.0 * z2_r2 - 3.0);

        // J3 (simplified approximation)
        let j3_term = 0.5 * self.j3 * r_eq_r.powi(3) * (self.mu / r2);
        ax += (x / r) * j3_term * 5.0 * z_r * (7.0 * z2_r2 - 3.0);
        ay += (y / r) * j3_term * 5.0 * z_r * (7.0 * z2_r2 - 3.0);
        az += j3_term * (35.0 * z_r.powi(3) - 30.0 * z_r + 3.0);

        // J4 (simplified approximation)
        let j4_term = 0.625 * self.j4 * r_eq_r.powi(4) * (self.mu / r2);
        ax += (x / r) * j4_term * (35.0 * z2_r2.powi(2) - 30.0 * z2_r2 + 3.0);
        ay += (y / r) * j4_term * (35.0 * z2_r2.powi(2) - 30.0 * z2_r2 + 3.0);
        az += (z / r) * j4_term * (35.0 * z_r.powi(4) - 30.0 * z2_r2 + 3.0);

        // 3. Post-Newtonian (1PN) Relativistic Correction
        let v2 = vel[0]*vel[0] + vel[1]*vel[1] + vel[2]*vel[2];
        let r_dot_v = x*vel[0] + y*vel[1] + z*vel[2];
        let c2 = self.c * self.c;
        let pn_coeff = self.mu / (c2 * r3);
        
        let term1 = 4.0 * self.mu / r - v2;
        let term2 = 4.0 * r_dot_v;

        ax += pn_coeff * (term1 * x + term2 * vel[0]);
        ay += pn_coeff * (term1 * y + term2 * vel[1]);
        az += pn_coeff * (term1 * z + term2 * vel[2]);

        [ax, ay, az]
    }

    pub fn step_6dof(&self, state: &mut SatelliteState, dt: f64) {
        // 4th-Order Yoshida Symplectic Integrator (Translational)
        let w0 = -(2.0f64.powf(1.0 / 3.0)) / (2.0 - 2.0f64.powf(1.0 / 3.0));
        let w1 = 1.0 / (2.0 - 2.0f64.powf(1.0 / 3.0));

        let c1 = w1 / 2.0;
        let c2 = (w0 + w1) / 2.0;
        let c3 = c2;
        let c4 = c1;

        let d1 = w1;
        let d2 = w0;
        let d3 = w1;

        let c_coeffs = [c1, c2, c3, c4];
        let d_coeffs = [d1, d2, d3];

        for i in 0..3 {
            for j in 0..3 {
                state.position[j] += c_coeffs[i] * state.velocity[j] * dt;
            }
            let accel = self.compute_acceleration(&state.position, &state.velocity);
            for j in 0..3 {
                state.velocity[j] += d_coeffs[i] * accel[j] * dt;
            }
        }
        for j in 0..3 {
            state.position[j] += c_coeffs[3] * state.velocity[j] * dt;
        }
    }

    /// Propagate attitude using Euler's rotational equations.
    /// I * omega_dot = tau_ext - omega x (I * omega)
    /// external_torque is the net torque from wheels + thrusters in body frame.
    pub fn step_attitude(state: &mut SatelliteState, external_torque: &[f64; 3], dt: f64) {
        let i = &state.inertia_tensor;
        let w = &state.angular_velocity;

        // I * omega (diagonal inertia tensor assumption for speed)
        let iw = [i[0][0] * w[0], i[1][1] * w[1], i[2][2] * w[2]];

        // omega x (I * omega) — gyroscopic coupling
        let cross = [
            w[1] * iw[2] - w[2] * iw[1],
            w[2] * iw[0] - w[0] * iw[2],
            w[0] * iw[1] - w[1] * iw[0],
        ];

        // omega_dot = I^-1 * (tau - omega x (I * omega))
        state.angular_velocity[0] += (external_torque[0] - cross[0]) / i[0][0] * dt;
        state.angular_velocity[1] += (external_torque[1] - cross[1]) / i[1][1] * dt;
        state.angular_velocity[2] += (external_torque[2] - cross[2]) / i[2][2] * dt;

        // Propagate quaternion
        quat_propagate(&mut state.quaternion_attitude, &state.angular_velocity, dt);
    }
}

