//! G^G Genesis Core: Advanced Materials Inverse Physics & Stress Tensor Engine
//! Models multi-scale Cauchy stress tensors (sigma_ij), Von Mises yield criteria,
//! crystallographic grain boundary shear, and principal stress eigenvector alignment.

use serde::{Deserialize, Serialize};

/// 3x3 Symmetric Cauchy Stress Tensor (MPa)
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CauchyStressTensor {
    pub sigma_xx: f64,
    pub sigma_yy: f64,
    pub sigma_zz: f64,
    pub tau_xy: f64,
    pub tau_xz: f64,
    pub tau_yz: f64,
}

impl CauchyStressTensor {
    pub fn zero() -> Self {
        Self {
            sigma_xx: 0.0,
            sigma_yy: 0.0,
            sigma_zz: 0.0,
            tau_xy: 0.0,
            tau_xz: 0.0,
            tau_yz: 0.0,
        }
    }

    /// Calculates Von Mises Equivalent Stress (MPa)
    pub fn von_mises(&self) -> f64 {
        let diff_x_y = self.sigma_xx - self.sigma_yy;
        let diff_y_z = self.sigma_yy - self.sigma_zz;
        let diff_z_x = self.sigma_zz - self.sigma_xx;
        let shear_terms = 6.0 * (self.tau_xy * self.tau_xy + self.tau_yz * self.tau_yz + self.tau_xz * self.tau_xz);

        (0.5 * (diff_x_y * diff_x_y + diff_y_z * diff_y_z + diff_z_x * diff_z_x + shear_terms)).sqrt()
    }
}

/// Dynamic State of an Advanced Engineered Material Sample
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterialSampleState {
    pub density_kg_m3: f64,
    pub yield_strength_mpa: f64,
    pub ultimate_tensile_strength_mpa: f64,
    pub applied_load_kn: f64,
    pub stress_tensor: CauchyStressTensor,
    pub von_mises_stress_mpa: f64,
    pub safety_margin: f64,
    pub eigenvector_alignment_score: f64, // 0.0 (misaligned) to 1.0 (perfectly stress-aligned)
    pub is_yield_failed: bool,
}

/// Parameters for Material Inverse Design
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterialInverseParams {
    pub target_safety_margin: f64,
    pub temperature_c: f64,
    pub thermal_expansion_alpha: f64,
}

impl Default for MaterialInverseParams {
    fn default() -> Self {
        Self {
            target_safety_margin: 1.5,
            temperature_c: 25.0,
            thermal_expansion_alpha: 1.2e-5, // ~12 ppm/K
        }
    }
}

impl MaterialSampleState {
    pub fn new(density: f64, base_yield_mpa: f64, load_kn: f64, alignment: f64) -> Self {
        Self {
            density_kg_m3: density,
            yield_strength_mpa: base_yield_mpa,
            ultimate_tensile_strength_mpa: base_yield_mpa * 1.4,
            applied_load_kn: load_kn,
            stress_tensor: CauchyStressTensor::zero(),
            von_mises_stress_mpa: 0.0,
            safety_margin: 0.0,
            eigenvector_alignment_score: alignment,
            is_yield_failed: false,
        }
    }

    /// Advances Multi-Scale Material Stress Physics by dt (seconds)
    pub fn step(&mut self, params: &MaterialInverseParams, _dt_sec: f64) {
        // Cross-sectional area scaled by material density (lightweight topology optimization)
        let area_m2 = (self.density_kg_m3 / 2700.0) * 0.005; // reference aluminum density
        let axial_stress_mpa = (self.applied_load_kn * 1000.0 / area_m2) / 1e6;

        // Shear stress reduced when material topology is aligned with principal stress eigenvectors
        let alignment_efficiency = 1.0 - (0.65 * self.eigenvector_alignment_score);
        let shear_stress_mpa = axial_stress_mpa * 0.45 * alignment_efficiency;

        // Thermal stress contribution
        let thermal_stress_mpa = params.thermal_expansion_alpha * params.temperature_c * 70_000.0; // E = 70 GPa

        self.stress_tensor = CauchyStressTensor {
            sigma_xx: axial_stress_mpa + thermal_stress_mpa,
            sigma_yy: axial_stress_mpa * 0.2,
            sigma_zz: axial_stress_mpa * 0.1,
            tau_xy: shear_stress_mpa,
            tau_xz: shear_stress_mpa * 0.5,
            tau_yz: 0.0,
        };

        self.von_mises_stress_mpa = self.stress_tensor.von_mises();
        self.safety_margin = (self.yield_strength_mpa / self.von_mises_stress_mpa) - 1.0;

        if self.von_mises_stress_mpa > self.yield_strength_mpa {
            self.is_yield_failed = true;
        }
    }
}
