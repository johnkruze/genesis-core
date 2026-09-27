// Reactor (Nuclear Kinetics): Xenon-135 Poisoning & Prompt Criticality
// THE EMBODIMENT: The core is a breathing thermal beast. Iodine-135 decays into Xenon-135.
// Xenon-135 is the greatest neutron poison known to man (2.6 million barns).
// When flux drops, Xenon spikes (the "Iodine Pit"). If operators pull the control rods
// to fight the poison, and then the Xenon burns out... the core runs away.
// Reactivity ρ crosses β (0.0065). The reactor becomes prompt critical.

use crate::proof::ProofChain;

pub const LAMBDA_I: f64 = 2.87e-5; // s^-1, Iodine-135 decay constant (~6.57h)
pub const LAMBDA_XE: f64 = 2.09e-5; // s^-1, Xenon-135 decay constant (~9.2h)
pub const GAMMA_I: f64 = 0.0639; // I-135 fission yield
pub const GAMMA_XE: f64 = 0.00237; // Xe-135 direct fission yield
pub const SIGMA_A_XE: f64 = 2.6e-18; // cm^2, Microscopic absorption cross section of Xe-135
pub const BETA: f64 = 0.0065; // Delayed neutron fraction (approximate for U-235)
pub const NEUTRON_GENERATION_TIME: f64 = 1e-4; // Lambda, standard thermal reactor

// 6-Group Precursor Constants for U-235 thermal fission
pub const LAMBDAS: [f64; 6] = [0.0124, 0.0305, 0.111, 0.301, 1.14, 3.01]; // s^-1 decay constants
pub const BETAS: [f64; 6] = [0.000215, 0.001424, 0.001274, 0.002568, 0.000748, 0.000273]; // group fractions

// Thermal-hydraulic feedback coefficients
pub const ALPHA_D: f64 = -2.0e-5; // Doppler coefficient (reactivity/K, negative feedback)
pub const ALPHA_C: f64 = -1.0e-4; // Coolant temperature coefficient (reactivity/K)
pub const FUEL_TEMP_0: f64 = 900.0; // Nominal reference fuel temperature (K)
pub const COOLANT_TEMP_0: f64 = 570.0; // Nominal reference coolant temperature (K)

// Heat transfer parameters
pub const H_FC: f64 = 0.05; // Fuel-to-coolant heat transfer coefficient (s^-1)
pub const W_C: f64 = 0.02; // Coolant mass flow heat extraction coefficient (s^-1)
pub const COOLANT_INLET_TEMP: f64 = 500.0; // Coolant inlet temperature (K)
pub const FLUX_TO_HEAT: f64 = 5.0e-11; // Conversion factor from flux to equivalent fuel heat (K/s)

#[derive(Debug, Clone)]
pub struct ReactorState {
    pub flux: f64,             // Neutrons / (cm^2 * s)
    pub iodine_135: f64,       // Atoms / cm^3
    pub xenon_135: f64,        // Atoms / cm^3
    pub control_rods_rho: f64, // Reactivity contribution from control rods (negative terms)
    pub base_rho: f64,         // Base built-in excess reactivity
    pub sigma_f: f64,          // Macroscopic fission cross-section (cm^-1)
    
    // Upgraded high-dimensional variables
    pub precursors: [f64; 6],  // Precursor group concentrations (atoms / cm^3 equivalent)
    pub fuel_temp: f64,        // Fuel temperature (K)
    pub coolant_temp: f64,     // Coolant temperature (K)
    pub residual: f64,         // Lagrangian physical inconsistency residual I_reactor(t)
    
    pub time_s: f64,           // Simulation time
    pub days_burned: f64,      // Days of fuel degradation (tracking Pu-239 breeding)
    pub prompt_critical: bool,
    pub proof: ProofChain,
}

impl Default for ReactorState {
    fn default() -> Self {
        let mut state = Self {
            flux: 1e13, // Nominal operating flux
            iodine_135: 2.0e15, // Equilibrium nominal initial concentrations
            xenon_135: 3.0e14,
            control_rods_rho: -0.20, // Inserted rods balancing base_rho exactly at start
            base_rho: 0.20,
            sigma_f: 0.1,
            precursors: [0.0; 6],
            fuel_temp: 900.0,
            coolant_temp: 570.0,
            residual: 0.0,
            time_s: 0.0,
            days_burned: 0.0,
            prompt_critical: false,
            proof: ProofChain::new(),
        };

        // Initialize precursors to equilibrium values corresponding to nominal flux
        let beta_eff = state.effective_beta();
        for i in 0..6 {
            let beta_i_scaled = BETAS[i] * (beta_eff / BETA);
            state.precursors[i] = (beta_i_scaled * state.sigma_f * state.flux) / (LAMBDAS[i] * NEUTRON_GENERATION_TIME);
        }
        state
    }
}

impl ReactorState {
    pub fn new() -> Self {
        let mut state = Self::default();
        // Seed the chain with initial layout
        state.proof.feed_str("REACTOR_INITIALIZED_V2");
        state.proof.feed_f64(state.base_rho);
        state.proof.feed_f64(state.fuel_temp);
        state
    }

    /// Calculate macroscopic thermal absorption cross-section of Xenon poison
    pub fn xenon_macro_abs(&self) -> f64 {
        self.xenon_135 * SIGMA_A_XE
    }

    /// Reactivity penalty from Xenon-135.
    /// Simplified negative reactivity = - (Sigma_a_Xe / Total_Sigma_a)
    pub fn xenon_reactivity_worth(&self) -> f64 {
        let sigma_a_fuel = 0.1; // Assumed macroscopic absorption of fuel/moderator
        -(self.xenon_macro_abs() / sigma_a_fuel)
    }

    /// Net reactivity (rho) including Xenon poisoning and thermal feedbacks
    pub fn net_reactivity(&self) -> f64 {
        let thermal_feedback = ALPHA_D * (self.fuel_temp - FUEL_TEMP_0) + ALPHA_C * (self.coolant_temp - COOLANT_TEMP_0);
        self.base_rho + self.control_rods_rho + self.xenon_reactivity_worth() + thermal_feedback
    }

    pub fn effective_beta(&self) -> f64 {
        // Linearly burn down beta from U-235 (0.0065) to a Pu-239 heavy blend (0.0048) over 300 days
        let fraction = (self.days_burned / 300.0).clamp(0.0, 1.0);
        BETA - (fraction * (BETA - 0.0048))
    }

    /// Step the kinetics forward by dt (seconds)
    pub fn step(&mut self, dt: f64) {
        if self.prompt_critical {
            return; // Physics breakdown beyond this point without explosive hydrodynamics
        }

        self.time_s += dt;
        self.days_burned += dt / 86400.0; // Track operational age

        let rho = self.net_reactivity();
        let beta_eff = self.effective_beta();

        // Check for prompt criticality
        if rho >= beta_eff {
            self.prompt_critical = true;
            // Seal the exact moment of destruction
            self.proof.feed_str("FATAL_PROMPT_CRITICALITY");
            self.proof.feed_f64(self.time_s);
            self.proof.feed_f64(self.days_burned);
            self.proof.feed_f64(rho);
            self.proof.feed_f64(beta_eff);
            self.proof.feed_f64(self.xenon_135);
            return;
        }

        // ═══════════════════════════════════════════════════════════════════════════
        // 1. POINT KINETICS - Prompt Jump Approximation & Precursor updates
        // ═══════════════════════════════════════════════════════════════════════════
        // Scale beta_i group values proportionally to total beta_eff
        let mut betas_scaled = [0.0; 6];
        for i in 0..6 {
            betas_scaled[i] = BETAS[i] * (beta_eff / BETA);
        }

        // Calculate prompt jump algebraic flux
        let mut precursor_sum = 0.0;
        for i in 0..6 {
            precursor_sum += LAMBDAS[i] * self.precursors[i];
        }

        // S = spontaneous fission background source scaled to beta
        let source_term = 1e2 * beta_eff;
        let next_flux = (NEUTRON_GENERATION_TIME * precursor_sum + source_term) / (beta_eff - rho);
        self.flux = next_flux.max(100.0);

        // Update 6 precursor groups using unconditionally stable exponential integrators
        for i in 0..6 {
            let lambda = LAMBDAS[i];
            let beta_scaled = betas_scaled[i];
            let decay_factor = (-lambda * dt).exp();
            let production_rate = (beta_scaled * self.sigma_f * self.flux) / NEUTRON_GENERATION_TIME;
            self.precursors[i] = self.precursors[i] * decay_factor + (production_rate / lambda) * (1.0 - decay_factor);
        }

        // ═══════════════════════════════════════════════════════════════════════════
        // 2. FISSION PRODUCT POISONING - Iodine & Xenon dynamics (exponential updates)
        // ═══════════════════════════════════════════════════════════════════════════
        // dI/dt = gamma_I * Sigma_f * phi - lambda_I * I
        let i_decay_factor = (-LAMBDA_I * dt).exp();
        let i_production = GAMMA_I * self.sigma_f * self.flux;
        self.iodine_135 = self.iodine_135 * i_decay_factor + (i_production / LAMBDA_I) * (1.0 - i_decay_factor);

        // dXe/dt = gamma_Xe * Sigma_f * phi + lambda_I * I - (lambda_Xe + sigma_a_Xe * phi) * Xe
        let xe_dest_rate = LAMBDA_XE + SIGMA_A_XE * self.flux;
        let xe_decay_factor = (-xe_dest_rate * dt).exp();
        let xe_production = GAMMA_XE * self.sigma_f * self.flux + LAMBDA_I * self.iodine_135;
        self.xenon_135 = self.xenon_135 * xe_decay_factor + (xe_production / xe_dest_rate) * (1.0 - xe_decay_factor);

        // ═══════════════════════════════════════════════════════════════════════════
        // 3. THERMAL-HYDRAULICS - Coupled Fuel/Coolant temperatures (Implicit Euler)
        // ═══════════════════════════════════════════════════════════════════════════
        // dT_f/dt = FLUX_TO_HEAT * flux - H_FC * (T_f - T_c)
        // dT_c/dt = H_FC * (T_f - T_c) - W_C * (T_c - COOLANT_INLET_TEMP)
        let a11 = 1.0 + dt * H_FC;
        let a12 = -dt * H_FC;
        let a21 = -dt * H_FC;
        let a22 = 1.0 + dt * (H_FC + W_C);
        
        let b1 = self.fuel_temp + dt * FLUX_TO_HEAT * self.flux;
        let b2 = self.coolant_temp + dt * W_C * COOLANT_INLET_TEMP;
        
        let det = a11 * a22 - a12 * a21;
        if det.abs() > 1e-9 {
            self.fuel_temp = (b1 * a22 - b2 * a12) / det;
            self.coolant_temp = (b2 * a11 - b1 * a21) / det;
        }

        // ═══════════════════════════════════════════════════════════════════════════
        // 4. LAGRANGIAN RESIDUAL - Physical Inconsistency calculation
        // ═══════════════════════════════════════════════════════════════════════════
        // Evaluates divergence from the algebraic prompt-jump constraint
        let expected_numerator = NEUTRON_GENERATION_TIME * precursor_sum + source_term;
        let actual_numerator = (beta_eff - rho) * self.flux;
        self.residual = (actual_numerator - expected_numerator).abs();

        // Periodically feed continuous state to the hash
        if self.time_s as u64 % 60 == 0 {
            self.proof.feed_f64(self.time_s);
            self.proof.feed_f64(self.days_burned);
            self.proof.feed_f64(self.flux);
            self.proof.feed_f64(self.xenon_135);
            self.proof.feed_f64(self.fuel_temp);
            self.proof.feed_f64(self.residual);
        }
    }
    
    /// Operators pull control rods maliciously (or foolishly fighting the Xenon pit).
    /// Extracting rods injects positive reactivity by reducing the negative control worth.
    pub fn pull_control_rods(&mut self, positive_rho_injected: f64) {
        self.control_rods_rho += positive_rho_injected;
        self.proof.feed_str("CONTROL_ROD_PULL_V2");
        self.proof.feed_f64(self.time_s);
        self.proof.feed_f64(self.control_rods_rho);
    }

    /// Seal the timeline into a single irreversible SHA-256 hash.
    pub fn get_sealed_hash(self) -> String {
        self.proof.seal()
    }
}
