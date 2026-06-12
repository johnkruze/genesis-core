//! High-frequency Tactile Slip Observer & Reflex Controller (ZTP-TSA)
//! Designed for embeddable real-time microcontrollers (no_std compatible).

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
        
        // Hard-coded safety limit: never exceed 45.0 Newtons (prevents crushing the payload)
        target_force = target_force.min(45.0f32);
        state.normal_force = target_force;
        
        // If the margin recovers and slip halts, disengage reflex
        if margin > 0.25f32 && !micro_slip_detected && !macro_slip_detected && !rotational_slip_detected {
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
    // 1. Force Clamping based on tissue classification
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

    // 2. Viscoelastic Rupture Detection (Stiffness drop during active displacement)
    let dx = auditor.measured_displacement_m - auditor.last_displacement_m;
    let df = auditor.measured_force_n - auditor.last_force_n;

    // If active displacement is positive (compressing) and force drops significantly, it's a rupture
    let viscoelastic_rupture_detected = dx > 0.0001f32 && df < -0.02f32;

    // 3. Cable Slip / Tension Fault
    // Jaws are open/stretched but force is extremely low (cable broke or slipped off pulley)
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
    // 1. Release Stiction detection (capillary forces keeping the part attached to gripper jaw)
    let release_stiction_active = auditor.jaw_separation_um > 10.0f32 && auditor.pull_off_force_un > 5.0f32;

    // 2. Electrostatic charge violation (danger of ESD or static attraction)
    let electrostatic_charge_violation = auditor.dynamic_electrostatic_charge_v > 150.0f32;

    // 3. Piezo shake trigger (active high-frequency vibrate to break stiction bridge)
    let piezo_shake_trigger = release_stiction_active;

    // 4. Safe to Retract
    let safe_to_retract = !release_stiction_active && !electrostatic_charge_violation;

    C_MicroResult {
        release_stiction_active,
        electrostatic_charge_violation,
        piezo_shake_trigger,
        safe_to_retract,
    }
}
