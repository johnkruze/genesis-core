//! Last-state frame geometry — the local orb.
//! 64-byte header + 64-byte frames. SPECTRA OceanState is 8×f64 = 64 B same size.

pub const HEADER_BYTES: u64 = 64;
pub const FRAME_BYTES: u64 = 64;
pub const MAGIC: [u8; 4] = *b"SOMA";
