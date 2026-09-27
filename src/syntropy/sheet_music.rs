//! Physical Sheet Music — an exact, reconstructible timeline of actuations.

use std::fmt;

/// Physical kind of an actuation channel. Magnitude is always an `f64` in `unit`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelKind {
    Force,
    Torque,
    Frequency,
    Nutrient,
    Concentration,
    Angle,
}

/// Named channel the plant actually consumes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActuationChannel {
    pub name: &'static str,
    pub unit: &'static str,
    pub kind: ChannelKind,
}

/// One hold on one channel. Integer steps make reconstruction exact.
#[derive(Clone, Debug, PartialEq)]
pub struct ActuationEvent {
    pub time_s: f64,
    pub duration_s: f64,
    pub start_step: usize,
    pub n_steps: usize,
    pub channel_index: usize,
    pub magnitude: f64,
    pub unit: &'static str,
}

/// Piecewise-constant score of physical actuations.
///
/// Dense `samples` is row-major: `samples[k * nu + j]` at step `k`, channel `j`.
/// [`SheetMusic::reconstruct`] must recover that vector from `events`.
#[derive(Clone, Debug)]
pub struct SheetMusic {
    pub dt_s: f64,
    pub channels: Vec<ActuationChannel>,
    pub events: Vec<ActuationEvent>,
    pub samples: Vec<f64>,
    pub n_steps: usize,
}

impl SheetMusic {
    /// Compress a dense sample tape into hold events, keeping the tape for replay.
    pub fn from_samples(
        channels: &[ActuationChannel],
        dt_s: f64,
        n_steps: usize,
        samples: Vec<f64>,
        abs_eps: f64,
    ) -> Self {
        let nu = channels.len();
        debug_assert_eq!(samples.len(), n_steps.saturating_mul(nu));
        let mut events = Vec::new();
        for j in 0..nu {
            let unit = channels[j].unit;
            let mut k = 0usize;
            while k < n_steps {
                let mag = samples[k * nu + j];
                let start = k;
                k += 1;
                while k < n_steps && (samples[k * nu + j] - mag).abs() <= abs_eps {
                    k += 1;
                }
                let hold = k - start;
                events.push(ActuationEvent {
                    time_s: start as f64 * dt_s,
                    duration_s: hold as f64 * dt_s,
                    start_step: start,
                    n_steps: hold,
                    channel_index: j,
                    magnitude: mag,
                    unit,
                });
            }
        }
        events.sort_by(|a, b| {
            a.start_step
                .cmp(&b.start_step)
                .then(a.channel_index.cmp(&b.channel_index))
        });
        Self {
            dt_s,
            channels: channels.to_vec(),
            events,
            samples,
            n_steps,
        }
    }

    /// Expand events back into the dense actuation vector the dynamics consume.
    pub fn reconstruct(&self) -> Vec<f64> {
        let nu = self.channels.len();
        let mut out = vec![0.0; self.n_steps * nu];
        for ev in &self.events {
            let j = ev.channel_index;
            if j >= nu {
                continue;
            }
            let end = (ev.start_step + ev.n_steps).min(self.n_steps);
            for k in ev.start_step..end {
                out[k * nu + j] = ev.magnitude;
            }
        }
        out
    }

    /// Hold-exact resample. Piecewise-constant magnitudes are preserved;
    /// `new_dt_s` is the playback clock (grasp body-time is 0.001 s).
    pub fn resample(&self, new_dt_s: f64) -> Self {
        let new_dt_s = new_dt_s.max(1e-9);
        let duration = self.n_steps as f64 * self.dt_s;
        let n_steps = ((duration / new_dt_s).round() as usize).max(1);
        let nu = self.channels.len();
        let old = self.reconstruct();
        let mut samples = vec![0.0; n_steps.saturating_mul(nu)];
        if self.n_steps == 0 || nu == 0 || old.is_empty() {
            return Self::from_samples(&self.channels, new_dt_s, n_steps, samples, 1e-4);
        }
        for k in 0..n_steps {
            let t = k as f64 * new_dt_s;
            let src = ((t / self.dt_s).floor() as usize).min(self.n_steps - 1);
            for j in 0..nu {
                samples[k * nu + j] = old[src * nu + j];
            }
        }
        Self::from_samples(&self.channels, new_dt_s, n_steps, samples, 1e-4)
    }

    /// Repeat the last sample until the tape lasts `total_duration_s`.
    pub fn extend_last_hold(&self, total_duration_s: f64) -> Self {
        let current = self.n_steps as f64 * self.dt_s;
        let nu = self.channels.len();
        if total_duration_s <= current + 0.5 * self.dt_s || nu == 0 {
            return self.clone();
        }
        let extra = ((total_duration_s - current) / self.dt_s).round() as usize;
        let mut samples = self.reconstruct();
        let last: Vec<f64> = if self.n_steps == 0 {
            vec![0.0; nu]
        } else {
            samples[(self.n_steps - 1) * nu..].to_vec()
        };
        samples.reserve(extra * nu);
        for _ in 0..extra {
            samples.extend_from_slice(&last);
        }
        Self::from_samples(
            &self.channels,
            self.dt_s,
            self.n_steps + extra,
            samples,
            1e-4,
        )
    }

    /// Human-readable score. Example: `0.000–0.003 s grip_force 40.0 N`.
    pub fn format_score(&self) -> String {
        format!("{self}")
    }
}

impl fmt::Display for SheetMusic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Physical Sheet Music")?;
        writeln!(
            f,
            " clock dt={:.6} s {} samples {} channel(s)",
            self.dt_s,
            self.n_steps,
            self.channels.len()
        )?;
        writeln!(f, " -----------------------------------------")?;
        if self.events.is_empty() {
            writeln!(f, " (rest)")?;
            return Ok(());
        }
        for ev in &self.events {
            let name = self
                .channels
                .get(ev.channel_index)
                .map(|c| c.name)
                .unwrap_or("?");
            let t1 = ev.time_s + ev.duration_s;
            writeln!(
                f,
                " {:>8.4}–{:<8.4} s {:<16} {:>10.4} {}",
                ev.time_s, t1, name, ev.magnitude, ev.unit
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grip() -> ActuationChannel {
        ActuationChannel {
            name: "grip_force",
            unit: "N",
            kind: ChannelKind::Force,
        }
    }

    #[test]
    fn compress_and_reconstruct_holds() {
        let ch = [grip()];
        // 40 N for 3 steps, 10 N for 2 steps — "Apply 40N … for 3 ms at dt = 0.001 s".
        let samples = vec![40.0, 40.0, 40.0, 10.0, 10.0];
        let sheet = SheetMusic::from_samples(&ch, 0.001, 5, samples.clone(), 1e-9);
        assert_eq!(sheet.events.len(), 2);
        assert!((sheet.events[0].magnitude - 40.0).abs() < 1e-12);
        assert_eq!(sheet.events[0].n_steps, 3);
        assert!((sheet.events[0].duration_s - 0.003).abs() < 1e-15);
        assert!((sheet.events[1].magnitude - 10.0).abs() < 1e-12);
        let replay = sheet.reconstruct();
        assert_eq!(replay, samples);
        let text = format!("{sheet}");
        assert!(text.contains("grip_force"));
        assert!(text.contains("40.0000 N"));
    }

    #[test]
    fn two_channels_interleave_by_time() {
        let ch = [
            grip(),
            ActuationChannel {
                name: "close_angle",
                unit: "rad",
                kind: ChannelKind::Angle,
            },
        ];
        // k=0: 5 N, 0.2 rad; k=1: 5 N, 0.4 rad
        let samples = vec![5.0, 0.2, 5.0, 0.4];
        let sheet = SheetMusic::from_samples(&ch, 0.001, 2, samples.clone(), 1e-12);
        assert_eq!(sheet.reconstruct(), samples);
        assert!(sheet
            .events
            .iter()
            .any(|e| e.channel_index == 1 && e.n_steps == 1));
    }

    #[test]
    fn resample_holds_preserve_magnitude() {
        let ch = [grip()];
        let samples = vec![40.0, 40.0, 10.0];
        let sheet = SheetMusic::from_samples(&ch, 0.050, 3, samples, 1e-12);
        let fine = sheet.resample(0.001);
        assert_eq!(fine.n_steps, 150);
        assert!((fine.dt_s - 0.001).abs() < 1e-18);
        assert!((fine.samples[0] - 40.0).abs() < 1e-12);
        assert!((fine.samples[49] - 40.0).abs() < 1e-12);
        assert!((fine.samples[100] - 10.0).abs() < 1e-12);
        assert_eq!(fine.events.len(), 2);
    }

    #[test]
    fn extend_last_hold_pads_duration() {
        let ch = [grip()];
        let sheet = SheetMusic::from_samples(&ch, 0.001, 4, vec![12.0, 12.0, 30.0, 30.0], 1e-12);
        let long = sheet.extend_last_hold(0.010);
        assert_eq!(long.n_steps, 10);
        assert!((long.samples[3] - 30.0).abs() < 1e-12);
        assert!((long.samples[9] - 30.0).abs() < 1e-12);
        assert_eq!(long.events.len(), 2);
    }
}
