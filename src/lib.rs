pub mod physics;
pub mod schema;
pub mod taichi_bridge;
pub mod rng;
pub mod proof;
pub mod output;

#[unsafe(no_mangle)]
pub extern "C" fn ztp_dexterous_evaluate_grasp(
    sensor: *const physics::dexterous::C_TactileArray,
    state: *mut physics::dexterous::C_GraspState,
    dt: f32,
) -> physics::dexterous::C_GraspResult {
    if sensor.is_null() || state.is_null() {
        return physics::dexterous::C_GraspResult {
            micro_slip_detected: false,
            macro_slip_detected: false,
            rotational_slip_detected: false,
            commanded_force: 0.0,
            margin: 0.0,
            estimated_mu: 0.0,
        };
    }
    unsafe {
        physics::dexterous::evaluate_grasp_dynamics(&*sensor, &mut *state, dt)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ztp_directed_energy_step(
    state: *mut physics::directed_energy::C_LaserTargetState,
    y_meas: f64,
    dy_history: *const f64,
    dy_history_len: u32,
    apply_ztp: bool,
    dt: f64,
) -> bool {
    if state.is_null() {
        return false;
    }
    unsafe {
        let history = if dy_history.is_null() || dy_history_len == 0 {
            &[]
        } else {
            std::slice::from_raw_parts(dy_history, dy_history_len as usize)
        };
        physics::directed_energy::step_directed_energy(&mut *state, y_meas, history, apply_ztp, dt)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ztp_josephson_step(
    state: *mut physics::josephson::JosephsonState,
    dt: f64,
    control_current: f64,
    noise_seed: u64,
) -> bool {
    if state.is_null() {
        return false;
    }
    unsafe {
        (*state).step(dt, control_current, noise_seed);
        (*state).quenched
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ztp_surgical_evaluate_grasp(
    auditor: *const physics::dexterous::C_SurgicalTissueAuditor,
    dt: f32,
) -> physics::dexterous::C_SurgicalResult {
    if auditor.is_null() {
        return physics::dexterous::C_SurgicalResult {
            tissue_overstress_detected: false,
            viscoelastic_rupture_detected: false,
            cable_slip_fault: false,
            clamped_force: 0.0,
        };
    }
    unsafe {
        physics::dexterous::evaluate_surgical_grasp_dynamics(&*auditor, dt)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ztp_micro_evaluate_release(
    auditor: *const physics::dexterous::C_MicroReleaseAuditor,
    dt: f32,
) -> physics::dexterous::C_MicroResult {
    if auditor.is_null() {
        return physics::dexterous::C_MicroResult {
            release_stiction_active: false,
            electrostatic_charge_violation: false,
            piezo_shake_trigger: false,
            safe_to_retract: false,
        };
    }
    unsafe {
        physics::dexterous::evaluate_micro_release_dynamics(&*auditor, dt)
    }
}



