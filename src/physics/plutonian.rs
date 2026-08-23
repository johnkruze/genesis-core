//! Plutonian core — the vision: outcomes without explosion.
//! Reconstructible twin: `plutonian_humanoid_deep_time` (Pu-238 watts, tendon mm, gait).
//! This module is the phase-shift language of that same seeing. They are one organ.

use std::collections::HashMap;

/// Pu-238 RTG — reconstructible deep time. t½ = 87.7 y.
pub const PU238_HALF_LIFE_YEARS: f64 = 87.7;
pub const PU238_LAMBDA: f64 = std::f64::consts::LN_2 / PU238_HALF_LIFE_YEARS;

/// P(t) = P0 e^{−λt}. Watts at year t.
pub fn rtg_power_watts(p0: f64, t_years: f64) -> f64 {
    p0 * (-PU238_LAMBDA * t_years).exp()
}

#[derive(Debug, Clone)]
pub struct PhaseState {
    pub energy_level: f64,
    pub coherence: f64,
    pub entropy: f64,
}

#[derive(Debug, Clone)]
pub struct PlutonianCore {
    pub nodes: HashMap<String, PhaseState>,
    pub base_decay_rate: f64,
    pub phase_shift_threshold: f64,
    pub temporal_compression: f64, 
}

impl Default for PlutonianCore {
    fn default() -> Self {
        Self {
            nodes: HashMap::new(),
            base_decay_rate: 0.0001,
            phase_shift_threshold: 0.85,
            temporal_compression: 1.0,
        }
    }
}

impl PlutonianCore {
    pub fn insert_node(&mut self, id: &str, state: PhaseState) {
        self.nodes.insert(id.to_string(), state);
    }

    /// Advances the deep-time simulation. Returns true if a phase shift occurred.
    pub fn step_time(&mut self, dt_years: f64) -> bool {
        let mut global_shift = false;
        let mut updates = Vec::new();

        for (id, state) in &self.nodes {
            let mut new_state = state.clone();
            
            // Entropy increases logarithmically with deep time
            let entropy_delta = self.base_decay_rate * dt_years * self.temporal_compression * (1.1 - state.coherence);
            new_state.entropy += entropy_delta;
            
            // Coherence degrades unless energy is high
            if new_state.energy_level < 0.5 {
                new_state.coherence -= entropy_delta * 0.5;
            }

            // Phase shift condition (decay Curve saturation)
            if new_state.entropy > self.phase_shift_threshold && new_state.coherence > 0.4 {
                // The Phase Shift: Entropy crystallizes into structure, dropping local entropy and spiking coherence
                new_state.entropy *= 0.1;
                new_state.coherence = (new_state.coherence + 0.5).min(1.0);
                new_state.energy_level *= 0.5; // Shift consumes energy
                global_shift = true;
            }

            // Bound constraints
            new_state.coherence = new_state.coherence.clamp(0.01, 1.0);
            new_state.entropy = new_state.entropy.max(0.0);
            new_state.energy_level = new_state.energy_level.max(0.0);

            updates.push((id.clone(), new_state));
        }

        for (id, new_state) in updates {
            self.nodes.insert(id, new_state);
        }

        global_shift
    }
}
