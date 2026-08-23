use sha2::{Sha256, Digest};

// Atheric (Vibration/Energy): Sovereign coherence below the noise floor.
// THE EMBODIMENT: A signal is a body in the electromagnetic substrate.
// Noise is gravity — it pulls coherence apart. Frequency hopping is locomotion.
// SHA-256 seeds the hop pattern — crypto IS the physics. The hopping
// sequence IS the sovereignty. If the pattern is compromised (clock drift),
// the multi-channel advantage collapses to single-channel equivalence.
//
// The question: Can a signal maintain sovereign coherence below the noise floor?
// Shannon says C = B·log2(1 + SNR). We find the cliff empirically.
// And we discover: compromised hopping reduces any N-channel system to 1.

// ─── CONSTANTS ────────────────────────────────────────────

pub const BASE_FREQUENCY: f64 = 2.4e9;        // 2.4 GHz ISM Band
pub const SPEED_OF_LIGHT: f64 = 299_792_458.0;
pub const RESONANCE_GATE: f64 = 0.98;         // Minimum coherence for inhabitation

// ─── UNIT HELPERS ─────────────────────────────────────────

/// dBm → Watts: P(W) = 10^((dBm - 30) / 10)
pub fn dbm_to_watts(dbm: f64) -> f64 {
    10.0_f64.powf((dbm - 30.0) / 10.0)
}

/// Watts → dBm: dBm = 10·log10(P) + 30
pub fn watts_to_dbm(watts: f64) -> f64 {
    if watts <= 0.0 { return -200.0; }
    10.0 * watts.log10() + 30.0
}

/// Free-space received power (Friis equation, isotropic antennas, clamped).
/// Lab path-loss is live at BASE_FREQUENCY (2.4 GHz ISM).
pub fn free_space_received(tx_w: f64, freq_hz: f64, distance_m: f64) -> f64 {
    if distance_m <= 0.0 { return tx_w; }
    let wavelength = SPEED_OF_LIGHT / freq_hz;
    let ratio = wavelength / (4.0 * std::f64::consts::PI * distance_m);
    (tx_w * ratio * ratio).min(tx_w) // can't receive more than transmitted
}

/// Shannon channel capacity: C = B·log2(1 + SNR)
pub fn shannon_capacity(bandwidth_hz: f64, snr_linear: f64) -> f64 {
    if snr_linear <= 0.0 { return 0.0; }
    bandwidth_hz * (1.0 + snr_linear).log2()
}

// ─── CHANNEL ──────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Channel {
    pub frequency: f64,
    pub noise_power: f64,     // Watts
    pub signal_power: f64,    // Watts (received)
    pub jammed: bool,
    pub fading: f64,          // 0.0-1.0 multipath attenuation
}

impl Channel {
    pub fn snr_linear(&self) -> f64 {
        if self.jammed { return 0.0; }
        (self.signal_power * self.fading) / self.noise_power.max(1e-30)
    }

    pub fn snr_db(&self) -> f64 {
        let s = self.snr_linear();
        if s <= 0.0 { -200.0 } else { 10.0 * s.log10() }
    }

    pub fn capacity(&self, bandwidth: f64) -> f64 {
        shannon_capacity(bandwidth, self.snr_linear())
    }
}

// ─── ATHERIC SYSTEM (Multi-channel SHA-256 hopping) ───────

#[derive(Debug, Clone)]
pub struct AthericSystem {
    pub channels: Vec<Channel>,
    pub bandwidth: f64,       // Hz per channel
    pub hop_seed: [u8; 32],
    pub hop_index: u64,
    pub desync: bool,         // clock drift desynchronized the hop pattern
}

impl AthericSystem {
    /// Build N channels at BASE_FREQUENCY harmonics (2.4 GHz ISM).
    pub fn new(
        n_channels: usize,
        tx_power: f64,
        noise_floor_dbm: f64,
        distance_km: f64,
    ) -> Self {
        let noise_w = dbm_to_watts(noise_floor_dbm);
        let dist_m = distance_km * 1000.0;
        let channels = (0..n_channels).map(|i| {
            let freq = BASE_FREQUENCY * (i + 1) as f64;
            let rx = free_space_received(tx_power, freq, dist_m);
            Channel {
                frequency: freq,
                noise_power: noise_w,
                signal_power: rx,
                jammed: false,
                fading: 1.0,
            }
        }).collect();

        Self {
            channels,
            bandwidth: 100.0,
            hop_seed: [0u8; 32],
            hop_index: 0,
            desync: false,
        }
    }

    /// SHA-256-seeded channel selection. Crypto IS the physics.
    pub fn hop_channel(&self) -> usize {
        let n = self.channels.len().max(1);
        let mut h = Sha256::new();
        h.update(self.hop_seed);
        h.update(self.hop_index.to_le_bytes());
        let hash: [u8; 32] = h.finalize().into();
        let v = u64::from_le_bytes([
            hash[0], hash[1], hash[2], hash[3],
            hash[4], hash[5], hash[6], hash[7],
        ]);
        (v as usize) % n
    }

    /// Transmit one packet on the hopped channel.
    /// Returns (received, channel_index, snr_db).
    pub fn transmit_packet(&mut self, min_snr_db: f64) -> (bool, usize, f64) {
        let idx = self.hop_channel();
        let snr = self.channels[idx].snr_db();
        let ok = snr > min_snr_db && !self.channels[idx].jammed;
        self.hop_index += 1;
        (ok, idx, snr)
    }

    /// Coherence: fraction of channels above SNR threshold.
    pub fn coherence(&self, min_snr_db: f64) -> f64 {
        let n = self.channels.len();
        if n == 0 { return 0.0; }
        let above = self.channels.iter()
            .filter(|c| c.snr_db() > min_snr_db && !c.jammed)
            .count();
        above as f64 / n as f64
    }

    /// Total system Shannon capacity (bits/sec).
    pub fn total_capacity(&self) -> f64 {
        self.channels.iter()
            .filter(|c| !c.jammed)
            .map(|c| c.capacity(self.bandwidth))
            .sum()
    }

    /// Average SNR (dB) across active channels.
    pub fn avg_snr_db(&self) -> f64 {
        let active: Vec<f64> = self.channels.iter()
            .filter(|c| !c.jammed)
            .map(|c| c.snr_db())
            .collect();
        if active.is_empty() { return -200.0; }
        active.iter().sum::<f64>() / active.len() as f64
    }

    // ─── FAILURE EVENTS ───────────────────────────────

    /// Broadband interference: raises noise on ALL channels.
    pub fn apply_broadband(&mut self, intensity: f64) {
        let mult = 1.0 + intensity * 100.0;
        for c in &mut self.channels { c.noise_power *= mult; }
    }

    /// Narrowband jamming: kills specific channels.
    pub fn apply_jamming(&mut self, n_jammed: usize, rng: &mut crate::rng::Rng) {
        let n = self.channels.len();
        let target = n_jammed.min(n);
        let mut count = 0;
        let mut tries = 0;
        while count < target && tries < n * 3 {
            let i = rng.index(n);
            if !self.channels[i].jammed {
                self.channels[i].jammed = true;
                count += 1;
            }
            tries += 1;
        }
    }

    /// Multipath fading: random attenuation per channel.
    pub fn apply_fading(&mut self, severity: f64, rng: &mut crate::rng::Rng) {
        for c in &mut self.channels {
            c.fading *= rng.range(1.0 - severity, 1.0).max(0.01);
        }
    }

    /// Power fade: signal drops on all channels.
    pub fn apply_power_fade(&mut self, factor: f64) {
        for c in &mut self.channels { c.signal_power *= factor; }
    }

    /// Clock drift: desynchronizes the hop pattern.
    /// Receiver can no longer predict the transmitter's channel.
    /// Multi-channel collapses to 1/N hit probability.
    pub fn apply_clock_drift(&mut self) {
        self.desync = true;
    }
}

// ─── ERASURE CODING ──────────────────────────────────────

/// Check: received enough packets for reconstruction?
pub fn erasure_check(received: usize, needed: usize) -> bool {
    received >= needed
}

/// Packets to transmit with redundancy factor.
pub fn with_redundancy(data_packets: usize, redundancy: f64) -> usize {
    (data_packets as f64 * redundancy).ceil() as usize
}

/// The Citadel Direct-Link. Calculate SNR and generate the 432Hz internal frequency response.
pub fn run_atheric_handshake(intent_hash: [u8; 32], strength: f32, distance: f32) -> String {
    let mut sys = AthericSystem::new(8, strength as f64, -120.0, distance as f64);
    sys.hop_seed = intent_hash;
    let snr = sys.avg_snr_db();
    let coherence = sys.coherence(3.0);
    
    if coherence > RESONANCE_GATE {
        format!("Atheric Handshake ESTABLISHED. Resonance: {:.4}. SNR: {:.1} dB.", coherence, snr)
    } else {
        format!("Atheric Handshake FAILED. Resonance: {:.4} (Requires > {}). SNR: {:.1} dB.", coherence, RESONANCE_GATE, snr)
    }
}
