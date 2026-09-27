// Swing Equation: AC Power Grid Cascade Physics
// THE EMBODIMENT: Generators are massive rotating beasts locked in a delicate dance
// at 60Hz. If one pushes too hard or drops too much load, the rotor accelerates.
// The restoring force is proportional to sin(delta). But if delta pushes past 90 degrees,
// the restoring force diminishes. The machine loses synchronism. It violently separates
// from the grid. And the cascaded grid blackout begins.

use crate::proof::ProofChain;

pub const NOMINAL_FREQUENCY: f64 = 60.0;
pub const OMEGA_S: f64 = 2.0 * std::f64::consts::PI * NOMINAL_FREQUENCY; // rad/s
pub const PI: f64 = std::f64::consts::PI;

// Inverter-based resource constants
pub const K_PLL: f64 = 20.0;             // PLL tracking bandwidth (s^-1)
pub const PLL_TRIP_LIMIT: f64 = 0.35;    // radians phase error (approx 20 degrees) before PLL slip-trip
pub const GOVERNOR_TC: f64 = 0.5;        // Seconds (Steam governor time constant)
pub const GOVERNOR_DROOP: f64 = 10.0;    // Governor speed regulation droop gain

#[derive(Debug, Clone)]
pub struct SynchronousMachine {
    pub h_constant: f64,       // Base Synchronous Machine Inertia constant (MW*s/MVA)
    pub damping: f64,          // Damping coefficient (pu)
    pub p_mech: f64,           // Nominal mechanical power input (pu)
    pub p_max: f64,            // Maximum transmissible electrical power (pu)
    
    pub delta: f64,            // Rotor angle (radians)
    pub delta_omega: f64,      // Angular velocity deviation (rad/s)
    
    // Upgraded hybrid grid variables
    pub inverter_fraction: f64, // Fraction of generation capacity from inverters (0 to 1)
    pub inverter_tripped: bool,  // Has the inverter PLL tripped off the grid?
    pub pll_err: f64,           // PLL phase tracking error (radians)
    pub governor_valve: f64,    // Turbine steam valve opening position (fraction)
    pub residual: f64,          // Lagrangian inconsistency residual I_grid(t)
    
    pub time_ms: u64,          // Elapsed time in milliseconds
    pub cascaded: bool,        // Flag for synchronism loss (delta > 90 deg)
    pub proof: ProofChain,
}

impl Default for SynchronousMachine {
    fn default() -> Self {
        let p_mech: f64 = 1.0;
        let p_max: f64 = 1.04; // Barely adequate transmission margin
        let initial_delta = (p_mech / p_max).asin(); 

        Self {
            h_constant: 5.0,
            damping: 0.05,
            p_mech,
            p_max,
            delta: initial_delta,
            delta_omega: 0.0,
            inverter_fraction: 0.30, // 30% IBR capacity initially
            inverter_tripped: false,
            pll_err: 0.0,
            governor_valve: 1.0,
            residual: 0.0,
            time_ms: 0,
            cascaded: false,
            proof: ProofChain::new(),
        }
    }
}

impl SynchronousMachine {
    pub fn new() -> Self {
        let mut sm = Self::default();
        sm.proof.feed_str("SWING_EQUATION_INIT_V2");
        sm.proof.feed_f64(sm.p_mech);
        sm.proof.feed_f64(sm.delta);
        sm.proof.feed_f64(sm.inverter_fraction);
        sm
    }

    /// Agentic AI hallucinates a load mismatch (e.g., changes routing abruptly).
    pub fn ai_apply_load_mismatch(&mut self, mismatch_percentage: f64) {
        let reduction = 1.0 - (mismatch_percentage / 100.0);
        self.p_max *= reduction;
        
        self.proof.feed_str("AI_HALLUCINATION_LOAD_MISMATCH");
        self.proof.feed_f64(self.p_max);
    }
    
    /// Atmospheric drop out (cloud cover over solar, wind shear dropping turbine speed)
    pub fn simulate_weather_loss(&mut self, loss_percentage: f64) {
        let reduction = 1.0 - (loss_percentage / 100.0);
        self.p_mech *= reduction;
        
        // Weather directly hits inverter solar/wind portion
        if self.inverter_fraction > 0.0 && !self.inverter_tripped {
            // Drop power capacity proportional to inverter share
            self.p_max *= 1.0 - (self.inverter_fraction * loss_percentage / 100.0);
        }

        self.proof.feed_str("ATMOSPHERIC_INTERMITTENCY_LOSS");
        self.proof.feed_f64(self.p_mech);
        self.proof.feed_f64(self.p_max);
    }

    /// Electrical power currently delivered to the grid
    pub fn p_elec(&self) -> f64 {
        self.p_max * self.delta.sin()
    }

    /// Effective grid inertia (reduced by inverter fraction since inverters have H=0)
    pub fn effective_inertia(&self) -> f64 {
        if self.inverter_tripped {
            self.h_constant // Inverter tripped, only synchronous machines left
        } else {
            self.h_constant * (1.0 - self.inverter_fraction)
        }
    }

    /// Step the simulation by dt (seconds)
    pub fn step(&mut self, dt: f64) {
        if self.cascaded {
            return;
        }

        // ═══════════════════════════════════════════════════════════════════════════
        // 1. INVERTER PLL DYNAMICS & TRIP DETECTION
        // ═══════════════════════════════════════════════════════════════════════════
        // d(pll_err)/dt = delta_omega - K_pll * pll_err
        if !self.inverter_tripped && self.inverter_fraction > 0.0 {
            let d_pll_err = self.delta_omega - K_PLL * self.pll_err;
            self.pll_err += d_pll_err * dt;

            // Check if PLL slips cycles and trips
            if self.pll_err.abs() >= PLL_TRIP_LIMIT {
                self.inverter_tripped = true;
                // Instant loss of inverter capacity creates a massive load shock
                self.p_max *= 1.0 - self.inverter_fraction;
                
                self.proof.feed_str("INVERTER_PLL_DESYNC_TRIP");
                self.proof.feed_f64(self.time_ms as f64);
                self.proof.feed_f64(self.pll_err);
                self.proof.feed_f64(self.p_max);
            }
        }

        // ═══════════════════════════════════════════════════════════════════════════
        // 2. GOVERNOR VALVE DYNAMICS
        // ═══════════════════════════════════════════════════════════════════════════
        // target valve adjusts to frequency deviation (droop response)
        let f_deviation = self.delta_omega / (2.0 * PI);
        let target_valve = (1.0 - GOVERNOR_DROOP * (f_deviation / NOMINAL_FREQUENCY)).clamp(0.5, 1.5);
        let dg = (target_valve - self.governor_valve) / GOVERNOR_TC;
        self.governor_valve += dg * dt;

        // Apply governor output to mechanical power
        let current_p_mech = self.p_mech * self.governor_valve;

        // ═══════════════════════════════════════════════════════════════════════════
        // 3. ROTOR DYNAMICS (Swing Equation)
        // ═══════════════════════════════════════════════════════════════════════════
        let p_e = self.p_elec();
        let h_eff = self.effective_inertia().max(0.1);
        let acceleration = (PI * NOMINAL_FREQUENCY / h_eff) 
                           * (current_p_mech - p_e - self.damping * self.delta_omega);

        self.delta_omega += acceleration * dt;
        self.delta += self.delta_omega * dt;

        self.time_ms += (dt * 1000.0).round() as u64;

        // ═══════════════════════════════════════════════════════════════════════════
        // 4. LAGRANGIAN RESIDUAL - Physical Inconsistency calculation
        // ═══════════════════════════════════════════════════════════════════════════
        // Unbalanced mechanical power after integration: P_m·gov - P_e(δ_new) - D·Δω.
        // Zero in equilibrium; grows as the rotor angle diverges toward the survival cliff.
        // Uses updated delta to compute P_e, so the residual reflects post-step imbalance.
        let p_e_final = self.p_elec();
        let imbalance = current_p_mech * self.governor_valve - p_e_final - self.damping * self.delta_omega;
        self.residual = imbalance.abs();

        // Check for loss of synchronism: rotor angle exceeds 90 degrees (pi/2 radians)
        if self.delta >= core::f64::consts::FRAC_PI_2 {
            self.cascaded = true;
            self.proof.feed_str("FATAL_LOSS_OF_SYNCHRONISM");
            self.proof.feed_f64(self.time_ms as f64);
            self.proof.feed_f64(self.delta);
            return;
        }

        // Periodically feed continuous state to the hash
        if self.time_ms % 100 == 0 {
            self.proof.feed_f64(self.time_ms as f64);
            self.proof.feed_f64(self.delta);
            self.proof.feed_f64(self.pll_err);
            self.proof.feed_f64(self.governor_valve);
        }
    }

    /// Seal the timeline into a single irreversible SHA-256 hash.
    pub fn get_sealed_hash(self) -> String {
        self.proof.seal()
    }
}
