//! Josephson Junction Phase Dynamics Substrate (Quantum Transmon Qubit)
//! THE EMBODIMENT: The "Quantum State". A Josephson junction shunted by capacitance and resistance.
//! The controller is a classical microwave pulse generator operating under statistical modeling.
//! The physical junction behaves as a non-linear pendulum, governed by classical conservation laws.
//! When thermal fluctuations in the dilution refrigerator exceed 100mK, phase coherence collapses.
//! ZTP audits this phase-dynamic boundary, proving consistency of charge and momentum on-die.

use crate::proof::ProofChain;
use rand::SeedableRng;

// Normalized constants for ps/ns dynamics:
// We work in time unit of NANOSECONDS (ns).
// Current in NANOAMPERES (nA).
// Phase in RADIANS (rad).
// 
// For a typical transmon:
// C = 100 fF = 100e-15 F
// R = 1000 Ohm = 1 kOhm
// Ic = 20 nA
//
// Normalized constants:
// gamma = 1 / (R * C) = 10^10 s^-1 = 10.0 ns^-1 (damping)
// omega_p^2 = 2e * Ic / (hbar * C) = 600.0 ns^-2 (plasma frequency squared)
// eta = 2e / (hbar * C) = 30.0 nA^-1 * ns^-2 (control coupling)

pub const OMEGA_P_SQ: f64 = 600.0; // ns^-2
pub const GAMMA: f64 = 10.0;       // ns^-1
pub const ETA: f64 = 30.0;         // nA^-1 * ns^-2

#[derive(Debug, Clone)]
pub struct JosephsonState {
    pub phase: f64,             // rad
    pub phase_velocity: f64,    // rad/ns (proportional to junction voltage)
    pub bias_current: f64,      // nA (control input)
    pub thermal_current: f64,   // nA (thermal fluctuation noise)
    
    pub temp_mk: f64,           // Dilution refrigerator temperature (mK)
    pub coherence: f64,         // Qubit coherence metric (0.0 to 1.0)
    pub residual: f64,          // ZTP Inconsistency residual (Lagrangian RCSJ residual)
    
    pub time_ns: f64,           // Nanoseconds
    pub quenched: bool,         // Qubit has decohered/quenched
    pub proof: ProofChain,
}

impl Default for JosephsonState {
    fn default() -> Self {
        Self {
            phase: 0.0,
            phase_velocity: 0.0,
            bias_current: 0.0,
            thermal_current: 0.0,
            temp_mk: 10.0,         // 10 mK (nominal superconducting state)
            coherence: 1.0,        // 100% coherence at start
            residual: 0.0,
            time_ns: 0.0,
            quenched: false,
            proof: ProofChain::new(),
        }
    }
}

impl JosephsonState {
    pub fn new() -> Self {
        let mut sim = Self::default();
        sim.proof.feed_str("JOSEPHSON_IGNITED_V1");
        sim.proof.feed_f64(sim.phase);
        sim.proof.feed_f64(sim.temp_mk);
        sim
    }

    /// Step the Josephson junction dynamics (typically dt = 0.001 ns / 1 ps)
    pub fn step(&mut self, dt: f64, control_current: f64, noise_seed: u64) {
        if self.quenched { return; }

        let old_phase = self.phase;
        self.bias_current = control_current;


        // Generate thermal noise current using the Box-Muller transform
        // Standard deviation scales with sqrt(temp_mk)
        let mut rng = rand::rngs::StdRng::seed_from_u64(noise_seed ^ (self.time_ns.to_bits() as u64));
        use rand::Rng;
        let u1: f64 = rng.gen_range(0.0..1.0);
        let u2: f64 = rng.gen_range(0.0..1.0);

        let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();

        let thermal_std = 0.05 * (self.temp_mk).sqrt();
        self.thermal_current = z * thermal_std;

        let total_current = self.bias_current + self.thermal_current;

        // ═══════════════════════════════════════════════════════════════════════════
        // 1. DYNAMICS: RCSJ Pendulum Integration (Semi-Implicit Euler)
        // ═══════════════════════════════════════════════════════════════════════════
        let old_velocity = self.phase_velocity;
        
        // Acceleration = eta * I_total - omega_p^2 * sin(phi) - gamma * dphi/dt
        let acceleration = ETA * total_current - OMEGA_P_SQ * self.phase.sin() - GAMMA * self.phase_velocity;
        
        self.phase_velocity += acceleration * dt;
        self.phase += self.phase_velocity * dt;
        
        self.time_ns += dt;

        // ═══════════════════════════════════════════════════════════════════════════
        // 2. COHERENCE DECAY (Quantum phase slips and thermal decoherence)
        // ═══════════════════════════════════════════════════════════════════════════
        // A phase slip (phase wrapping past 2pi) destroys quantum coherence
        let phase_slip_factor = if self.phase.abs() > std::f64::consts::PI * 2.0 {
            0.05 * (self.phase.abs() / (std::f64::consts::PI * 2.0))
        } else {
            0.0
        };

        // Thermal decoherence rate increases exponentially with temperature above 20 mK
        let thermal_decoherence_rate = if self.temp_mk > 20.0 {
            1e-4 * (self.temp_mk - 20.0).exp()
        } else {
            0.0
        };

        let decay = (phase_slip_factor + thermal_decoherence_rate) * dt;
        self.coherence = (self.coherence - decay).clamp(0.0, 1.0);

        // ═══════════════════════════════════════════════════════════════════════════
        // 3. LAGRANGIAN RESIDUAL (ZTP Inconsistency residual)
        // ═══════════════════════════════════════════════════════════════════════════
        let actual_acceleration = (self.phase_velocity - old_velocity) / dt;
        let expected_acceleration = ETA * total_current - OMEGA_P_SQ * old_phase.sin() - GAMMA * old_velocity;
        self.residual = (actual_acceleration - expected_acceleration).abs();


        // Check if the qubit is quenched (critical failure)
        // Dilution refrigerator thermal runaway (T > 100 mK) or absolute coherence collapse
        if self.coherence < 0.005 || self.temp_mk > 100.0 {
            self.quenched = true;
            self.proof.feed_str("QUBIT_DECOHERED_QUENCH");
            self.proof.feed_f64(self.time_ns);
            self.proof.feed_f64(self.phase);
            self.proof.feed_f64(self.coherence);
        }

        // Feed trajectory to proof chain periodically
        let step_count = (self.time_ns / dt).round() as u64;
        if step_count % 100 == 0 {
            self.proof.feed_f64(self.time_ns);
            self.proof.feed_f64(self.phase);
            self.proof.feed_f64(self.coherence);
            self.proof.feed_f64(self.residual);
        }
    }

    /// Seal the run
    pub fn get_sealed_hash(self) -> String {
        self.proof.seal()
    }
}
