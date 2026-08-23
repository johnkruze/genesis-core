// Ephemeris & N-Body Schema: Connecting to JPL DE440/DE441 standard

use std::collections::HashMap;

/// Standard definition for a celestial body's ephemeris retrieval
pub trait Ephemeris {
    fn get_position(&self, body_id: &str, time_jd: f64) -> Option<[f64; 3]>;
    fn get_velocity(&self, body_id: &str, time_jd: f64) -> Option<[f64; 3]>;
    fn get_mu(&self, body_id: &str) -> Option<f64>; // Standard gravitational parameter (km^3/s^2)
}

/// A structure to hold the state of an N-Body simulation
#[derive(Debug, Clone)]
pub struct NBodyState {
    pub bodies: HashMap<String, BodyState>,
    pub c: f64, // Speed of light for PN corrections
}

#[derive(Debug, Clone)]
pub struct BodyState {
    pub position: [f64; 3], // km
    pub velocity: [f64; 3], // km/s
    pub mu: f64,            // km^3/s^2
}

impl NBodyState {
    pub fn new() -> Self {
        Self {
            bodies: HashMap::new(),
            c: 299792.458,
        }
    }

    pub fn insert_body(&mut self, id: &str, state: BodyState) {
        self.bodies.insert(id.to_string(), state);
    }

    /// Computes the mutual gravitational acceleration for a specific body given all other bodies
    pub fn compute_acceleration(&self, body_id: &str, pos: &[f64; 3], vel: &[f64; 3]) -> [f64; 3] {
        let mut ax = 0.0;
        let mut ay = 0.0;
        let mut az = 0.0;

        for (other_id, other_state) in &self.bodies {
            if body_id == other_id {
                continue;
            }

            let dx = other_state.position[0] - pos[0];
            let dy = other_state.position[1] - pos[1];
            let dz = other_state.position[2] - pos[2];

            let r2 = dx * dx + dy * dy + dz * dz;
            let r = r2.sqrt();
            let r3 = r2 * r;

            let central = other_state.mu / r3;
            ax += dx * central;
            ay += dy * central;
            az += dz * central;

            // Simplified 1PN relativistic interactions could be added here for massive bodies (e.g. Sun, Jupiter)
            let v2 = vel[0]*vel[0] + vel[1]*vel[1] + vel[2]*vel[2];
            let r_dot_v = -(dx*vel[0] + dy*vel[1] + dz*vel[2]); // vector from body to other
            let c2 = self.c * self.c;
            let pn_coeff = other_state.mu / (c2 * r3);
            
            let term1 = 4.0 * other_state.mu / r - v2;
            let term2 = 4.0 * r_dot_v;

            ax += pn_coeff * (term1 * dx + term2 * vel[0]);
            ay += pn_coeff * (term1 * dy + term2 * vel[1]);
            az += pn_coeff * (term1 * dz + term2 * vel[2]);
        }

        [ax, ay, az]
    }

    /// Advances the entire N-body system by dt using 4th-order Yoshida Symplectic Integration
    pub fn step_nbody(&mut self, dt: f64) {
        let w0 = -(2.0f64.powf(1.0 / 3.0)) / (2.0 - 2.0f64.powf(1.0 / 3.0));
        let w1 = 1.0 / (2.0 - 2.0f64.powf(1.0 / 3.0));
        
        let c1 = w1 / 2.0;
        let c2 = (w0 + w1) / 2.0;
        let c3 = c2;
        let c4 = c1;

        let d1 = w1;
        let d2 = w0;
        let d3 = w1;

        let c_coeffs = [c1, c2, c3, c4];
        let d_coeffs = [d1, d2, d3];

        let body_ids: Vec<String> = self.bodies.keys().cloned().collect();

        for i in 0..3 {
            // Drift
            for id in &body_ids {
                let state = self.bodies.get_mut(id).unwrap();
                state.position[0] += c_coeffs[i] * state.velocity[0] * dt;
                state.position[1] += c_coeffs[i] * state.velocity[1] * dt;
                state.position[2] += c_coeffs[i] * state.velocity[2] * dt;
            }

            // Compute accelerations (Kick preparation)
            let mut accelerations = HashMap::new();
            for id in &body_ids {
                let state = self.bodies.get(id).unwrap();
                let acc = self.compute_acceleration(id, &state.position, &state.velocity);
                accelerations.insert(id.clone(), acc);
            }

            // Kick
            for id in &body_ids {
                let state = self.bodies.get_mut(id).unwrap();
                let acc = accelerations.get(id).unwrap();
                state.velocity[0] += d_coeffs[i] * acc[0] * dt;
                state.velocity[1] += d_coeffs[i] * acc[1] * dt;
                state.velocity[2] += d_coeffs[i] * acc[2] * dt;
            }
        }

        // Final Drift
        for id in &body_ids {
            let state = self.bodies.get_mut(id).unwrap();
            state.position[0] += c_coeffs[3] * state.velocity[0] * dt;
            state.position[1] += c_coeffs[3] * state.velocity[1] * dt;
            state.position[2] += c_coeffs[3] * state.velocity[2] * dt;
        }
    }
}

/// JPL Horizons kernel slice (JSON). Linear interpolation between samples.
/// This is not the full DE440 bsp — it is a dated, hashed extract.
#[derive(Debug, Clone)]
pub struct JplSample {
    pub jd: f64,
    pub r_km: [f64; 3],
    pub v_kms: [f64; 3],
}

#[derive(Debug, Clone)]
pub struct JplBodySeries {
    pub mu_km3_s2: f64,
    pub samples: Vec<JplSample>,
}

#[derive(Debug, Clone)]
pub struct JplEphemerisLoader {
    pub is_loaded: bool,
    pub path: String,
    pub bodies: HashMap<String, JplBodySeries>,
    pub jd_start: f64,
    pub jd_end: f64,
}

impl JplEphemerisLoader {
    pub fn new(path: &str) -> Self {
        Self {
            is_loaded: false,
            path: path.to_string(),
            bodies: HashMap::new(),
            jd_start: 0.0,
            jd_end: 0.0,
        }
    }

    pub fn load(&mut self) -> Result<(), String> {
        let bytes = std::fs::read(&self.path).map_err(|e| format!("kernel {}: {e}", self.path))?;
        let raw: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| format!("kernel json: {e}"))?;
        let bodies_val = raw
            .get("bodies")
            .and_then(|v| v.as_object())
            .ok_or_else(|| "kernel missing bodies".to_string())?;

        let mut bodies = HashMap::new();
        let mut jd_start = f64::MAX;
        let mut jd_end = f64::MIN;

        for (name, body) in bodies_val {
            let mu = body
                .get("mu_km3_s2")
                .and_then(|v| v.as_f64())
                .ok_or_else(|| format!("{name}: missing mu"))?;
            let samples_val = body
                .get("samples")
                .and_then(|v| v.as_array())
                .ok_or_else(|| format!("{name}: missing samples"))?;
            let mut samples = Vec::with_capacity(samples_val.len());
            for s in samples_val {
                let jd = s.get("jd").and_then(|v| v.as_f64()).ok_or("sample jd")?;
                let r = take3(s.get("r_km").ok_or("r_km")?)?;
                let v = take3(s.get("v_kms").ok_or("v_kms")?)?;
                jd_start = jd_start.min(jd);
                jd_end = jd_end.max(jd);
                samples.push(JplSample {
                    jd,
                    r_km: r,
                    v_kms: v,
                });
            }
            samples.sort_by(|a, b| a.jd.partial_cmp(&b.jd).unwrap());
            bodies.insert(
                name.clone(),
                JplBodySeries {
                    mu_km3_s2: mu,
                    samples,
                },
            );
        }

        self.bodies = bodies;
        self.jd_start = jd_start;
        self.jd_end = jd_end;
        self.is_loaded = true;
        Ok(())
    }

    pub fn body_names(&self) -> Vec<String> {
        let mut n: Vec<String> = self.bodies.keys().cloned().collect();
        n.sort();
        n
    }

    fn interpolate(series: &JplBodySeries, time_jd: f64) -> Option<([f64; 3], [f64; 3])> {
        let s = &series.samples;
        if s.is_empty() {
            return None;
        }
        if time_jd <= s[0].jd {
            return Some((s[0].r_km, s[0].v_kms));
        }
        if time_jd >= s[s.len() - 1].jd {
            let last = &s[s.len() - 1];
            return Some((last.r_km, last.v_kms));
        }
        let mut lo = 0usize;
        let mut hi = s.len() - 1;
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if s[mid].jd <= time_jd {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let a = &s[lo];
        let b = &s[hi];
        let span = (b.jd - a.jd).max(1e-12);
        let t = (time_jd - a.jd) / span;
        let mut r = [0.0; 3];
        let mut v = [0.0; 3];
        for i in 0..3 {
            r[i] = a.r_km[i] + t * (b.r_km[i] - a.r_km[i]);
            v[i] = a.v_kms[i] + t * (b.v_kms[i] - a.v_kms[i]);
        }
        Some((r, v))
    }
}

fn take3(v: &serde_json::Value) -> Result<[f64; 3], String> {
    let a = v.as_array().ok_or_else(|| "expected [x,y,z]".to_string())?;
    if a.len() < 3 {
        return Err("vec3 short".into());
    }
    Ok([
        a[0].as_f64().ok_or("x")?,
        a[1].as_f64().ok_or("y")?,
        a[2].as_f64().ok_or("z")?,
    ])
}

impl Ephemeris for JplEphemerisLoader {
    fn get_position(&self, body_id: &str, time_jd: f64) -> Option<[f64; 3]> {
        if !self.is_loaded {
            return None;
        }
        let series = self.bodies.get(body_id)?;
        Self::interpolate(series, time_jd).map(|(r, _)| r)
    }

    fn get_velocity(&self, body_id: &str, time_jd: f64) -> Option<[f64; 3]> {
        if !self.is_loaded {
            return None;
        }
        let series = self.bodies.get(body_id)?;
        Self::interpolate(series, time_jd).map(|(_, v)| v)
    }

    fn get_mu(&self, body_id: &str) -> Option<f64> {
        self.bodies.get(body_id).map(|b| b.mu_km3_s2)
    }
}

/// A priori reduced-order coherence gates (6-body Yoshida vs DE440/Horizons).
pub fn ephemeris_gate_km(body: &str) -> f64 {
    match body {
        "Sun" => 5_000.0,
        "Moon" => 50_000.0, // 1-hour Yoshida vs daily JPL; lunar harmonics omitted
        "Mercury" | "Venus" | "Earth" | "Mars" => 1_495_978.7, // 0.01 AU
        "Jupiter" | "Saturn" | "Uranus" | "Neptune" => 7_479_893.5, // 0.05 AU
        "Pluto" => 14_959_787.0, // 0.1 AU
        _ => 1_495_978.7,
    }
}
