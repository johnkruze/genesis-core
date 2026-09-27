//! Directed Energy Laser Targeting Coherence Auditor & Gimbal Jitter Projector
//! Core physics and state estimation (Kalman filter) solver.

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct C_LaserTargetState {
    pub true_y: f64,
    pub true_vy: f64,
    pub est_y: f64,
    pub est_vy: f64,
    pub p_xx: f64,
    pub p_xv: f64,
    pub p_vv: f64,
    pub gimbal_y: f64,
    pub gimbal_vy: f64,
    pub anomaly_detected: bool,
}

pub fn step_directed_energy(
    state: &mut C_LaserTargetState,
    y_meas: f64,
    dy_history: &[f64],
    apply_ztp: bool,
    dt: f64,
) -> bool {
    // 1. Advance true physical kinematics
    state.true_y += state.true_vy * dt;

    // 2. Kalman filter prediction step
    let y_pred = state.est_y + state.est_vy * dt;
    let vy_pred = state.est_vy;

    // Process noise covariance (Q)
    let q_xx = 1e-6;
    let q_vv = 1e-6;

    let p_xx_pred = state.p_xx + 2.0 * dt * state.p_xv + dt * dt * state.p_vv + q_xx;
    let p_xv_pred = state.p_xv + dt * state.p_vv;
    let p_vv_pred = state.p_vv + q_vv;

    // 3. ZTP Anomaly Check
    let innovation = y_meas - y_pred;
    let r_nominal = 0.0004; // 2cm noise variance (0.02^2)
    let innov_variance = p_xx_pred + r_nominal;
    let innov_std = innov_variance.sqrt();
    
    let is_anomaly = innovation.abs() > 4.0 * innov_std;

    // Detrended rolling variance check
    let mut is_sensor_clean = false;
    if dy_history.len() >= 10 {
        let n = dy_history.len() as f64;
        let sum: f64 = dy_history.iter().sum();
        let mean = sum / n;
        let variance: f64 = dy_history.iter().map(|&x| (x - mean).powi(2)).sum::<f64>() / n;
        if variance < 0.1 {
            is_sensor_clean = true;
        }
    }

    let mut r = r_nominal;
    let mut anomaly_active = false;

    if apply_ztp {
        if is_anomaly && !is_sensor_clean {
            r = 200.0; // Dilate measurement noise covariance to ignore sensor input
            anomaly_active = true;
        }
    }

    state.anomaly_detected = anomaly_active;

    // 4. Kalman filter update step
    let s_val = p_xx_pred + r;
    let k_x = p_xx_pred / s_val;
    let k_v = p_xv_pred / s_val;

    state.est_y = y_pred + k_x * innovation;
    state.est_vy = vy_pred + k_v * innovation;

    state.p_xx = (1.0 - k_x) * p_xx_pred;
    state.p_xv = (1.0 - k_x) * p_xv_pred;
    state.p_vv = p_vv_pred - k_v * p_xv_pred;

    // 5. Gimbal servo loop update
    let kp = 1000.0;
    let kd = 60.0;
    
    let err_y = state.est_y - state.gimbal_y;
    let err_vy = state.est_vy - state.gimbal_vy;
    
    let mut u_gimbal = kp * err_y + kd * err_vy;
    
    // Gimbal acceleration/torque physical limit
    u_gimbal = u_gimbal.clamp(-5000.0, 5000.0);

    state.gimbal_vy += u_gimbal * dt;
    state.gimbal_y += state.gimbal_vy * dt;

    anomaly_active
}
