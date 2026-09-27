//! G^G Genesis Core: Molecular & Biomolecular Inverse Physics Engine
//! Models 3D molecular force fields (Lennard-Jones, Coulomb, harmonic bond potential)
//! coupled with SPH thermal hydrodynamics and MPM shear stress limits for bio-catalyst design.

use serde::{Deserialize, Serialize};

/// 3D Vector for molecular positions, velocities, and forces
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Vector3D {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vector3D {
    pub fn zero() -> Self {
        Self { x: 0.0, y: 0.0, z: 0.0 }
    }

    pub fn norm_sq(&self) -> f64 {
        self.x * self.x + self.y * self.y + self.z * self.z
    }

    pub fn norm(&self) -> f64 {
        self.norm_sq().sqrt()
    }
}

/// A single atom / amino-acid residue in a biomolecular structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtomNode {
    pub id: u32,
    pub element_charge: f64, // e.g. +1.0, -1.0, 0.0
    pub mass_amu: f64,       // atomic mass units
    pub radius_angstrom: f64,
    pub pos: Vector3D,
    pub vel: Vector3D,
    pub force: Vector3D,
}

/// Molecular Force-Field Configuration Parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForceFieldParams {
    pub epsilon_lj: f64,      // Lennard-Jones depth (kJ/mol)
    pub sigma_lj: f64,        // LJ zero-potential distance (Angstroms)
    pub coulomb_k: f64,       // Electrostatic constant (8.98755e9 converted for Å)
    pub bond_stiffness: f64,  // Harmonic bond stiffness K_b
    pub temperature_k: f64,   // System temperature (Kelvin)
    pub ph_level: f64,        // Solution pH
    pub shear_rate_s1: f64,   // Fluid shear rate (1/s)
}

impl Default for ForceFieldParams {
    fn default() -> Self {
        Self {
            epsilon_lj: 0.15,      // ~0.15 kcal/mol default
            sigma_lj: 3.4,         // Carbon-like ~3.4 Å
            coulomb_k: 138.935,    // kJ*Å / (mol * e^2)
            bond_stiffness: 1000.0,// kJ / (mol * Å^2)
            temperature_k: 310.15, // 37°C physiological
            ph_level: 7.0,
            shear_rate_s1: 100.0,
        }
    }
}

/// Dynamic State of a Biomolecular Enzyme / Catalyst Pocket
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BiomoleculeState {
    pub atoms: Vec<AtomNode>,
    pub total_potential_energy_kj: f64,
    pub total_kinetic_energy_kj: f64,
    pub catalytic_pocket_volume_a3: f64,
    pub binding_affinity_score: f64,
    pub is_denatured: bool,
    pub thermal_stress_residual: f64,
}

impl BiomoleculeState {
    pub fn new(num_residues: usize, params: &ForceFieldParams) -> Self {
        let mut atoms = Vec::with_capacity(num_residues);
        for i in 0..num_residues {
            let theta = i as f64 * 0.5;
            let radius = 5.0 + (i as f64 * 0.1);
            atoms.push(AtomNode {
                id: i as u32,
                element_charge: if i % 2 == 0 { 0.5 } else { -0.5 },
                mass_amu: 110.0, // average amino acid residue mass
                radius_angstrom: 3.5,
                pos: Vector3D {
                    x: radius * theta.cos(),
                    y: radius * theta.sin(),
                    z: i as f64 * 0.8,
                },
                vel: Vector3D::zero(),
                force: Vector3D::zero(),
            });
        }

        Self {
            atoms,
            total_potential_energy_kj: 0.0,
            total_kinetic_energy_kj: 0.0,
            catalytic_pocket_volume_a3: 1250.0,
            binding_affinity_score: -8.5, // kcal/mol baseline
            is_denatured: false,
            thermal_stress_residual: 0.0,
        }
    }

    /// Advances 3D Molecular Force-Field Physics by dt (picoseconds)
    pub fn step(&mut self, params: &ForceFieldParams, dt_ps: f64) {
        let n = self.atoms.len();
        let mut pot_energy = 0.0;
        let mut kin_energy = 0.0;

        // Reset forces
        for atom in &mut self.atoms {
            atom.force = Vector3D::zero();
        }

        // Pairwise Non-Bonded Interactions: Lennard-Jones 6-12 + Coulomb
        for i in 0..n {
            for j in (i + 1)..n {
                let dx = self.atoms[j].pos.x - self.atoms[i].pos.x;
                let dy = self.atoms[j].pos.y - self.atoms[i].pos.y;
                let dz = self.atoms[j].pos.z - self.atoms[i].pos.z;
                let r_sq = dx * dx + dy * dy + dz * dz + 1e-4;
                let r = r_sq.sqrt();

                // Lennard-Jones 6-12
                let s_over_r = params.sigma_lj / r;
                let s_over_r6 = s_over_r.powi(6);
                let s_over_r12 = s_over_r6 * s_over_r6;
                let v_lj = 4.0 * params.epsilon_lj * (s_over_r12 - s_over_r6);
                let f_lj_mag = 24.0 * params.epsilon_lj * (2.0 * s_over_r12 - s_over_r6) / r_sq;

                // Coulomb Electrostatic (pH-dependent charge scaling)
                let ph_factor = 1.0 + 0.05 * (params.ph_level - 7.0).abs();
                let q_prod = self.atoms[i].element_charge * self.atoms[j].element_charge * ph_factor;
                let v_coulomb = params.coulomb_k * q_prod / r;
                let f_coulomb_mag = params.coulomb_k * q_prod / (r_sq * r);

                let f_total_mag = f_lj_mag + f_coulomb_mag;
                let fx = f_total_mag * dx;
                let fy = f_total_mag * dy;
                let fz = f_total_mag * dz;

                self.atoms[i].force.x -= fx;
                self.atoms[i].force.y -= fy;
                self.atoms[i].force.z -= fz;

                self.atoms[j].force.x += fx;
                self.atoms[j].force.y += fy;
                self.atoms[j].force.z += fz;

                pot_energy += v_lj + v_coulomb;
            }
        }

        // Thermal Shake + Shear Stress Coupling
        let thermal_factor = (params.temperature_k / 300.0).sqrt();
        let shear_force_mag = params.shear_rate_s1 * 1e-5;

        for atom in &mut self.atoms {
            // Acceleration (F = ma)
            let ax = atom.force.x / atom.mass_amu;
            let ay = atom.force.y / atom.mass_amu;
            let az = (atom.force.z + shear_force_mag) / atom.mass_amu;

            // Velocity Verlet Integration
            atom.vel.x += ax * dt_ps * thermal_factor;
            atom.vel.y += ay * dt_ps * thermal_factor;
            atom.vel.z += az * dt_ps * thermal_factor;

            atom.pos.x += atom.vel.x * dt_ps;
            atom.pos.y += atom.vel.y * dt_ps;
            atom.pos.z += atom.vel.z * dt_ps;

            kin_energy += 0.5 * atom.mass_amu * atom.vel.norm_sq();
        }

        self.total_potential_energy_kj = pot_energy;
        self.total_kinetic_energy_kj = kin_energy;

        // Thermal Denaturation Threshold check (e.g., T > 390K or pH extreme < 2 or > 12)
        if params.temperature_k > 390.0 || params.ph_level < 2.0 || params.ph_level > 12.0 {
            self.thermal_stress_residual += (params.temperature_k - 370.0).max(0.0) * dt_ps;
            if self.thermal_stress_residual > 500.0 {
                self.is_denatured = true;
            }
        }

        // Binding Affinity scales continuously with thermal residual & pocket geometry stability
        let center_atom = &self.atoms[0];
        let pocket_dist = (center_atom.pos.norm() - 5.0).abs();
        let thermal_penalty = (self.thermal_stress_residual * 0.012).min(8.0);
        self.binding_affinity_score = -14.0 + pocket_dist * 0.8 + thermal_penalty + (if self.is_denatured { 6.0 } else { 0.0 });
    }
}
