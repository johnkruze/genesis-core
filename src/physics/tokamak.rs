// Tokamak (Magnetic Confinement): MHD Quench & Probabilistic Plasma Loss
// THE EMBODIMENT: 150 million degrees. A star in a bottle.
// The bottle is made of magnetic fields, which are smooth, continuous, and deterministic.
// The controller is an AI agent running down-sampled, discretized, probabilistic logits.
// When the AI "guesses" the poloidal coil voltages, it injects micro-fluctuations into
// the B-field. The plasma expands. If it touches the tungsten first wall: Quench.
// The entire thermal mass flashes the wall to vapor. Billions destroyed by a floating point error.

use crate::proof::ProofChain;

pub const MU_0: f64 = 1.25663706e-6;     // Vacuum permeability (H/m)
pub const K_B: f64 = 1.380649e-23;       // Boltzmann constant (J/K)
pub const CONTAINMENT_RADIUS: f64 = 2.0; // Meters to the tungsten first wall
pub const PLASMA_MASS_DENSITY: f64 = 2e-8; // kg/m^3 (approximate macroscopic mass)

pub const DIVERTOR_Z_LIMIT: f64 = 0.5;     // Meters (vertical distance to the floor/divertor)

// Superconducting PF Coil constants
pub const L_COIL: f64 = 0.01;            // Henry (Inductance)
pub const R_0: f64 = 1e-3;                // Ohm (superconducting nominal resistance)
pub const ALPHA_R: f64 = 0.1;            // K^-1 (Resistance temperature coefficient)
pub const T_CRYO: f64 = 4.2;             // Kelvin (Helium cryostat reference temp)
pub const T_QUENCH: f64 = 15.0;          // Kelvin (superconductor critical quench temperature)
pub const C_COIL: f64 = 0.5;             // J/K (Coil heat capacity)
pub const H_COOL: f64 = 0.1;             // W/K (Cryogenic coolant heat transfer)

// Vertical stability coupling constants (normalized to plasma mass)
pub const GAMMA_Z_SQ: f64 = 1000.0;      // s^-2 (Unstable vertical growth rate square)
pub const K_STABILIZE: f64 = -30.0;      // m/(A*s^2) (Stabilizing acceleration per Ampere of PF coil current)
pub const K_DISTURB: f64 = 3000.0;       // m/(T*s^2) (Disturbing acceleration per Tesla of field error)

#[derive(Debug, Clone)]
pub struct Tokamak {
    pub plasma_radius: f64,     // m
    pub radial_velocity: f64,   // m/s
    pub z_displacement: f64,    // m (vertical offset from center)
    pub z_velocity: f64,        // m/s (vertical drift speed)
    pub temperature: f64,       // Kelvin
    pub particle_density: f64,  // particles/m^3
    
    pub b_field: f64,           // Magnetic confinement field (Tesla)
    pub b_field_asymmetry: f64, // Unintended vertical shear field (Tesla)
    
    // Upgraded actuator & thermal variables
    pub pf_voltage: f64,        // Control input: PF coil voltage (V)
    pub pf_current: f64,        // PF coil current (A)
    pub coil_temp: f64,         // PF coil temperature (K)
    pub coil_quenched: bool,    // Has the coil lost superconductivity?
    pub residual: f64,          // Lagrangian inconsistency residual I_tokamak(t)
    
    pub time_us: u64,           // Microseconds
    pub quenched: bool,         // Wall breach flag
    pub proof: ProofChain,
}

impl Default for Tokamak {
    fn default() -> Self {
        Self {
            plasma_radius: 1.8,       // Starts 20cm from the wall
            radial_velocity: 0.0,
            z_displacement: 0.0,      // Perfectly centered on Day 1
            z_velocity: 0.0,
            temperature: 1.5e8,       // 150 Million K
            particle_density: 1.0e20, // 10^20 m^-3
            b_field: 0.721,           // Nominal balancing Tesla
            b_field_asymmetry: 0.0,
            pf_voltage: 0.0,
            pf_current: 0.0,
            coil_temp: 4.2,           // In cryo bath
            coil_quenched: false,
            residual: 0.0,
            time_us: 0,
            quenched: false,
            proof: ProofChain::new(),
        }
    }
}

impl Tokamak {
    pub fn new() -> Self {
        let mut sim = Self::default();
        sim.b_field = sim.exact_equilibrium_b_field(); // Lock exact equilibrium
        sim.proof.feed_str("TOKAMAK_IGNITED_V3");
        sim.proof.feed_f64(sim.plasma_radius);
        sim.proof.feed_f64(sim.b_field);
        sim.proof.feed_f64(sim.coil_temp);
        sim
    }

    /// Plasma Pressure = n * k_B * T
    pub fn plasma_pressure(&self) -> f64 {
        let volume_ratio = (1.8 / self.plasma_radius).powi(2); // density drops as it expands
        self.particle_density * volume_ratio * K_B * self.temperature
    }

    /// Magnetic Pressure = B^2 / (2 * mu_0)
    pub fn magnetic_pressure(&self) -> f64 {
        (self.b_field * self.b_field) / (2.0 * MU_0)
    }

    /// Calculate the exact B-field required to hold the plasma static.
    /// P_mag = P_plasma => B = sqrt(2 * mu_0 * P_plasma)
    pub fn exact_equilibrium_b_field(&self) -> f64 {
        (2.0 * MU_0 * self.plasma_pressure()).sqrt()
    }

    /// The AI attempts to control the magnetic field and vertical stabilization voltage
    pub fn apply_agentic_ai_field(&mut self, target_b: f64, radial_noise: f64, z_asymmetry_noise: f64) {
        // AI injects the noise into the continuous control signal
        self.b_field = target_b + radial_noise;
        self.b_field_asymmetry = z_asymmetry_noise; // Poloidal shear field
        
        // Active PD vertical control command (stabilizing vertical displacement z)
        // AI's vertical asymmetry noise acts as an external disturbance on the voltage setpoint
        // Stabilizer voltage loop: V = -Kp * z - Kd * v + disturbance
        self.pf_voltage = -80.0 * self.z_displacement - 12.0 * self.z_velocity + z_asymmetry_noise * 50.0;
        
        self.proof.feed_str("AI_COIL_UPDATE_V3");
        self.proof.feed_f64(self.b_field);
        self.proof.feed_f64(self.pf_voltage);
    }

    /// Step the continuous physics engine (1 microsecond timestep)
    pub fn step(&mut self, dt_us: f64) {
        if self.quenched { return; }

        let dt = dt_us * 1e-6; // convert to seconds
        
        let p_plasma = self.plasma_pressure();
        let p_mag = self.magnetic_pressure();

        // ═══════════════════════════════════════════════════════════════════════════
        // 1. RADIAL MHD FORCE BALANCE
        // ═══════════════════════════════════════════════════════════════════════════
        let radial_acceleration = (p_plasma - p_mag) / PLASMA_MASS_DENSITY;
        self.radial_velocity += radial_acceleration * dt;
        self.plasma_radius += self.radial_velocity * dt;

        // ═══════════════════════════════════════════════════════════════════════════
        // 2. PF COIL ACTUATOR & THERMAL DYNAMICS
        // ═══════════════════════════════════════════════════════════════════════════
        // Determine coil resistance (superconducting vs quenched)
        let resistance = if self.coil_quenched {
            1.0 // Normal state resistive loss
        } else {
            R_0 * (1.0 + ALPHA_R * (self.coil_temp - T_CRYO))
        };

        // dI/dt = (V - I * R) / L
        let di_dt = (self.pf_voltage - self.pf_current * resistance) / L_COIL;
        self.pf_current += di_dt * dt;

        // Coil heating: dT_coil/dt = (I^2 * R) / C_coil - H_cool * (T_coil - T_cryo)
        let power_dissipated = self.pf_current * self.pf_current * resistance;
        let dt_coil = (power_dissipated / C_COIL - H_COOL * (self.coil_temp - T_CRYO)) * dt;
        self.coil_temp = (self.coil_temp + dt_coil).max(T_CRYO);

        // Quench threshold check
        if self.coil_temp >= T_QUENCH && !self.coil_quenched {
            self.coil_quenched = true;
            self.proof.feed_str("COIL_THERMAL_QUENCH");
            self.proof.feed_f64(self.coil_temp);
        }

        // ═══════════════════════════════════════════════════════════════════════════
        // 3. VERTICAL DISPLACEMENT (MHD elongation instability stabilized by current)
        // ═══════════════════════════════════════════════════════════════════════════
        // Elongated instability: growth acceleration proportional to displacement
        let vertical_growth_accel = GAMMA_Z_SQ * self.z_displacement;
        
        // Stabilizing force from control coils: F_pf = K_stabilize * pf_current
        // External poloidal asymmetry field acts as a disturbing vertical force
        let stabilizing_accel = K_STABILIZE * self.pf_current;
        let disturbing_accel = K_DISTURB * self.b_field_asymmetry;
        
        let z_accel = vertical_growth_accel + stabilizing_accel + disturbing_accel;
        self.z_velocity += z_accel * dt;
        self.z_displacement += self.z_velocity * dt;

        self.time_us += dt_us.round() as u64;

        // ═══════════════════════════════════════════════════════════════════════════
        // 4. LAGRANGIAN RESIDUAL - Physical Inconsistency calculation
        // ═══════════════════════════════════════════════════════════════════════════
        // Evaluates the radial MHD force balance residual using the FINAL plasma_radius
        // (after the integration step), compared against the acceleration computed from
        // the OLD radius at the start of this step. Non-trivially non-zero because
        // plasma_radius changed during integration; grows as the plasma expands toward the wall.
        let p_plasma_final = self.plasma_pressure();
        let p_mag_final = self.magnetic_pressure();
        let radial_accel_final = (p_plasma_final - p_mag_final) / PLASMA_MASS_DENSITY;
        self.residual = (radial_accel_final - radial_acceleration).abs();

        // BOUNDARY CONDITION: Does the plasma touch the radial wall OR the vertical divertor floor?
        if self.plasma_radius >= CONTAINMENT_RADIUS || self.z_displacement.abs() >= DIVERTOR_Z_LIMIT {
            self.quenched = true;
            self.proof.feed_str("FATAL_MHD_QUENCH");
            self.proof.feed_f64(self.time_us as f64);
            self.proof.feed_f64(self.plasma_radius);
            self.proof.feed_f64(self.z_displacement);
            self.proof.feed_f64(self.temperature);
            self.proof.feed_f64(self.pf_current);
            self.proof.feed_f64(self.coil_temp);
            return;
        }

        // Periodically feed continuous state to the hash
        if self.time_us % 100 == 0 {
            self.proof.feed_f64(self.time_us as f64);
            self.proof.feed_f64(self.plasma_radius);
            self.proof.feed_f64(self.z_displacement);
            self.proof.feed_f64(self.pf_current);
            self.proof.feed_f64(self.coil_temp);
        }
    }

    /// Seal the destruction
    pub fn get_sealed_hash(self) -> String {
        self.proof.seal()
    }
}
