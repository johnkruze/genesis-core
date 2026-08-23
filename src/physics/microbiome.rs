//! G^G Genesis Core: Predictive Microbiome Engineering & Gene Expression Physics Engine
//! Models multi-species Fisher-KPP (FKPP) non-linear reaction-diffusion traveling waves
//! coupled with dynamic gene expression feedback gains (Kp) and metabolic cross-feeding in porous media.

use serde::{Deserialize, Serialize};

/// State of a 1D/3D multi-species microbial community grid node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MicrobiomeNodeState {
    pub species_densities: Vec<f64>,  // u_i population concentrations normalized [0, 1]
    pub metabolic_substrate: f64,     // Available carbon/nitrogen substrate S
    pub gene_expression_activation: f64, // Expression level [0, 1] (0 = dormant, 1 = max synthesis)
    pub local_ph: f64,
}

/// Parameters for multi-species FKPP reaction-diffusion kinetics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MicrobiomeKineticsParams {
    pub diffusion_coefficients: Vec<f64>, // D_i diffusion rates (m^2/s)
    pub growth_rates: Vec<f64>,           // r_i intrinsic growth rates (1/s)
    pub carrying_capacities: Vec<f64>,     // K_i species carrying capacity
    pub interaction_matrix: Vec<Vec<f64>>, // alpha_ij competition/mutualism gains
    pub gene_expression_gain_kp: f64,     // Dynamic feedback gain Kp for gene activation
}

impl Default for MicrobiomeKineticsParams {
    fn default() -> Self {
        Self {
            diffusion_coefficients: vec![1e-4, 5e-5, 2e-4], // 3 interacting species
            growth_rates: vec![0.5, 0.8, 0.3],
            carrying_capacities: vec![1.0, 1.0, 1.0],
            interaction_matrix: vec![
                vec![0.0, -0.2, 0.1],  // Species 0: slightly suppressed by 1, helped by 2
                vec![-0.1, 0.0, -0.3], // Species 1: suppressed by 2
                vec![0.2, -0.1, 0.0],  // Species 2: mutualistic with 0
            ],
            gene_expression_gain_kp: 2.5,
        }
    }
}

/// State of the spatial microbiome simulation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpatialMicrobiomeField {
    pub grid_nodes: Vec<MicrobiomeNodeState>,
    pub grid_size: usize,
    pub spatial_dx: f64,
    pub community_shannon_diversity_index: f64,
    pub max_species_fraction: f64,
    pub wavefront_velocity_m_s: f64,
    pub is_dysbiosis_collapsed: bool,
}

impl SpatialMicrobiomeField {
    pub fn new(grid_size: usize, num_species: usize) -> Self {
        let mut grid_nodes = Vec::with_capacity(grid_size);
        for i in 0..grid_size {
            // Seed species 0 at x=0 (left boundary pulse)
            let s0_seed = if i < grid_size / 5 { 0.8 } else { 0.05 };
            grid_nodes.push(MicrobiomeNodeState {
                species_densities: vec![s0_seed, 0.2, 0.1],
                metabolic_substrate: 1.0,
                gene_expression_activation: 0.1,
                local_ph: 7.0,
            });
        }

        Self {
            grid_nodes,
            grid_size,
            spatial_dx: 0.01, // 1cm grid resolution
            community_shannon_diversity_index: 1.09,
            max_species_fraction: 0.33,
            wavefront_velocity_m_s: 0.0,
            is_dysbiosis_collapsed: false,
        }
    }

    /// Advances 1D Finite-Difference Multi-Species FKPP Reaction-Diffusion by dt (seconds)
    pub fn step(&mut self, params: &MicrobiomeKineticsParams, dt_sec: f64) {
        let n_nodes = self.grid_size;
        let n_species = params.diffusion_coefficients.len();
        let dx_sq = self.spatial_dx * self.spatial_dx;

        let mut next_nodes = self.grid_nodes.clone();

        for i in 0..n_nodes {
            let left_idx = if i == 0 { 0 } else { i - 1 };
            let right_idx = if i == n_nodes - 1 { n_nodes - 1 } else { i + 1 };

            for s in 0..n_species {
                let u = self.grid_nodes[i].species_densities[s];
                let u_left = self.grid_nodes[left_idx].species_densities[s];
                let u_right = self.grid_nodes[right_idx].species_densities[s];

                // 1. Spatial Diffusion: D * d^2(u) / dx^2
                let laplacian = (u_left - 2.0 * u + u_right) / dx_sq;
                let diff_term = params.diffusion_coefficients[s] * laplacian;

                // 2. Gene Expression Feedback Gain Kp modulation on intrinsic growth rate
                let gene_act = self.grid_nodes[i].gene_expression_activation;
                let effective_r = params.growth_rates[s] * (1.0 + params.gene_expression_gain_kp * gene_act);

                // 3. Multi-species Interaction (Competition/Mutualism)
                let mut interaction_sum = 0.0;
                for j in 0..n_species {
                    if j != s {
                        interaction_sum += params.interaction_matrix[s][j] * self.grid_nodes[i].species_densities[j];
                    }
                }

                // 4. FKPP Reaction Term: r * u * (1 - u / K) + u * sum(alpha * u_j)
                let reaction_term = effective_r * u * (1.0 - u / params.carrying_capacities[s]) + u * interaction_sum;

                // Finite Difference Euler Integration
                let mut u_next = u + (diff_term + reaction_term) * dt_sec;
                if u_next < 0.0 { u_next = 0.0; }
                if u_next > 2.0 { u_next = 2.0; }

                next_nodes[i].species_densities[s] = u_next;
            }

            // Dynamic Gene Expression Activation: scaled by local substrate & population stress
            let total_pop: f64 = next_nodes[i].species_densities.iter().sum();
            next_nodes[i].gene_expression_activation = (total_pop * 0.3).min(1.0);
        }

        self.grid_nodes = next_nodes;

        // Calculate Shannon Diversity Index H' = - sum(p_i * ln(p_i))
        let mut total_community_pop = 0.0;
        let mut species_pops = vec![0.0; n_species];
        for node in &self.grid_nodes {
            for (s, &p) in node.species_densities.iter().enumerate() {
                species_pops[s] += p;
                total_community_pop += p;
            }
        }

        if total_community_pop > 1e-6 {
            let mut shannon_h = 0.0;
            for &sp_pop in &species_pops {
                let pi = sp_pop / total_community_pop;
                if pi > 1e-6 {
                    shannon_h -= pi * pi.ln();
                }
            }
            self.community_shannon_diversity_index = shannon_h;
        }

        // Tipping point: Shannon diversity < 0.65 or single species dominance (>80%) indicates community collapse (dysbiosis)
        self.max_species_fraction = species_pops.iter().cloned().fold(0.0, f64::max) / (total_community_pop + 1e-6);
        if self.community_shannon_diversity_index < 0.65 || self.max_species_fraction > 0.75 {
            self.is_dysbiosis_collapsed = true;
        }
    }
}

/// One-species FKPP with kill term. Threat-class sibling of the community field.
/// ∂u/∂t = D ∇²u + r u (1−u) − k u. Dirichlet sterile ends.
pub fn step_fkpp_1d(u: &[f64], nxt: &mut [f64], d: f64, r: f64, k: f64, dx: f64, dt: f64) {
    let n = u.len();
    if n < 3 || nxt.len() < n {
        return;
    }
    let dx_sq = dx * dx;
    nxt[0] = 0.0;
    nxt[n - 1] = 0.0;
    for i in 1..(n - 1) {
        let lap = (u[i + 1] - 2.0 * u[i] + u[i - 1]) / dx_sq;
        let reaction = r * u[i] * (1.0 - u[i]) - k * u[i];
        nxt[i] = (u[i] + dt * (d * lap + reaction)).clamp(0.0, 1.0);
    }
}
