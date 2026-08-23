// Compounding (Biological Fluids & Biotech Production)
// THE EMBODIMENT: Modeling blood rheology, stomach gastric motility, and high-shear protein denaturing.

use crate::proof::ProofChain;

/// Reconstructible gates. Must match `ztp-runtime` compounding FFI defaults.
pub const POTENCY_COLLAPSE_PCT: f64 = 80.0;
pub const DISSOLUTION_STALL_PCT: f64 = 70.0;
pub const SHEAR_GATE_BROTH_PA: f64 = 15.0;
pub const SHEAR_GATE_DEFAULT_PA: f64 = 500.0;
pub const OSTWALD_K: f64 = 0.015;
pub const OSTWALD_N: f64 = 0.70;
pub const NOYES_D: f64 = 5.0e-10;
pub const NOYES_H: f64 = 1.0e-5;
pub const NOYES_CS: f64 = 50.0;

#[derive(Debug, Clone)]
pub struct CompoundingState {
    // Spatial coordinates/velocity (as SPH-representative particle / packet)
    pub position: [f64; 3],
    pub velocity: [f64; 3],
    
    // Constituent concentrations (fractions sum to 1.0)
    pub api_concentration: f64,        // Active Pharmaceutical Ingredient (kg/m^3)
    pub solvent_concentration: f64,    // Solvent (kg/m^3)
    pub excipient_concentration: f64,  // Excipients/binder (kg/m^3)
    
    // Solid solute tracker (for dissolution)
    pub solid_mass_kg: f64,            // Mass of undissolved solid drug (kg)
    pub solid_surface_area_m2: f64,     // Active surface area of solid drug particles (m^2)
    
    // Fluid properties
    pub temperature_c: f64,            // Fluid temperature (°C)
    pub ph: f64,                       // pH (1.5 in stomach, 7.4 in blood)
    pub shear_rate: f64,               // Local shear rate (s^-1)
    pub viscosity: f64,                // Dynamic viscosity (Pa*s)
    
    // Non-Newtonian Ostwald-de Waele parameters
    pub flow_consistency_index_k: f64, // K (Pa*s^n)
    pub flow_behavior_index_n: f64,    // n (dimensionless, <1 for shear-thinning)
    
    // Noyes-Whitney constants
    pub diffusion_coefficient: f64,    // D (m^2/s)
    pub boundary_layer_h: f64,         // h (m)
    pub solubility_limit_cs: f64,      // Cs (kg/m^3)
    
    // Shear degradation tracking
    pub accumulated_shear_stress: f64, // Integral of (viscosity * shear_rate) over time (Pa)
    pub critical_shear_limit: f64,      // Max shear history before denaturing / cell lysis (Pa)
    pub active_potency: f64,           // Potency multiplier (1.0 = fully potent, 0.0 = completely denatured)
    
    pub time_s: f64,
    pub proof: ProofChain,
}

impl Default for CompoundingState {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0, 0.0],
            velocity: [1.0, 0.0, 0.0],
            api_concentration: 0.0,       // Starts undissolved
            solvent_concentration: 990.0, // Water/solvent baseline
            excipient_concentration: 10.0,
            solid_mass_kg: 0.100,         // 100 grams of solid drug
            solid_surface_area_m2: 0.05,  // 0.05 m^2 (initial surface area)
            temperature_c: 37.0,          // Body temp
            ph: 1.5,                      // Gastric pH
            shear_rate: 1.0,
            viscosity: 0.001,             // Viscosity of water-like solvent at start
            flow_consistency_index_k: OSTWALD_K,
            flow_behavior_index_n: OSTWALD_N,
            diffusion_coefficient: NOYES_D,
            boundary_layer_h: NOYES_H,
            solubility_limit_cs: NOYES_CS,
            accumulated_shear_stress: 0.0,
            critical_shear_limit: SHEAR_GATE_DEFAULT_PA,
            active_potency: 1.0,           // Potency multiplier
            time_s: 0.0,
            proof: ProofChain::new(),
        }
    }
}

impl CompoundingState {
    pub fn new_stomach_state() -> Self {
        let mut state = Self::default();
        state.ph = 1.5;
        state.flow_consistency_index_k = 0.025; // Thicker gastric slurry
        state.flow_behavior_index_n = 0.65;    // Strongly shear-thinning
        state.solubility_limit_cs = 40.0;
        state.solid_mass_kg = 0.001;            // 1.0 gram solid pill
        state.solid_surface_area_m2 = 0.05;     // 500 cm^2 initial surface area (granular dispersion)
        state
    }

    pub fn new_blood_state() -> Self {
        let mut state = Self::default();
        state.ph = 7.4;
        state.flow_consistency_index_k = 0.0035; // Blood plasma + cells
        state.flow_behavior_index_n = 0.70;     // Pseudoplastic blood behavior
        state.solubility_limit_cs = 60.0;
        state.diffusion_coefficient = 8.0e-10;  // Higher diffusion in blood serum
        state.solid_mass_kg = 0.0;              // Already dissolved in serum
        state.solid_surface_area_m2 = 0.0;
        state.api_concentration = 2.0;          // 2 kg/m^3 initial API concentration
        state
    }

    pub fn new_bioreactor_state() -> Self {
        let mut state = Self::default();
        state.ph = 7.2;
        state.flow_consistency_index_k = 0.015;  // Thicker biologic broth with cells
        state.flow_behavior_index_n = 0.85;     // Shear-thinning cell broth
        state.critical_shear_limit = SHEAR_GATE_BROTH_PA;
        state.solid_mass_kg = 0.0;
        state.solid_surface_area_m2 = 0.0;
        state.api_concentration = 5.0;           // 5 kg/m^3 of active protein
        state
    }

    /// Evaluates effective viscosity based on local shear rate
    pub fn update_viscosity(&mut self) {
        // Clamp shear rate to avoid division by zero or infinite viscosity at zero shear
        let cl_shear = self.shear_rate.max(1e-3);
        self.viscosity = self.flow_consistency_index_k * cl_shear.powf(self.flow_behavior_index_n - 1.0);
        // Safety bounds for dynamic viscosity (water to thick gel)
        self.viscosity = self.viscosity.clamp(0.0005, 5.0);
    }

    /// Computes Noyes-Whitney dissolution step
    pub fn step_dissolution(&mut self, dt: f64) {
        if self.solid_mass_kg <= 0.0 {
            self.solid_mass_kg = 0.0;
            self.solid_surface_area_m2 = 0.0;
            return;
        }

        // Noyes-Whitney: dM/dt = (D * A / h) * (Cs - C)
        let c_current = self.api_concentration;
        let c_diff = self.solubility_limit_cs - c_current;

        if c_diff <= 0.0 {
            // Saturated: no more dissolution
            return;
        }

        // Mass transfer rate
        let dm_dt = (self.diffusion_coefficient * self.solid_surface_area_m2 / self.boundary_layer_h) * c_diff;
        let mut dissolved_mass = dm_dt * dt;

        // Cap dissolved mass by available solid mass
        if dissolved_mass > self.solid_mass_kg {
            dissolved_mass = self.solid_mass_kg;
        }

        // Keep track of pre-dissolved mass for recursive scaling
        let previous_mass = self.solid_mass_kg;

        // Transfer mass: solid -> liquid concentration
        self.solid_mass_kg -= dissolved_mass;
        self.api_concentration += dissolved_mass; // api_concentration represents mass dissolved in local packet volume (1 m^3 normalized)

        // Dynamically shrink the surface area recursively as solid mass decreases
        // Modeling solid as spherical particles: A is proportional to M^(2/3)
        // A(t + dt) = A(t) * (M(t + dt) / M(t))^(2/3)
        if self.solid_mass_kg > 0.0 && previous_mass > 0.0 {
            self.solid_surface_area_m2 *= (self.solid_mass_kg / previous_mass).powf(2.0 / 3.0);
        } else {
            self.solid_surface_area_m2 = 0.0;
        }
    }

    /// Integrates accumulated shear stress and computes potency degradation
    pub fn step_shear_degradation(&mut self, dt: f64) {
        // Local shear stress = viscosity * shear_rate (Pa)
        let shear_stress = self.viscosity * self.shear_rate;
        self.accumulated_shear_stress += shear_stress * dt;

        if self.accumulated_shear_stress > self.critical_shear_limit {
            // Exponential decay of active potency when critical shear threshold is breached
            let overshoot = self.accumulated_shear_stress - self.critical_shear_limit;
            let decay_rate = 0.02; // sensitivity parameter
            self.active_potency = (-decay_rate * overshoot).exp();
            self.active_potency = self.active_potency.clamp(0.0, 1.0);
        }
    }

    /// Advances the system state by timestep dt
    pub fn step(&mut self, commanded_shear_rate: f64, temperature_shift: f64, dt: f64) {
        self.time_s += dt;
        self.shear_rate = commanded_shear_rate.max(0.0);
        self.temperature_c = (self.temperature_c + temperature_shift).clamp(1.0, 100.0);

        // 1. Update dynamic viscosity based on shear thinning behavior
        self.update_viscosity();

        // 2. Perform dissolution
        self.step_dissolution(dt);

        // 3. Accumulate mechanical shear stress and check degradation limits
        self.step_shear_degradation(dt);

        // 4. Feed variables to cryptographic proof chain
        self.proof.feed_f64(self.time_s);
        self.proof.feed_f64(self.viscosity);
        self.proof.feed_f64(self.api_concentration);
        self.proof.feed_f64(self.solid_mass_kg);
        self.proof.feed_f64(self.accumulated_shear_stress);
        self.proof.feed_f64(self.active_potency);
    }
}
