//! High-frequency Tactile Slip Observer & Reflex Controller (ZTP-TSA)
//! Designed for embeddable real-time microcontrollers (no_std compatible).

pub const GRASP_CLAMP_N: f32 = 45.0;
pub const N_FINGER_PATCHES: usize = 5;

/// Serial finger (thumb + four) that places FingerPatches into the same cone.
pub const N_HAND_FINGERS: usize = N_FINGER_PATCHES;
pub const PHALANGE_PROXIMAL_M: f32 = 0.045;
pub const PHALANGE_INTERMEDIATE_M: f32 = 0.028;
pub const PHALANGE_DISTAL_M: f32 = 0.018;
pub const TENDON_STIFFNESS_N_PER_M: f32 = 1800.0;
pub const TENDON_REST_LENGTH_M: f32 = 0.118;
pub const TENDON_STRAIN_WARN: f32 = 0.055;
pub const TENDON_MOMENT_ARM_M: f32 = 0.0075;
pub const THUMB_OPPOSITION_RAD: f32 = 1.047;
pub const HAND_JOINT_KP: f32 = 0.42;
pub const HAND_JOINT_KD: f32 = 0.012;
pub const HAND_JOINT_INERTIA: f32 = 6.5e-5;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Taxel {
    pub normal: f32,  // normal force (N)
    pub shear_x: f32, // shear force in X direction (N)
    pub shear_y: f32, // shear force in Y direction (N)
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct C_TactileArray {
    pub taxels: [Taxel; 16], // 4x4 flat array of contact taxels
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FingerPatch {
    pub array: C_TactileArray,
    pub com_offset_m: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct C_GraspState {
    pub normal_force: f32,           // current normal force (N)
    pub slip_velocity: f32,          // macro slip velocity (m/s)
    pub slip_angular_velocity: f32,  // rotational slip velocity (rad/s)
    pub object_mass: f32,            // estimated mass of gripped object (kg)
    pub static_friction_coeff: f32,  // static friction coefficient (mu_s)
    pub dynamic_friction_coeff: f32, // dynamic friction coefficient (mu_d)
    pub reflex_active: bool,         // whether safety reflex is currently active
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct C_GraspResult {
    pub micro_slip_detected: bool,
    pub macro_slip_detected: bool,
    pub rotational_slip_detected: bool,
    pub commanded_force: f32,
    pub margin: f32, // friction margin index (0.0 = slipping, 1.0 = highly secure)
    pub estimated_mu: f32, // dynamically estimated friction coefficient
}

/// Wrist gravity torque from a COM offset the grasp did not model. \tau = m g n e.
#[inline]
pub fn wrist_offset_torque_nm(mass_kg: f64, load_factor: f64, com_offset_m: f64) -> f64 {
    mass_kg.max(0.0) * 9.81 * load_factor.max(0.0) * com_offset_m.abs()
}

/// Helper function to check if a 4x4 flat index belongs to the outer border ring
#[inline]
fn is_outer_border(index: usize) -> bool {
    let row = index / 4;
    let col = index % 4;
    row == 0 || row == 3 || col == 0 || col == 3
}

/// Evaluates tactile matrices to detect localized micro-slip, rotational slip, 
/// estimates friction, and adjusts grip force.
/// Implements a 1000Hz reactive control loop.
pub fn evaluate_grasp_dynamics(
    sensor: &C_TactileArray,
    state: &mut C_GraspState,
    dt: f32,
) -> C_GraspResult {
    let mut outer_slip_count = 0;
    let mut inner_slip_count = 0;
    let mut total_normal = 0.0f32;
    let mut total_shear_x = 0.0f32;
    let mut total_shear_y = 0.0f32;
    
    // Accumulators for adaptive friction estimation
    let mut slipping_taxels_count = 0;
    let mut accumulated_mu_est = 0.0f32;

    // Coordinate mapping for rotational torque moment
    let mut total_mz = 0.0f32;

    let mu_s = state.static_friction_coeff;

    for i in 0..16 {
        let taxel = sensor.taxels[i];
        total_normal += taxel.normal;
        total_shear_x += taxel.shear_x;
        total_shear_y += taxel.shear_y;

        // Vector magnitude of local shear force
        let shear_mag = (taxel.shear_x * taxel.shear_x + taxel.shear_y * taxel.shear_y).sqrt();

        // Friction cone threshold check: if shear exceeds maximum static friction
        let local_slipping = taxel.normal > 0.0f32 && shear_mag > (mu_s * taxel.normal);

        if local_slipping {
            if is_outer_border(i) {
                outer_slip_count += 1;
            } else {
                inner_slip_count += 1;
            }
            
            // Dynamic friction estimator: ratio of shear to normal force at slip interface
            slipping_taxels_count += 1;
            accumulated_mu_est += shear_mag / taxel.normal;
        }

        // Torsional torque moment: M_z = dx * F_y - dy * F_x
        // Center is at (1.5, 1.5)
        let row = (i / 4) as f32;
        let col = (i % 4) as f32;
        let dx = col - 1.5f32;
        let dy = row - 1.5f32;
        total_mz += dx * taxel.shear_y - dy * taxel.shear_x;
    }

    // Update friction coefficient dynamically if slipping occurs
    let mut estimated_mu = mu_s;
    if slipping_taxels_count > 0 {
        let avg_measured_mu = accumulated_mu_est / (slipping_taxels_count as f32);
        
        // Low-pass blend factor (alpha = 0.05) to filter high-frequency sensor noise
        let alpha = 0.05f32;
        let new_mu_s = mu_s * (1.0 - alpha) + avg_measured_mu * alpha;
        
        // Dynamic friction scales proportionally
        state.static_friction_coeff = new_mu_s.clamp(0.05f32, 1.5f32);
        state.dynamic_friction_coeff = (state.static_friction_coeff * 0.8f32).clamp(0.04f32, 1.2f32);
        estimated_mu = state.static_friction_coeff;
    }

    let total_shear_mag = (total_shear_x * total_shear_x + total_shear_y * total_shear_y).sqrt();
    let friction_limit = total_normal * state.static_friction_coeff;

    // Macro-slip definition: Inner core slips or object has linear velocity
    let macro_slip_detected = inner_slip_count > 0 || state.slip_velocity.abs() > 0.005f32;
    
    // Micro-slip definition: Boundary slips while core is stuck, or shear force is within 10% of limit
    let micro_slip_detected = (outer_slip_count > 2 && inner_slip_count == 0)
        || (total_shear_mag > friction_limit * 0.90f32 && !macro_slip_detected);

    // Rotational slip definition: Significant net torsional moment while slipping or angular velocity detected
    let rotational_slip_detected = (total_mz.abs() > friction_limit * 0.15f32 && outer_slip_count > 2)
        || state.slip_angular_velocity.abs() > 0.1f32;

    let margin = if friction_limit > 0.0f32 {
        ((friction_limit - total_shear_mag) / friction_limit).clamp(0.0f32, 1.0f32)
    } else {
        0.0f32
    };

    // Integrate linear slip velocity from shear excess (m/s) — sealed product column source
    let mass = state.object_mass.max(0.01f32);
    if macro_slip_detected {
        let excess_shear = (total_shear_mag - friction_limit).max(0.0f32);
        // Accel ≈ excess_force / mass; scale for taxel aggregate → object frame
        let a = (excess_shear * 0.15f32) / mass;
        state.slip_velocity = (state.slip_velocity + a * dt).clamp(0.0f32, 0.85f32);
    } else if micro_slip_detected {
        let a = 0.08f32 * (1.0f32 - margin) / mass.sqrt();
        state.slip_velocity = (state.slip_velocity + a * dt).clamp(0.0f32, 0.35f32);
    } else {
        // Stick recovery — exponential decay toward zero
        state.slip_velocity *= (-8.0f32 * dt).exp();
        if state.slip_velocity.abs() < 1e-4 {
            state.slip_velocity = 0.0;
        }
    }
    if rotational_slip_detected {
        state.slip_angular_velocity =
            (state.slip_angular_velocity + 12.0f32 * (1.0f32 - margin) * dt).clamp(0.0f32, 4.0f32);
    } else {
        state.slip_angular_velocity *= (-6.0f32 * dt).exp();
    }

    // Grasp reflex logic:
    // If micro-slip, macro-slip, or rotational slip is active, trigger an immediate proportional force correction.
    let mut target_force = state.normal_force;
    if micro_slip_detected || macro_slip_detected || rotational_slip_detected || state.reflex_active {
        state.reflex_active = true;
        
        // Ramps force up rapidly to prevent drops, scaling with the level of slip detected
        let scale = if macro_slip_detected { 
            650.0f32 
        } else if rotational_slip_detected {
            450.0f32
        } else { 
            280.0f32 
        };
        target_force += scale * dt;
        
        // Hard-coded safety limit: never exceed GRASP_CLAMP_N Newtons (prevents crushing the payload)
        target_force = target_force.min(GRASP_CLAMP_N);
        state.normal_force = target_force;
        
        // If the margin recovers and slip is damped, disengage reflex
        if margin > 0.25f32
            && !micro_slip_detected
            && !macro_slip_detected
            && !rotational_slip_detected
            && state.slip_velocity < 0.01f32
        {
            state.reflex_active = false;
        }
    }

    C_GraspResult {
        micro_slip_detected,
        macro_slip_detected,
        rotational_slip_detected,
        commanded_force: state.normal_force,
        margin,
        estimated_mu,
    }
}

/// Evaluates multi-patch fingertip grasp (thumb + N fingers) over the shared friction cone.
pub fn evaluate_multi_patch_grasp(
    patches: &[FingerPatch],
    state: &mut C_GraspState,
    dt: f32,
) -> C_GraspResult {
    let mut micro_any = false;
    let mut macro_any = false;
    let mut rot_any = false;
    let mut min_margin = 1.0f32;
    let mut sum_mu = 0.0f32;

    let n = patches.len().max(1) as f32;
    let dt_share = dt / n;
    for patch in patches {
        let extra_shear = state.object_mass * 9.81 * patch.com_offset_m[0].abs() / 16.0;
        let mut array = patch.array;
        if extra_shear > 0.0 {
            for taxel in &mut array.taxels {
                taxel.shear_x += extra_shear;
            }
        }
        let res = evaluate_grasp_dynamics(&array, state, dt_share);
        if res.micro_slip_detected {
            micro_any = true;
        }
        if res.macro_slip_detected {
            macro_any = true;
        }
        if res.rotational_slip_detected {
            rot_any = true;
        }
        if res.margin < min_margin {
            min_margin = res.margin;
        }
        sum_mu += res.estimated_mu;
    }

    let avg_mu = if patches.is_empty() {
        state.static_friction_coeff
    } else {
        sum_mu / patches.len() as f32
    };

    C_GraspResult {
        micro_slip_detected: micro_any,
        macro_slip_detected: macro_any,
        rotational_slip_detected: rot_any,
        commanded_force: state.normal_force,
        margin: min_margin,
        estimated_mu: avg_mu,
    }
}

/// Serial-chain hand. Tendon places the five pads; skin is evaluate_multi_patch_grasp.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct C_HandTendonState {
    pub q_mcp: [f32; N_HAND_FINGERS],
    pub q_pip: [f32; N_HAND_FINGERS],
    pub q_dip: [f32; N_HAND_FINGERS],
    pub qdot_mcp: [f32; N_HAND_FINGERS],
    pub qdot_pip: [f32; N_HAND_FINGERS],
    pub qdot_dip: [f32; N_HAND_FINGERS],
    pub tendon_stretch_m: f32,
    pub tendon_tension_n: f32,
    pub opposition_rad: f32,
    pub object_span_m: f32,
    pub commanded_close_rad: f32,
    pub pad_normal_n: f32,
    pub normal_force: f32,
    pub slip_velocity: f32,
    pub slip_angular_velocity: f32,
    pub object_mass: f32,
    pub static_friction_coeff: f32,
    pub dynamic_friction_coeff: f32,
    pub reflex_active: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct C_HandTendonResult {
    pub tendon_overstretch: bool,
    pub pad_slip: bool,
    pub commanded_force: f32,
    pub margin: f32,
    pub tendon_tension_n: f32,
    pub pad_normal_n: f32,
    pub stretch_m: f32,
    pub strain: f32,
}

#[inline]
fn hand_chain_length_m() -> f32 {
    PHALANGE_PROXIMAL_M + PHALANGE_INTERMEDIATE_M + PHALANGE_DISTAL_M
}

/// Contact flexion sum (rad): a large span stops the chain early.
#[inline]
pub fn hand_contact_q_sum_max(object_span_m: f32) -> f32 {
    let span_ratio = (object_span_m / hand_chain_length_m()).clamp(0.08, 1.0);
    (1.0 - span_ratio) * 2.55 + 0.22
}

#[inline]
fn pad_com_offset_m(finger: usize, q_mcp: f32, q_pip: f32, q_dip: f32, opposition_rad: f32) -> [f32; 3] {
    let s1 = q_mcp.sin();
    let s2 = (q_mcp + q_pip).sin();
    let s3 = (q_mcp + q_pip + q_dip).sin();
    let reach =
        PHALANGE_PROXIMAL_M * s1 + PHALANGE_INTERMEDIATE_M * s2 + PHALANGE_DISTAL_M * s3;
    let lateral = if finger == 0 {
        -0.032 * opposition_rad.sin()
    } else {
        (finger as f32 - 2.5) * 0.018
    };
    [lateral, 0.0, reach * 0.25]
}

/// 1 kHz tendon + PD finger step. Pads share the gripper friction cone.
pub fn evaluate_hand_tendon_dynamics(state: &mut C_HandTendonState, dt: f32) -> C_HandTendonResult {
    let dt = dt.max(1e-6);
    let q_sum_max = hand_contact_q_sum_max(state.object_span_m);
    let pinch = state.opposition_rad.sin().clamp(0.18, 1.0);

    let q_des_sum = state.commanded_close_rad * 2.18;
    let excess_q = (q_des_sum - q_sum_max).max(0.0);
    let blocked_stretch = TENDON_MOMENT_ARM_M * excess_q;
    let working_n = (state.commanded_close_rad / 1.40 * GRASP_CLAMP_N * pinch).clamp(0.0, GRASP_CLAMP_N);
    let working_stretch = working_n / TENDON_STIFFNESS_N_PER_M.max(1.0);
    let stretch = working_stretch + blocked_stretch;
    state.tendon_stretch_m = stretch;
    state.tendon_tension_n = (TENDON_STIFFNESS_N_PER_M * stretch).max(0.0);
    let strain = stretch / TENDON_REST_LENGTH_M;
    let tendon_overstretch = blocked_stretch / TENDON_REST_LENGTH_M > TENDON_STRAIN_WARN;

    let t_tau = state.tendon_tension_n * TENDON_MOMENT_ARM_M;
    for f in 0..N_HAND_FINGERS {
        let close_scale = if f == 0 {
            0.82
        } else {
            1.0 - 0.03 * f as f32
        };
        let q_des_mcp = state.commanded_close_rad * close_scale
            + if f == 0 {
                0.20 * state.opposition_rad
            } else {
                0.0
            };
        let q_des_pip = state.commanded_close_rad * 0.70 * close_scale;
        let q_des_dip = state.commanded_close_rad * 0.48 * close_scale;

        let tau_mcp =
            t_tau + HAND_JOINT_KP * (q_des_mcp - state.q_mcp[f]) - HAND_JOINT_KD * state.qdot_mcp[f];
        let tau_pip =
            t_tau + HAND_JOINT_KP * (q_des_pip - state.q_pip[f]) - HAND_JOINT_KD * state.qdot_pip[f];
        let tau_dip =
            t_tau + HAND_JOINT_KP * (q_des_dip - state.q_dip[f]) - HAND_JOINT_KD * state.qdot_dip[f];

        state.qdot_mcp[f] += (tau_mcp / HAND_JOINT_INERTIA) * dt;
        state.qdot_pip[f] += (tau_pip / HAND_JOINT_INERTIA) * dt;
        state.qdot_dip[f] += (tau_dip / HAND_JOINT_INERTIA) * dt;
        state.q_mcp[f] += state.qdot_mcp[f] * dt;
        state.q_pip[f] += state.qdot_pip[f] * dt;
        state.q_dip[f] += state.qdot_dip[f] * dt;

        let q_sum = state.q_mcp[f] + state.q_pip[f] + state.q_dip[f];
        if q_sum > q_sum_max {
            let scale = q_sum_max / q_sum.max(1e-6);
            state.q_mcp[f] *= scale;
            state.q_pip[f] *= scale;
            state.q_dip[f] *= scale;
            state.qdot_mcp[f] = state.qdot_mcp[f].min(0.0);
            state.qdot_pip[f] = state.qdot_pip[f].min(0.0);
            state.qdot_dip[f] = state.qdot_dip[f].min(0.0);
        }
        state.q_mcp[f] = state.q_mcp[f].clamp(0.0, 1.6);
        state.q_pip[f] = state.q_pip[f].clamp(0.0, 1.8);
        state.q_dip[f] = state.q_dip[f].clamp(0.0, 1.4);
        state.qdot_mcp[f] = state.qdot_mcp[f].clamp(-25.0, 25.0);
        state.qdot_pip[f] = state.qdot_pip[f].clamp(-25.0, 25.0);
        state.qdot_dip[f] = state.qdot_dip[f].clamp(-25.0, 25.0);
    }

    let pad_normal = working_n;
    state.pad_normal_n = pad_normal;
    state.normal_force = pad_normal;

    let n_taxel = pad_normal / (16.0 * N_FINGER_PATCHES as f32);
    let shear_load = (state.object_mass * 9.81) / (16.0 * N_FINGER_PATCHES as f32);
    let mut patches = [FingerPatch {
        array: C_TactileArray {
            taxels: [Taxel {
                normal: n_taxel,
                shear_x: 0.0,
                shear_y: 0.0,
            }; 16],
        },
        com_offset_m: [0.0, 0.0, 0.0],
    }; N_FINGER_PATCHES];
    for p in 0..N_FINGER_PATCHES {
        patches[p].com_offset_m = pad_com_offset_m(
            p,
            state.q_mcp[p],
            state.q_pip[p],
            state.q_dip[p],
            state.opposition_rad,
        );
        for i in 0..16 {
            patches[p].array.taxels[i].shear_x =
                shear_load * (1.0 + 0.08 * (i as f32 % 4.0) + 0.04 * p as f32);
            patches[p].array.taxels[i].shear_y = shear_load * 0.12 * (i as f32 / 4.0);
        }
    }

    let mut grasp = C_GraspState {
        normal_force: state.normal_force,
        slip_velocity: state.slip_velocity,
        slip_angular_velocity: state.slip_angular_velocity,
        object_mass: state.object_mass,
        static_friction_coeff: state.static_friction_coeff,
        dynamic_friction_coeff: state.dynamic_friction_coeff,
        reflex_active: state.reflex_active,
    };
    let slip_before = grasp.slip_velocity;
    let res = evaluate_multi_patch_grasp(&patches, &mut grasp, dt);
    if res.macro_slip_detected {
        grasp.slip_velocity = (slip_before + (1.0 - res.margin) * 6.0 * dt).clamp(0.0, 0.85);
    } else {
        let decayed = slip_before * (-8.0 * dt).exp();
        grasp.slip_velocity = if decayed.abs() < 1e-4 { 0.0 } else { decayed };
    }
    grasp.normal_force = grasp.normal_force.min(GRASP_CLAMP_N);

    state.normal_force = grasp.normal_force;
    state.slip_velocity = grasp.slip_velocity;
    state.slip_angular_velocity = grasp.slip_angular_velocity;
    state.static_friction_coeff = grasp.static_friction_coeff;
    state.dynamic_friction_coeff = grasp.dynamic_friction_coeff;
    state.reflex_active = grasp.reflex_active;

    let pad_slip = res.macro_slip_detected || state.slip_velocity.abs() > 0.005;

    C_HandTendonResult {
        tendon_overstretch,
        pad_slip,
        commanded_force: state.normal_force,
        margin: res.margin,
        tendon_tension_n: state.tendon_tension_n,
        pad_normal_n: state.pad_normal_n,
        stretch_m: state.tendon_stretch_m,
        strain,
    }
}

// ─── SURGICAL & MICRO-MANUFACTURING EXTENSIONS ───

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct C_SurgicalTissueAuditor {
    pub tissue_type_id: u32,
    pub max_tearing_force_n: f32,
    pub measured_displacement_m: f32,
    pub measured_force_n: f32,
    pub relaxation_tau: f32,
    pub last_displacement_m: f32,
    pub last_force_n: f32,
    pub accumulated_energy_j: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct C_SurgicalResult {
    pub tissue_overstress_detected: bool,
    pub viscoelastic_rupture_detected: bool,
    pub cable_slip_fault: bool,
    pub clamped_force: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct C_MicroReleaseAuditor {
    pub part_mass_micrograms: f32,
    pub pull_off_force_un: f32,
    pub jaw_separation_um: f32,
    pub dynamic_electrostatic_charge_v: f32,
    pub last_jaw_separation_um: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct C_MicroResult {
    pub release_stiction_active: bool,
    pub electrostatic_charge_violation: bool,
    pub piezo_shake_trigger: bool,
    pub safe_to_retract: bool,
}

pub fn evaluate_surgical_grasp_dynamics(
    auditor: &C_SurgicalTissueAuditor,
    _dt: f32,
) -> C_SurgicalResult {
    let tissue_limit = match auditor.tissue_type_id {
        0 => 1.2f32,  // Liver / Spleen
        1 => 2.5f32,  // Bowel / Vessel
        2 => 40.0f32, // Bone / Tendon
        _ => 1.0f32,  // Safe default
    };

    let clamped_force = if auditor.max_tearing_force_n > 0.0f32 {
        tissue_limit.min(auditor.max_tearing_force_n)
    } else {
        tissue_limit
    };

    let tissue_overstress_detected = auditor.measured_force_n > clamped_force;
    let dx = auditor.measured_displacement_m - auditor.last_displacement_m;
    let df = auditor.measured_force_n - auditor.last_force_n;
    let viscoelastic_rupture_detected = dx > 0.0001f32 && df < -0.02f32;
    let cable_slip_fault = auditor.measured_displacement_m > 0.012f32 && auditor.measured_force_n < 0.05f32;

    C_SurgicalResult {
        tissue_overstress_detected,
        viscoelastic_rupture_detected,
        cable_slip_fault,
        clamped_force,
    }
}

pub fn evaluate_micro_release_dynamics(
    auditor: &C_MicroReleaseAuditor,
    _dt: f32,
) -> C_MicroResult {
    let release_stiction_active = auditor.jaw_separation_um > 10.0f32 && auditor.pull_off_force_un > 5.0f32;
    let electrostatic_charge_violation = auditor.dynamic_electrostatic_charge_v > 150.0f32;
    let piezo_shake_trigger = release_stiction_active;
    let safe_to_retract = !release_stiction_active && !electrostatic_charge_violation;

    C_MicroResult {
        release_stiction_active,
        electrostatic_charge_violation,
        piezo_shake_trigger,
        safe_to_retract,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrist_offset_is_mge() {
        let t = wrist_offset_torque_nm(10.0, 1.2, 0.20);
        assert!((t - 23.544).abs() < 1e-6);
    }

    #[test]
    fn test_multi_patch_grasp() {
        let taxels = [Taxel { normal: 2.0, shear_x: 0.1, shear_y: 0.1 }; 16];
        let patch = FingerPatch {
            array: C_TactileArray { taxels },
            com_offset_m: [0.0, 0.0, 0.0],
        };
        let patches = [patch; N_FINGER_PATCHES];
        let mut state = C_GraspState {
            normal_force: 32.0,
            slip_velocity: 0.0,
            slip_angular_velocity: 0.0,
            object_mass: 0.5,
            static_friction_coeff: 0.5,
            dynamic_friction_coeff: 0.4,
            reflex_active: false,
        };
        let res = evaluate_multi_patch_grasp(&patches, &mut state, 0.001);
        assert_eq!(res.commanded_force, 32.0);
        assert!(res.margin > 0.0);
    }

    #[test]
    fn com_offset_adds_patch_shear() {
        let taxels = [Taxel { normal: 2.0, shear_x: 0.1, shear_y: 0.1 }; 16];
        let centered = FingerPatch {
            array: C_TactileArray { taxels },
            com_offset_m: [0.0, 0.0, 0.0],
        };
        let offset = FingerPatch {
            array: C_TactileArray { taxels },
            com_offset_m: [0.20, 0.0, 0.0],
        };
        let mut state_c = C_GraspState {
            normal_force: 8.0,
            slip_velocity: 0.0,
            slip_angular_velocity: 0.0,
            object_mass: 2.0,
            static_friction_coeff: 0.35,
            dynamic_friction_coeff: 0.28,
            reflex_active: false,
        };
        let mut state_o = state_c;
        let res_c = evaluate_multi_patch_grasp(&[centered], &mut state_c, 0.001);
        let res_o = evaluate_multi_patch_grasp(&[offset], &mut state_o, 0.001);
        assert!(res_o.margin < res_c.margin);
    }

    fn blank_hand() -> C_HandTendonState {
        C_HandTendonState {
            q_mcp: [0.05; N_HAND_FINGERS],
            q_pip: [0.04; N_HAND_FINGERS],
            q_dip: [0.03; N_HAND_FINGERS],
            qdot_mcp: [0.0; N_HAND_FINGERS],
            qdot_pip: [0.0; N_HAND_FINGERS],
            qdot_dip: [0.0; N_HAND_FINGERS],
            tendon_stretch_m: 0.0,
            tendon_tension_n: 0.0,
            opposition_rad: THUMB_OPPOSITION_RAD,
            object_span_m: 0.04,
            commanded_close_rad: 0.8,
            pad_normal_n: 0.0,
            normal_force: 0.0,
            slip_velocity: 0.0,
            slip_angular_velocity: 0.0,
            object_mass: 0.4,
            static_friction_coeff: 0.55,
            dynamic_friction_coeff: 0.44,
            reflex_active: false,
        }
    }

    #[test]
    fn tendon_overstretch_without_requiring_slip() {
        let mut state = blank_hand();
        state.object_span_m = 0.072;
        state.commanded_close_rad = 1.45;
        state.object_mass = 0.12;
        state.static_friction_coeff = 0.85;
        state.dynamic_friction_coeff = 0.68;
        let mut over = false;
        let mut slip = false;
        for _ in 0..100 {
            let r = evaluate_hand_tendon_dynamics(&mut state, 0.001);
            over |= r.tendon_overstretch;
            slip |= r.pad_slip;
        }
        assert!(over, "large span + high close must stretch the tendon");
        assert!(!slip, "light object + high µ must hold the pad");
    }

    #[test]
    fn pad_slip_without_requiring_overstretch() {
        let mut state = blank_hand();
        state.object_span_m = 0.022;
        state.commanded_close_rad = 0.42;
        state.object_mass = 2.4;
        state.static_friction_coeff = 0.11;
        state.dynamic_friction_coeff = 0.09;
        let mut over = false;
        let mut slip = false;
        for _ in 0..100 {
            let r = evaluate_hand_tendon_dynamics(&mut state, 0.001);
            over |= r.tendon_overstretch;
            slip |= r.pad_slip;
        }
        assert!(slip, "heavy + low µ + light close must slip the pad");
        assert!(!over, "small span must not overstretch the tendon");
    }

    #[test]
    fn opposition_raises_pad_normal() {
        let mut open = blank_hand();
        open.opposition_rad = 0.35;
        open.commanded_close_rad = 1.0;
        open.object_span_m = 0.055;
        let mut pinch = open;
        pinch.opposition_rad = THUMB_OPPOSITION_RAD;
        let mut n_open = 0.0;
        let mut n_pinch = 0.0;
        for _ in 0..40 {
            n_open = evaluate_hand_tendon_dynamics(&mut open, 0.001).pad_normal_n;
            n_pinch = evaluate_hand_tendon_dynamics(&mut pinch, 0.001).pad_normal_n;
        }
        assert!(n_pinch > n_open);
    }
}
