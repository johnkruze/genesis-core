//! G^G Genesis Core: Genome Engineering & Chromatin Mechanical Physics Engine
//! Models 3D supercoiled chromatin strain, multiplexed CRISPR/Cas binding kinetics,
//! off-target cleavage risk, and cellular repair energy budgets.

use serde::{Deserialize, Serialize};

/// State of a multiplexed genome editing event in a cellular chassis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenomeEditState {
    pub target_edit_count: usize,          // Number of simultaneous edits (10 - 200)
    pub guide_rna_affinity_dg: f64,        // Binding free energy Delta G (kJ/mol)
    pub chromatin_torsional_strain_pa: f64, // Chromatin mechanical supercoil strain
    pub off_target_cleavage_count: usize,   // Unintended off-target double-strand breaks
    pub cellular_repair_energy_kj: f64,    // Remaining ATP maintenance budget
    pub genomic_integrity_pct: f64,        // Overall structural genome stability
    pub is_chromosomal_translocation: bool,
}

/// Parameters for genome engineering kinetics and physical constraints
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenomeEngineeringParams {
    pub temperature_k: f64,
    pub max_repair_energy_budget_kj: f64,
    pub off_target_mismatch_penalty_dg: f64,
}

impl Default for GenomeEngineeringParams {
    fn default() -> Self {
        Self {
            temperature_k: 310.15, // 37°C
            max_repair_energy_budget_kj: 2500.0,
            off_target_mismatch_penalty_dg: 4.2, // kJ/mol per base mismatch
        }
    }
}

impl GenomeEditState {
    pub fn new(edits: usize, affinity_dg: f64) -> Self {
        Self {
            target_edit_count: edits,
            guide_rna_affinity_dg: affinity_dg,
            chromatin_torsional_strain_pa: 10.0,
            off_target_cleavage_count: 0,
            cellular_repair_energy_kj: 1500.0,
            genomic_integrity_pct: 100.0,
            is_chromosomal_translocation: false,
        }
    }

    /// Advances Genome Editing Physical Mechanics & Repair Kinetics by dt (seconds)
    pub fn step(&mut self, params: &GenomeEngineeringParams, dt_sec: f64) {
        // Torsional strain increases non-linearly with simultaneous double-strand cuts
        let cut_density_factor = self.target_edit_count as f64 * 0.15;
        self.chromatin_torsional_strain_pa = 10.0 + (cut_density_factor * cut_density_factor * 8.5);

        // Off-target cleavage probability scales with thermal fluctuation & gRNA mismatch penalty
        let thermal_kb_t = 8.314e-3 * params.temperature_k;
        let binding_prob = (-self.guide_rna_affinity_dg / thermal_kb_t).exp();

        let off_target_rate = (self.target_edit_count as f64 * 0.08) * (1.0 / (binding_prob + 0.1));
        self.off_target_cleavage_count = (off_target_rate * dt_sec * 50.0) as usize;

        // Cellular repair energy cost: 12.5 kJ per double-strand break repair
        let total_breaks = self.target_edit_count + self.off_target_cleavage_count;
        let repair_cost = total_breaks as f64 * 12.5 * dt_sec;
        self.cellular_repair_energy_kj = (self.cellular_repair_energy_kj - repair_cost).max(0.0);

        // Genomic structural integrity drops if repair energy depletes or off-target breaks spike
        let strain_penalty = (self.chromatin_torsional_strain_pa - 50.0).max(0.0) * 0.4;
        let energy_depletion_penalty = (1.0 - (self.cellular_repair_energy_kj / params.max_repair_energy_budget_kj)) * 50.0;

        self.genomic_integrity_pct = (100.0 - strain_penalty - energy_depletion_penalty - (self.off_target_cleavage_count as f64 * 2.5)).max(0.0);

        if self.genomic_integrity_pct < 40.0 || self.off_target_cleavage_count > 15 {
            self.is_chromosomal_translocation = true;
        }
    }
}
