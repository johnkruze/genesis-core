//! G^G Genesis Core: Advanced Materials Inverse Physics & Stress Tensor Engine
//! Models multi-scale Cauchy stress tensors (sigma_ij), Von Mises yield criteria,
//! crystallographic grain boundary shear, and principal stress eigenvector alignment.

use serde::{Deserialize, Serialize};

/// Named stack densities (g/cc). Living-CAD / forge gates.
pub const CC_DENSITY_GCC: f64 = 1.80;
pub const SUPERCARBON_DENSITY_GCC: f64 = 2.00;

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

    /// Closed-form 3D Cauchy Stress Tensor Eigensolver (Cardano's Method)
    /// Returns (principal_stresses: [f64; 3], eigenvectors: [[f64; 3]; 3])
    /// Sorted descending: lambda_1 >= lambda_2 >= lambda_3
    pub fn solve_principal_eigensystem(&self) -> ([f64; 3], [[f64; 3]; 3]) {
        let q = (self.sigma_xx + self.sigma_yy + self.sigma_zz) / 3.0;

        let b_xx = self.sigma_xx - q;
        let b_yy = self.sigma_yy - q;
        let b_zz = self.sigma_zz - q;

        let p2 = b_xx * b_xx + b_yy * b_yy + b_zz * b_zz
            + 2.0 * (self.tau_xy * self.tau_xy + self.tau_xz * self.tau_xz + self.tau_yz * self.tau_yz);
        let p = (p2 / 6.0).sqrt();

        if p < 1e-12 {
            // Isotropic state - uniform pressure
            return (
                [q, q, q],
                [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            );
        }

        // Normalized deviatoric matrix B / p
        let c_xx = b_xx / p;
        let c_yy = b_yy / p;
        let c_zz = b_zz / p;
        let c_xy = self.tau_xy / p;
        let c_xz = self.tau_xz / p;
        let c_yz = self.tau_yz / p;

        // Half determinant of C
        let det_c_half = 0.5 * (
            c_xx * (c_yy * c_zz - c_yz * c_yz)
          - c_xy * (c_xy * c_zz - c_xz * c_yz)
          + c_xz * (c_xy * c_yz - c_xz * c_yy)
        );

        let r = det_c_half.clamp(-1.0, 1.0);
        let phi = r.acos() / 3.0;

        let l1 = q + 2.0 * p * phi.cos();
        let l3 = q + 2.0 * p * (phi + 2.0 * std::f64::consts::PI / 3.0).cos();
        let l2 = 3.0 * q - l1 - l3; // Trace conservation

        let l1_final = l1.max(l2).max(l3);
        let l3_final = l1.min(l2).min(l3);
        let l2_final = l1 + l2 + l3 - l1_final - l3_final;

        let lambdas = [l1_final, l2_final, l3_final];

        // Compute 3D unit eigenvectors
        let compute_vector_for_lambda = |lam: f64| -> [f64; 3] {
            let m00 = self.sigma_xx - lam;
            let m11 = self.sigma_yy - lam;
            let m22 = self.sigma_zz - lam;
            let m01 = self.tau_xy;
            let m02 = self.tau_xz;
            let m12 = self.tau_yz;

            // Cross products of row pairs from (A - lambda I)
            let c0 = [
                m01 * m12 - m02 * m11,
                m02 * m01 - m00 * m12,
                m00 * m11 - m01 * m01,
            ];
            let c1 = [
                m01 * m22 - m02 * m12,
                m02 * m02 - m00 * m22,
                m00 * m12 - m01 * m02,
            ];
            let c2 = [
                m11 * m22 - m12 * m12,
                m12 * m02 - m01 * m22,
                m01 * m12 - m11 * m02,
            ];

            let n0 = c0[0] * c0[0] + c0[1] * c0[1] + c0[2] * c0[2];
            let n1 = c1[0] * c1[0] + c1[1] * c1[1] + c1[2] * c1[2];
            let n2 = c2[0] * c2[0] + c2[1] * c2[1] + c2[2] * c2[2];

            let max_c = if n0 >= n1 && n0 >= n2 {
                c0
            } else if n1 >= n0 && n1 >= n2 {
                c1
            } else {
                c2
            };

            let norm = (max_c[0] * max_c[0] + max_c[1] * max_c[1] + max_c[2] * max_c[2]).sqrt();
            if norm > 1e-12 {
                [max_c[0] / norm, max_c[1] / norm, max_c[2] / norm]
            } else {
                [1.0, 0.0, 0.0]
            }
        };

        let v1 = compute_vector_for_lambda(lambdas[0]);
        let mut v2 = compute_vector_for_lambda(lambdas[1]);

        // Gram-Schmidt orthogonalization for v1, v2
        let dot1 = v1[0] * v2[0] + v1[1] * v2[1] + v1[2] * v2[2];
        v2 = [v2[0] - dot1 * v1[0], v2[1] - dot1 * v1[1], v2[2] - dot1 * v1[2]];
        let norm_v2 = (v2[0] * v2[0] + v2[1] * v2[1] + v2[2] * v2[2]).sqrt();
        if norm_v2 > 1e-12 {
            v2 = [v2[0] / norm_v2, v2[1] / norm_v2, v2[2] / norm_v2];
        } else {
            // Find orthogonal vector manually if degenerate
            v2 = if v1[0].abs() < 0.8 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
            let dot_sub = v1[0] * v2[0] + v1[1] * v2[1] + v1[2] * v2[2];
            v2 = [v2[0] - dot_sub * v1[0], v2[1] - dot_sub * v1[1], v2[2] - dot_sub * v1[2]];
            let n2_sub = (v2[0] * v2[0] + v2[1] * v2[1] + v2[2] * v2[2]).sqrt();
            v2 = [v2[0] / n2_sub, v2[1] / n2_sub, v2[2] / n2_sub];
        }

        // v3 = v1 x v2
        let mut v3 = [
            v1[1] * v2[2] - v1[2] * v2[1],
            v1[2] * v2[0] - v1[0] * v2[2],
            v1[0] * v2[1] - v1[1] * v2[0],
        ];
        let norm_v3 = (v3[0] * v3[0] + v3[1] * v3[1] + v3[2] * v3[2]).sqrt();
        if norm_v3 > 1e-12 {
            v3 = [v3[0] / norm_v3, v3[1] / norm_v3, v3[2] / norm_v3];
        } else {
            v3 = [0.0, 0.0, 1.0];
        }

        (lambdas, [v1, v2, v3])
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
            tau_yz: shear_stress_mpa * 0.25 * (1.0 - 0.5 * self.eigenvector_alignment_score),
        };

        self.von_mises_stress_mpa = self.stress_tensor.von_mises();
        self.safety_margin = (self.yield_strength_mpa / self.von_mises_stress_mpa) - 1.0;

        if self.von_mises_stress_mpa > self.yield_strength_mpa {
            self.is_yield_failed = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diagonal_3d_eigensolve() {
        let tensor = CauchyStressTensor {
            sigma_xx: 100.0,
            sigma_yy: 50.0,
            sigma_zz: 10.0,
            tau_xy: 0.0,
            tau_xz: 0.0,
            tau_yz: 0.0,
        };
        let (lambdas, vecs) = tensor.solve_principal_eigensystem();
        assert!((lambdas[0] - 100.0).abs() < 1e-5);
        assert!((lambdas[1] - 50.0).abs() < 1e-5);
        assert!((lambdas[2] - 10.0).abs() < 1e-5);
        assert!((vecs[0][0].abs() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_pure_shear_3d_eigensolve() {
        let tensor = CauchyStressTensor {
            sigma_xx: 0.0,
            sigma_yy: 0.0,
            sigma_zz: 0.0,
            tau_xy: 50.0,
            tau_xz: 0.0,
            tau_yz: 0.0,
        };
        let (lambdas, _vecs) = tensor.solve_principal_eigensystem();
        assert!((lambdas[0] - 50.0).abs() < 1e-5);
        assert!((lambdas[1] - 0.0).abs() < 1e-5);
        assert!((lambdas[2] - (-50.0)).abs() < 1e-5);
    }

    #[test]
    fn test_general_full_3d_eigensolve() {
        let tensor = CauchyStressTensor {
            sigma_xx: 120.0,
            sigma_yy: 45.0,
            sigma_zz: -30.0,
            tau_xy: 25.0,
            tau_xz: 15.0,
            tau_yz: 10.0,
        };
        let (lambdas, vecs) = tensor.solve_principal_eigensystem();

        // 1. Descending order
        assert!(lambdas[0] >= lambdas[1]);
        assert!(lambdas[1] >= lambdas[2]);

        // 2. Trace conservation (lambda1 + lambda2 + lambda3 = tr(A))
        let tr_a = tensor.sigma_xx + tensor.sigma_yy + tensor.sigma_zz;
        let tr_lam = lambdas[0] + lambdas[1] + lambdas[2];
        assert!((tr_a - tr_lam).abs() < 1e-8, "Trace not conserved");

        // 3. Eigen-residual check ||A * v_i - lambda_i * v_i|| < 1e-6
        for i in 0..3 {
            let lam = lambdas[i];
            let v = vecs[i];
            let av = [
                tensor.sigma_xx * v[0] + tensor.tau_xy * v[1] + tensor.tau_xz * v[2],
                tensor.tau_xy * v[0] + tensor.sigma_yy * v[1] + tensor.tau_yz * v[2],
                tensor.tau_xz * v[0] + tensor.tau_yz * v[1] + tensor.sigma_zz * v[2],
            ];
            let lv = [lam * v[0], lam * v[1], lam * v[2]];

            let res = ((av[0] - lv[0]).powi(2) + (av[1] - lv[1]).powi(2) + (av[2] - lv[2]).powi(2)).sqrt();
            assert!(res < 1e-6, "Eigen-residual too high for mode {}: {}", i, res);

            // Unit norm
            let norm = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            assert!((norm - 1.0).abs() < 1e-6, "Eigenvector not normalized");
        }

        // 4. Orthogonality check v_i . v_j = 0
        let dot01 = vecs[0][0] * vecs[1][0] + vecs[0][1] * vecs[1][1] + vecs[0][2] * vecs[1][2];
        let dot12 = vecs[1][0] * vecs[2][0] + vecs[1][1] * vecs[2][1] + vecs[1][2] * vecs[2][2];
        let dot20 = vecs[2][0] * vecs[0][0] + vecs[2][1] * vecs[0][1] + vecs[2][2] * vecs[0][2];
        assert!(dot01.abs() < 1e-6, "v0 not orthogonal to v1: {}", dot01);
        assert!(dot12.abs() < 1e-6, "v1 not orthogonal to v2: {}", dot12);
        assert!(dot20.abs() < 1e-6, "v2 not orthogonal to v0: {}", dot20);
    }

    #[test]
    fn test_isotropic_degeneracy_3d_eigensolve() {
        let tensor = CauchyStressTensor {
            sigma_xx: 50.0,
            sigma_yy: 50.0,
            sigma_zz: 50.0,
            tau_xy: 0.0,
            tau_xz: 0.0,
            tau_yz: 0.0,
        };
        let (lambdas, _vecs) = tensor.solve_principal_eigensystem();
        assert!((lambdas[0] - 50.0).abs() < 1e-8);
        assert!((lambdas[1] - 50.0).abs() < 1e-8);
        assert!((lambdas[2] - 50.0).abs() < 1e-8);
    }
}


