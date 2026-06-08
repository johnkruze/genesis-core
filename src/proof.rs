use sha2::{Sha256, Digest};

/// SHA-256 proof chain for trajectory sealing.
/// Pattern: new → feed state at cadence → seal at end.
#[derive(Debug, Clone)]
pub struct ProofChain {
    hasher: Sha256,
    feeds: u64,
}

impl ProofChain {
    pub fn new() -> Self {
        Self {
            hasher: Sha256::new(),
            feeds: 0,
        }
    }

    /// Seed the chain with initial conditions
    pub fn seed(&mut self, data: &[u8]) {
        self.hasher.update(data);
    }

    /// Feed state into the chain (call at proof cadence, e.g. 1Hz)
    pub fn feed(&mut self, data: &[u8]) {
        self.hasher.update(data);
        self.feeds += 1;
    }

    /// Feed an f64 value
    pub fn feed_f64(&mut self, val: f64) {
        self.hasher.update(val.to_le_bytes());
    }

    /// Feed a string/label
    pub fn feed_str(&mut self, s: &str) {
        self.hasher.update(s.as_bytes());
    }

    /// Number of state feeds accumulated
    pub fn feed_count(&self) -> u64 {
        self.feeds
    }

    /// Seal the chain and return the hex-encoded SHA-256 hash.
    /// Consumes the chain — each proof is one-time.
    pub fn seal(self) -> String {
        hex::encode(self.hasher.finalize())
    }
}

/// Seal a collection of proof hashes into a single run-level proof.
pub fn seal_run(proof_hashes: &[String]) -> String {
    let mut hasher = Sha256::new();
    for h in proof_hashes {
        hasher.update(h.as_bytes());
    }
    hex::encode(hasher.finalize())
}
