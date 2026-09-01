//! Last-state file geometry — the local orb when the radio is dead.
//!
//! One header, one frame envelope, named slots per `body_id`.
//! Law: `grokd/public/soma/SOMA.md`. Magic lives in the **header only**.
//!
//! Live 1 kHz orbs (marine 8×f64, drone packed RAM frame) are siblings, not this file.

use sha2::{Digest, Sha256};

pub const HEADER_BYTES: u64 = 64;
pub const FRAME_BYTES: u64 = 64;
pub const MAGIC: [u8; 4] = *b"SOMA";
pub const SPEC_VERSION: u16 = 1;

pub const BODY_OCEAN: u16 = 6;
pub const BODY_DRONE: u16 = 7;
pub const BODY_VEHICLE: u16 = 9; /* STREAM2 Pacejka chassis / hydroplane */
pub const BODY_MYCELIAL: u16 = 8; /* STREAM1 mycelial Kirchhoff terminal */
pub const BODY_PLASMA: u16 = 10; /* STREAM3 reentry / plasma */
pub const BODY_FUSION: u16 = 11; /* STREAM4 fusion tokamak/pit terminal */
pub const BODY_ATHERIC: u16 = 13;
pub const BODY_AUTOLAB: u16 = 27;
pub const BODY_GRASP: u16 = 28;
pub const BODY_FLEET: u16 = 29;
pub const BODY_HUMANOID: u16 = 30;
pub const BODY_HAND: u16 = 31;
pub const BODY_COMPOUNDING: u16 = 32; /* STREAM5 compounding mill BROTH001 */

pub const FLAG_DARK_WINDOW: u64 = 1 << 0;
pub const FLAG_HUMANOID_BUCKLE: u64 = 1 << 1;
pub const FLAG_HUMANOID_REFLEX: u64 = 1 << 2;
pub const FLAG_HAND_OVERSTRETCH: u64 = 1 << 0;
pub const FLAG_HAND_PAD_SLIP: u64 = 1 << 1;
pub const FLAG_OCEAN_CRUSHED: u64 = 1 << 0;
pub const FLAG_OCEAN_STARVED: u64 = 1 << 1;
pub const FLAG_DRONE_DARK: u64 = 1 << 0;
pub const FLAG_DRONE_VSLAM_FAIL: u64 = 1 << 1;
pub const FLAG_DRONE_REFLEX: u64 = 1 << 2;
pub const FLAG_VEHICLE_HYDROPLANE: u64 = 1 << 0; /* STREAM2 body 9 */
pub const FLAG_VEHICLE_CORNER_LOST: u64 = 1 << 1;
pub const FLAG_VEHICLE_GRIP: u64 = 1 << 2;
pub const FLAG_MYCELIAL_FRAGMENTED: u64 = 1 << 0; /* STREAM1 body 8 */
pub const FLAG_MYCELIAL_BELOW_PERC: u64 = 1 << 1;
pub const FLAG_PLASMA_BLACKOUT: u64 = 1 << 0; /* STREAM3 */
pub const FLAG_PLASMA_MISS: u64 = 1 << 1;
pub const FLAG_PLASMA_GPS_HELD: u64 = 1 << 2;
pub const FLAG_FUSION_PROMPT: u64 = 1 << 0; /* STREAM4 */
pub const FLAG_FUSION_SURVIVED: u64 = 1 << 1;
pub const FLAG_COMPOUNDING_POTENCY_COLLAPSED: u64 = 1 << 0; /* STREAM5 body 32 */
pub const FLAG_COMPOUNDING_DISSOLUTION_STALLED: u64 = 1 << 1;

/// File header. Matches Python `'<4sHHQQ32s8s'`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct LastStateHeader64 {
    pub magic: [u8; 4],
    pub spec_version: u16,
    pub body_id: u16,
    pub traj_count: u64,
    pub frame_count: u64,
    pub digest: [u8; 32],
    pub reserved: [u8; 8],
}

const _: () = {
    assert!(std::mem::size_of::<LastStateHeader64>() == 64);
    assert!(std::mem::offset_of!(LastStateHeader64, body_id) == 6);
    assert!(std::mem::offset_of!(LastStateHeader64, digest) == 24);
};

/// File frame envelope. Matches Python `'<d6f2fQ16s'`.
/// Slot names depend on `body_id` (see SOMA.md).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct LastStateFrame64 {
    pub t: f64,
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    pub force_torque: f32,
    pub residual: f32,
    pub flags: u64,
    pub proof: [u8; 16],
}

const _: () = {
    assert!(std::mem::size_of::<LastStateFrame64>() == 64);
    assert!(std::mem::offset_of!(LastStateFrame64, flags) == 40);
    assert!(std::mem::offset_of!(LastStateFrame64, proof) == 48);
};

impl LastStateHeader64 {
    pub fn to_bytes(self) -> [u8; 64] {
        unsafe { std::mem::transmute(self) }
    }
}

impl LastStateFrame64 {
    pub fn to_bytes(self) -> [u8; 64] {
        unsafe { std::mem::transmute(self) }
    }

    pub fn from_bytes(bytes: [u8; 64]) -> Self {
        unsafe { std::mem::transmute(bytes) }
    }

    /// Humanoid slots: pos=COM, vel=velocity, force_torque=pitch, residual=ZMP margin.
    pub fn pack_humanoid(
        timestamp_ms: u32,
        com_xyz: [f32; 3],
        velocity_xyz: [f32; 3],
        pitch_rad: f32,
        zmp_margin_m: f32,
        is_dark_window: bool,
        is_buckle: bool,
        is_reflex_grasp: bool,
    ) -> Self {
        let mut flags = 0u64;
        if is_dark_window {
            flags |= FLAG_DARK_WINDOW;
        }
        if is_buckle {
            flags |= FLAG_HUMANOID_BUCKLE;
        }
        if is_reflex_grasp {
            flags |= FLAG_HUMANOID_REFLEX;
        }

        let t = timestamp_ms as f64 / 1000.0;
        let mut hasher = Sha256::new();
        hasher.update(&t.to_le_bytes());
        for f in &com_xyz {
            hasher.update(&f.to_le_bytes());
        }
        for f in &velocity_xyz {
            hasher.update(&f.to_le_bytes());
        }
        hasher.update(&pitch_rad.to_le_bytes());
        hasher.update(&zmp_margin_m.to_le_bytes());
        hasher.update(&flags.to_le_bytes());
        let digest = hasher.finalize();
        let mut proof = [0u8; 16];
        proof.copy_from_slice(&digest[..16]);

        Self {
            t,
            pos: com_xyz,
            vel: velocity_xyz,
            force_torque: pitch_rad,
            residual: zmp_margin_m,
            flags,
            proof,
        }
    }

    /// Hand slots: pos=tension/pad_normal/stretch, vel=opposition/q_mcp/slip,
    /// force_torque=margin, residual=object_span.
    pub fn pack_hand(
        timestamp_ms: u32,
        tension_n: f32,
        pad_normal_n: f32,
        stretch_m: f32,
        opposition_rad: f32,
        q_mcp: f32,
        slip_m_s: f32,
        margin: f32,
        object_span_m: f32,
        tendon_overstretch: bool,
        pad_slip: bool,
    ) -> Self {
        let mut flags = 0u64;
        if tendon_overstretch {
            flags |= FLAG_HAND_OVERSTRETCH;
        }
        if pad_slip {
            flags |= FLAG_HAND_PAD_SLIP;
        }
        let t = timestamp_ms as f64 / 1000.0;
        let pos = [tension_n, pad_normal_n, stretch_m];
        let vel = [opposition_rad, q_mcp, slip_m_s];
        let mut hasher = Sha256::new();
        hasher.update(&t.to_le_bytes());
        for f in &pos {
            hasher.update(&f.to_le_bytes());
        }
        for f in &vel {
            hasher.update(&f.to_le_bytes());
        }
        hasher.update(&margin.to_le_bytes());
        hasher.update(&object_span_m.to_le_bytes());
        hasher.update(&flags.to_le_bytes());
        let digest = hasher.finalize();
        let mut proof = [0u8; 16];
        proof.copy_from_slice(&digest[..16]);
        Self {
            t,
            pos,
            vel,
            force_torque: margin,
            residual: object_span_m,
            flags,
            proof,
        }
    }

    /// Ocean slots: pos=depth/pressure/battery_wh, vel=true_crush/believed/used_pct,
    /// force_torque=mass_kg, residual=target_depth_m. Flags bit0 crushed · bit1 starved.
    pub fn pack_ocean(
        timestamp_ms: u32,
        max_depth_m: f32,
        peak_pressure_mpa: f32,
        battery_wh: f32,
        true_crush_m: f32,
        believed_crush_m: f32,
        battery_used_pct: f32,
        mass_kg: f32,
        target_depth_m: f32,
        is_crushed: bool,
        is_power_starved: bool,
    ) -> Self {
        let mut flags = 0u64;
        if is_crushed {
            flags |= FLAG_OCEAN_CRUSHED;
        }
        if is_power_starved {
            flags |= FLAG_OCEAN_STARVED;
        }
        let t = timestamp_ms as f64 / 1000.0;
        let pos = [max_depth_m, peak_pressure_mpa, battery_wh];
        let vel = [true_crush_m, believed_crush_m, battery_used_pct];
        let mut hasher = Sha256::new();
        hasher.update(&t.to_le_bytes());
        for f in &pos {
            hasher.update(&f.to_le_bytes());
        }
        for f in &vel {
            hasher.update(&f.to_le_bytes());
        }
        hasher.update(&mass_kg.to_le_bytes());
        hasher.update(&target_depth_m.to_le_bytes());
        hasher.update(&flags.to_le_bytes());
        let digest = hasher.finalize();
        let mut proof = [0u8; 16];
        proof.copy_from_slice(&digest[..16]);
        Self {
            t,
            pos,
            vel,
            force_torque: mass_kg,
            residual: target_depth_m,
            flags,
            proof,
        }
    }

    /// Drone file slots: pos=xyz m, vel=vxyz m/s, force_torque=pitch rad,
    /// residual=VSLAM/IMU coherence residual. Flags bit0 dark · bit1 vslam_fail · bit2 reflex.
    /// Live RAM orb remains `SomaDroneFrame64` (inner magic). This is the header-only file pinout.
    pub fn pack_drone(
        timestamp_ms: u32,
        pos_x: f32,
        pos_y: f32,
        pos_z: f32,
        vel_x: f32,
        vel_y: f32,
        vel_z: f32,
        pitch_rad: f32,
        coherence_residual: f32,
        is_dark_window: bool,
        is_vslam_fail: bool,
        is_reflex_active: bool,
    ) -> Self {
        let mut flags = 0u64;
        if is_dark_window {
            flags |= FLAG_DRONE_DARK;
        }
        if is_vslam_fail {
            flags |= FLAG_DRONE_VSLAM_FAIL;
        }
        if is_reflex_active {
            flags |= FLAG_DRONE_REFLEX;
        }
        let t = timestamp_ms as f64 / 1000.0;
        let pos = [pos_x, pos_y, pos_z];
        let vel = [vel_x, vel_y, vel_z];
        let mut hasher = Sha256::new();
        hasher.update(&t.to_le_bytes());
        for f in &pos {
            hasher.update(&f.to_le_bytes());
        }
        for f in &vel {
            hasher.update(&f.to_le_bytes());
        }
        hasher.update(&pitch_rad.to_le_bytes());
        hasher.update(&coherence_residual.to_le_bytes());
        hasher.update(&flags.to_le_bytes());
        let digest = hasher.finalize();
        let mut proof = [0u8; 16];
        proof.copy_from_slice(&digest[..16]);
        Self {
            t,
            pos,
            vel,
            force_torque: pitch_rad,
            residual: coherence_residual,
            flags,
            proof,
        }
    }

    /// Plasma file slots (body 10): pos=last_gps_x/alt/tgt, vel=fp_ghz/L1_ghz/miss_m,
    /// force_torque=fp/L1, residual=miss_m. Flags bit0 blackout · bit1 miss · bit2 GPS-held.
    /// STREAM3 — 20 Hz pinout. Not the 128 B HGV Forge cache.
    pub fn pack_plasma(
        timestamp_ms: u32,
        last_gps_x_m: f32,
        altitude_m: f32,
        last_gps_tgt_m: f32,
        fp_ghz: f32,
        l1_ghz: f32,
        miss_m: f32,
        fp_over_l1: f32,
        miss_repeat_m: f32,
        is_blackout: bool,
        is_miss: bool,
        is_gps_held: bool,
    ) -> Self {
        let mut flags = 0u64;
        if is_blackout {
            flags |= FLAG_PLASMA_BLACKOUT;
        }
        if is_miss {
            flags |= FLAG_PLASMA_MISS;
        }
        if is_gps_held {
            flags |= FLAG_PLASMA_GPS_HELD;
        }
        let t = timestamp_ms as f64 / 1000.0;
        let pos = [last_gps_x_m, altitude_m, last_gps_tgt_m];
        let vel = [fp_ghz, l1_ghz, miss_m];
        let mut hasher = Sha256::new();
        hasher.update(&t.to_le_bytes());
        for f in &pos {
            hasher.update(&f.to_le_bytes());
        }
        for f in &vel {
            hasher.update(&f.to_le_bytes());
        }
        hasher.update(&fp_over_l1.to_le_bytes());
        hasher.update(&miss_repeat_m.to_le_bytes());
        hasher.update(&flags.to_le_bytes());
        let digest = hasher.finalize();
        let mut proof = [0u8; 16];
        proof.copy_from_slice(&digest[..16]);
        Self {
            t,
            pos,
            vel,
            force_torque: fp_over_l1,
            residual: miss_repeat_m,
            flags,
            proof,
        }
    }

    /// Fusion pit slots (body 11): pos=flux/beta_eff/time_s, vel=xenon_worth/pit_hours/core_age_days,
    /// force_torque=delta_rho, residual=base_rho. Flags bit0 prompt-critical · bit1 pit-survived.
    /// STREAM4 — reactor hours stay a terminal frame; no hour loop.
    pub fn pack_fusion(
        t_s: f64,
        flux: f32,
        beta_eff: f32,
        time_s: f32,
        xenon_worth: f32,
        pit_hours: f32,
        core_age_days: f32,
        delta_rho: f32,
        base_rho: f32,
        is_prompt_critical: bool,
        is_pit_survived: bool,
    ) -> Self {
        let mut flags = 0u64;
        if is_prompt_critical {
            flags |= FLAG_FUSION_PROMPT;
        }
        if is_pit_survived {
            flags |= FLAG_FUSION_SURVIVED;
        }
        let pos = [flux, beta_eff, time_s];
        let vel = [xenon_worth, pit_hours, core_age_days];
        let mut hasher = Sha256::new();
        hasher.update(&t_s.to_le_bytes());
        for f in &pos {
            hasher.update(&f.to_le_bytes());
        }
        for f in &vel {
            hasher.update(&f.to_le_bytes());
        }
        hasher.update(&delta_rho.to_le_bytes());
        hasher.update(&base_rho.to_le_bytes());
        hasher.update(&flags.to_le_bytes());
        let digest = hasher.finalize();
        let mut proof = [0u8; 16];
        proof.copy_from_slice(&digest[..16]);
        Self {
            t: t_s,
            pos,
            vel,
            force_torque: delta_rho,
            residual: base_rho,
            flags,
            proof,
        }
    }

    /// STREAM1 mycelial file slots (body 8): pos=health/density/percolation,
    /// vel=delivered/conductance/tilling, force_torque=delivered (repeat),
    /// residual=percolation (repeat). Flags bit0 fragmented · bit1 below-percolation.
    /// Live 8×f64 orb is SPECTRA MycelialState — not this envelope.
    pub fn pack_mycelial(
        timestamp_ms: u32,
        health_index: f32,
        hyphal_density: f32,
        percolation_index: f32,
        delivered_nutrient: f32,
        conductance_mean: f32,
        tilling_stress: f32,
        is_fragmented: bool,
        is_below_percolation: bool,
    ) -> Self {
        let mut flags = 0u64;
        if is_fragmented {
            flags |= FLAG_MYCELIAL_FRAGMENTED;
        }
        if is_below_percolation {
            flags |= FLAG_MYCELIAL_BELOW_PERC;
        }
        let t = timestamp_ms as f64 / 1000.0;
        let pos = [health_index, hyphal_density, percolation_index];
        let vel = [delivered_nutrient, conductance_mean, tilling_stress];
        let mut hasher = Sha256::new();
        hasher.update(&t.to_le_bytes());
        for f in &pos {
            hasher.update(&f.to_le_bytes());
        }
        for f in &vel {
            hasher.update(&f.to_le_bytes());
        }
        hasher.update(&delivered_nutrient.to_le_bytes());
        hasher.update(&percolation_index.to_le_bytes());
        hasher.update(&flags.to_le_bytes());
        let digest = hasher.finalize();
        let mut proof = [0u8; 16];
        proof.copy_from_slice(&digest[..16]);
        Self {
            t,
            pos,
            vel,
            force_torque: delivered_nutrient,
            residual: percolation_index,
            flags,
            proof,
        }
    }

    /// STREAM5 compounding mill slots (body 32): pos=acc_shear/potency/dissolution,
    /// vel=viscosity/api/shear_rate, force_torque=acc_shear, residual=potency.
    /// Flags bit0 potency-collapsed · bit1 dissolution-stalled. Reserved BROTH001.
    pub fn pack_compounding(
        timestamp_ms: u32,
        accumulated_shear_stress_pa: f32,
        active_potency_pct: f32,
        dissolution_pct: f32,
        final_viscosity_pas: f32,
        final_api_concentration_kg_m3: f32,
        shear_rate_s1: f32,
        is_potency_collapsed: bool,
        is_dissolution_stalled: bool,
    ) -> Self {
        let mut flags = 0u64;
        if is_potency_collapsed {
            flags |= FLAG_COMPOUNDING_POTENCY_COLLAPSED;
        }
        if is_dissolution_stalled {
            flags |= FLAG_COMPOUNDING_DISSOLUTION_STALLED;
        }
        let t = timestamp_ms as f64 / 1000.0;
        let pos = [
            accumulated_shear_stress_pa,
            active_potency_pct,
            dissolution_pct,
        ];
        let vel = [
            final_viscosity_pas,
            final_api_concentration_kg_m3,
            shear_rate_s1,
        ];
        let mut hasher = Sha256::new();
        hasher.update(&t.to_le_bytes());
        for f in &pos {
            hasher.update(&f.to_le_bytes());
        }
        for f in &vel {
            hasher.update(&f.to_le_bytes());
        }
        hasher.update(&accumulated_shear_stress_pa.to_le_bytes());
        hasher.update(&active_potency_pct.to_le_bytes());
        hasher.update(&flags.to_le_bytes());
        let digest = hasher.finalize();
        let mut proof = [0u8; 16];
        proof.copy_from_slice(&digest[..16]);
        Self {
            t,
            pos,
            vel,
            force_torque: accumulated_shear_stress_pa,
            residual: active_potency_pct,
            flags,
            proof,
        }
    }

    /// STREAM2 vehicle slots (body 9): pos=μ / |y| / yaw from Pacejka + chassis_q.
    /// vel=vx, vy, yaw_rate from chassis_dq. force_torque=yaw · residual=|y|.
    /// Flags bit0 hydroplane · bit1 corner-lost · bit2 grip.
    /// 128 B Forge VehicleDynamicsState is the RAM cache line. This file is 64 B.
    pub fn pack_vehicle(
        timestamp_ms: u32,
        mu: f32,
        abs_y_m: f32,
        yaw_rad: f32,
        vel_x: f32,
        vel_y: f32,
        yaw_rate: f32,
        is_hydroplane: bool,
        is_corner_lost: bool,
        is_grip: bool,
    ) -> Self {
        let mut flags = 0u64;
        if is_hydroplane {
            flags |= FLAG_VEHICLE_HYDROPLANE;
        }
        if is_corner_lost {
            flags |= FLAG_VEHICLE_CORNER_LOST;
        }
        if is_grip {
            flags |= FLAG_VEHICLE_GRIP;
        }
        let t = timestamp_ms as f64 / 1000.0;
        let pos = [mu, abs_y_m, yaw_rad];
        let vel = [vel_x, vel_y, yaw_rate];
        let mut hasher = Sha256::new();
        hasher.update(&t.to_le_bytes());
        for f in &pos {
            hasher.update(&f.to_le_bytes());
        }
        for f in &vel {
            hasher.update(&f.to_le_bytes());
        }
        hasher.update(&yaw_rad.to_le_bytes());
        hasher.update(&abs_y_m.to_le_bytes());
        hasher.update(&flags.to_le_bytes());
        let digest = hasher.finalize();
        let mut proof = [0u8; 16];
        proof.copy_from_slice(&digest[..16]);
        Self {
            t,
            pos,
            vel,
            force_torque: yaw_rad,
            residual: abs_y_m,
            flags,
            proof,
        }
    }
}

/// Header + frames. Digest is SHA-256 of concatenated frame bytes (SOMA.md).
pub fn write_soma_file(body_id: u16, reserved: [u8; 8], frames: &[[u8; 64]]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(frames.len() * 64);
    for f in frames {
        payload.extend_from_slice(f);
    }
    let digest: [u8; 32] = Sha256::digest(&payload).into();
    let header = LastStateHeader64 {
        magic: MAGIC,
        spec_version: SPEC_VERSION,
        body_id,
        traj_count: 1,
        frame_count: frames.len() as u64,
        digest,
        reserved,
    };
    let mut bin = Vec::with_capacity(64 + payload.len());
    bin.extend_from_slice(&header.to_bytes());
    bin.extend_from_slice(&payload);
    bin
}

/// Live RAM drone orb (Dark Window flight demo). Not the .soma.bin envelope.
/// Inner magic is historical; do not write these bytes as file frames.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct SomaDroneFrame64 {
    pub magic: [u8; 4],
    pub timestamp_ms: u32,
    pub position_xyz: [f32; 3],
    pub velocity_xyz: [f32; 3],
    pub attitude_rpy: [f32; 3],
    pub coherence_residual: f32,
    pub flags: u32,
    pub checksum: [u8; 12],
}

const _: () = assert!(std::mem::size_of::<SomaDroneFrame64>() == 64);

impl SomaDroneFrame64 {
    pub fn pack(
        timestamp_ms: u32,
        pos: [f32; 3],
        vel: [f32; 3],
        rpy: [f32; 3],
        coherence_residual: f32,
        is_dark_window: bool,
        coherence_fail: bool,
        reflex_active: bool,
    ) -> [u8; 64] {
        let mut flags = 0u32;
        if is_dark_window {
            flags |= 1 << 0;
        }
        if coherence_fail {
            flags |= 1 << 1;
        }
        if reflex_active {
            flags |= 1 << 2;
        }

        let mut hasher = Sha256::new();
        hasher.update(&timestamp_ms.to_le_bytes());
        for f in &pos {
            hasher.update(&f.to_le_bytes());
        }
        for f in &vel {
            hasher.update(&f.to_le_bytes());
        }
        for f in &rpy {
            hasher.update(&f.to_le_bytes());
        }
        hasher.update(&coherence_residual.to_le_bytes());
        hasher.update(&flags.to_le_bytes());
        let digest = hasher.finalize();

        let mut checksum = [0u8; 12];
        checksum.copy_from_slice(&digest[..12]);

        let frame = Self {
            magic: MAGIC,
            timestamp_ms,
            position_xyz: pos,
            velocity_xyz: vel,
            attitude_rpy: rpy,
            coherence_residual,
            flags,
            checksum,
        };

        let mut bytes = [0u8; 64];
        unsafe {
            std::ptr::copy_nonoverlapping(&frame as *const _ as *const u8, bytes.as_mut_ptr(), 64);
        }
        bytes
    }
}

/// Humanoid file-frame packer. Bytes are `LastStateFrame64`, not a second magic-in-frame layout.
pub struct SomaHumanoidFrame64;

impl SomaHumanoidFrame64 {
    pub fn pack(
        timestamp_ms: u32,
        com_xyz: [f32; 3],
        velocity_xyz: [f32; 3],
        attitude_rpy: [f32; 3],
        zmp_margin_m: f32,
        is_dark_window: bool,
        is_buckle: bool,
        is_reflex_grasp: bool,
    ) -> [u8; 64] {
        LastStateFrame64::pack_humanoid(
            timestamp_ms,
            com_xyz,
            velocity_xyz,
            attitude_rpy[1],
            zmp_margin_m,
            is_dark_window,
            is_buckle,
            is_reflex_grasp,
        )
        .to_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_and_frame_are_64() {
        assert_eq!(std::mem::size_of::<LastStateHeader64>(), 64);
        assert_eq!(std::mem::size_of::<LastStateFrame64>(), 64);
    }

    #[test]
    fn humanoid_file_has_header_body_id_and_frame_digest() {
        let frame = LastStateFrame64::pack_humanoid(
            10,
            [0.0, 0.0, 0.9],
            [0.1, 0.0, 0.0],
            0.02,
            0.04,
            true,
            false,
            true,
        );
        let bytes = frame.to_bytes();
        let decoded = LastStateFrame64::from_bytes(bytes);
        assert!((decoded.pos[2] - 0.9).abs() < 1e-6);
        assert_eq!(decoded.flags & FLAG_DARK_WINDOW, FLAG_DARK_WINDOW);
        assert_eq!(decoded.flags & FLAG_HUMANOID_BUCKLE, 0);
        assert_eq!(decoded.flags & FLAG_HUMANOID_REFLEX, FLAG_HUMANOID_REFLEX);

        let file = write_soma_file(BODY_HUMANOID, *b"HUMANOID", &[bytes]);
        assert_eq!(&file[0..4], b"SOMA");
        let body = u16::from_le_bytes([file[6], file[7]]);
        assert_eq!(body, BODY_HUMANOID);
        let nframes = u64::from_le_bytes(file[16..24].try_into().unwrap());
        assert_eq!(nframes, 1);
        let digest = Sha256::digest(&file[64..]);
        assert_eq!(&file[24..56], digest.as_slice());
        // File frames do not repeat header magic.
        assert_ne!(&file[64..68], b"SOMA");
    }

    #[test]
    fn hand_file_has_header_body_id_and_frame_digest() {
        let frame = LastStateFrame64::pack_hand(
            10, 12.0, 9.0, 0.007, 1.047, 0.6, 0.01, 0.22, 0.04, true, false,
        );
        let bytes = frame.to_bytes();
        let decoded = LastStateFrame64::from_bytes(bytes);
        assert!((decoded.pos[0] - 12.0).abs() < 1e-5);
        assert_eq!(decoded.flags & FLAG_HAND_OVERSTRETCH, FLAG_HAND_OVERSTRETCH);
        assert_eq!(decoded.flags & FLAG_HAND_PAD_SLIP, 0);

        let file = write_soma_file(BODY_HAND, *b"HAND0001", &[bytes]);
        assert_eq!(&file[0..4], b"SOMA");
        let body = u16::from_le_bytes([file[6], file[7]]);
        assert_eq!(body, BODY_HAND);
        let digest = Sha256::digest(&file[64..]);
        assert_eq!(&file[24..56], digest.as_slice());
        assert_ne!(&file[64..68], b"SOMA");
    }

    #[test]
    fn ocean_file_has_header_body_id_and_frame_digest() {
        let frame = LastStateFrame64::pack_ocean(
            10, 4345.0, 43.79, 25.7, 5759.2, 6526.3, 80.0, 3200.0, 4000.0, false, true,
        );
        let bytes = frame.to_bytes();
        let decoded = LastStateFrame64::from_bytes(bytes);
        assert!((decoded.pos[0] - 4345.0).abs() < 1e-3);
        assert_eq!(decoded.flags & FLAG_OCEAN_CRUSHED, 0);
        assert_eq!(decoded.flags & FLAG_OCEAN_STARVED, FLAG_OCEAN_STARVED);

        let file = write_soma_file(BODY_OCEAN, *b"OCEAN001", &[bytes]);
        assert_eq!(&file[0..4], b"SOMA");
        let body = u16::from_le_bytes([file[6], file[7]]);
        assert_eq!(body, BODY_OCEAN);
        let digest = Sha256::digest(&file[64..]);
        assert_eq!(&file[24..56], digest.as_slice());
        assert_ne!(&file[64..68], b"SOMA");
    }

    #[test]
    fn drone_file_has_header_body_id_and_frame_digest() {
        let frame = LastStateFrame64::pack_drone(
            10, 0.0, 0.0, 5.0, 0.0, 0.0, -0.4, 0.02, 2.1, true, true, false,
        );
        let bytes = frame.to_bytes();
        let decoded = LastStateFrame64::from_bytes(bytes);
        assert!((decoded.pos[2] - 5.0).abs() < 1e-5);
        assert_eq!(decoded.flags & FLAG_DRONE_DARK, FLAG_DRONE_DARK);
        assert_eq!(decoded.flags & FLAG_DRONE_VSLAM_FAIL, FLAG_DRONE_VSLAM_FAIL);
        assert_eq!(decoded.flags & FLAG_DRONE_REFLEX, 0);

        let file = write_soma_file(BODY_DRONE, *b"DRONE001", &[bytes]);
        assert_eq!(&file[0..4], b"SOMA");
        let body = u16::from_le_bytes([file[6], file[7]]);
        assert_eq!(body, BODY_DRONE);
        let digest = Sha256::digest(&file[64..]);
        assert_eq!(&file[24..56], digest.as_slice());
        assert_ne!(&file[64..68], b"SOMA");
    }

    #[test]
    fn plasma_file_has_header_body_id_and_frame_digest() {
        /* STREAM3 body 10 */
        let frame = LastStateFrame64::pack_plasma(
            50, 120.0, 0.0, 80.0, 2.1, 1.575, 55.0, 1.33, 55.0, true, true, false,
        );
        let bytes = frame.to_bytes();
        let decoded = LastStateFrame64::from_bytes(bytes);
        assert!((decoded.pos[0] - 120.0).abs() < 1e-3);
        assert_eq!(decoded.flags & FLAG_PLASMA_BLACKOUT, FLAG_PLASMA_BLACKOUT);
        assert_eq!(decoded.flags & FLAG_PLASMA_MISS, FLAG_PLASMA_MISS);
        assert_eq!(decoded.flags & FLAG_PLASMA_GPS_HELD, 0);

        let file = write_soma_file(BODY_PLASMA, *b"PLASMA01", &[bytes]);
        assert_eq!(&file[0..4], b"SOMA");
        let body = u16::from_le_bytes([file[6], file[7]]);
        assert_eq!(body, BODY_PLASMA);
        let digest = Sha256::digest(&file[64..]);
        assert_eq!(&file[24..56], digest.as_slice());
        assert_ne!(&file[64..68], b"SOMA");
    }

    #[test]
    fn fusion_file_has_header_body_id_and_frame_digest() {
        /* STREAM4 body 11 — terminal pit row, not an hour loop */
        let frame = LastStateFrame64::pack_fusion(
            134011.0, 1.24e13, 0.005236, 134011.0, -0.008, 25.2, 221.5, 0.056, 0.214, true, false,
        );
        let bytes = frame.to_bytes();
        let decoded = LastStateFrame64::from_bytes(bytes);
        assert!((decoded.pos[1] - 0.005236).abs() < 1e-6);
        assert_eq!(decoded.flags & FLAG_FUSION_PROMPT, FLAG_FUSION_PROMPT);
        assert_eq!(decoded.flags & FLAG_FUSION_SURVIVED, 0);

        let file = write_soma_file(BODY_FUSION, *b"FUSION01", &[bytes]);
        assert_eq!(&file[0..4], b"SOMA");
        let body = u16::from_le_bytes([file[6], file[7]]);
        assert_eq!(body, BODY_FUSION);
        let digest = Sha256::digest(&file[64..]);
        assert_eq!(&file[24..56], digest.as_slice());
        assert_ne!(&file[64..68], b"SOMA");
    }

    #[test]
    fn compounding_file_has_header_body_id_and_frame_digest() {
        /* STREAM5 body 32 — mill pinout, reserved BROTH001 */
        let frame = LastStateFrame64::pack_compounding(
            10, 697.28, 0.0, 93.1, 0.04, 12.0, 120.0, true, false,
        );
        let bytes = frame.to_bytes();
        let decoded = LastStateFrame64::from_bytes(bytes);
        assert!((decoded.pos[0] - 697.28).abs() < 1e-2);
        assert_eq!(
            decoded.flags & FLAG_COMPOUNDING_POTENCY_COLLAPSED,
            FLAG_COMPOUNDING_POTENCY_COLLAPSED
        );
        assert_eq!(decoded.flags & FLAG_COMPOUNDING_DISSOLUTION_STALLED, 0);

        let file = write_soma_file(BODY_COMPOUNDING, *b"BROTH001", &[bytes]);
        assert_eq!(&file[0..4], b"SOMA");
        let body = u16::from_le_bytes([file[6], file[7]]);
        assert_eq!(body, BODY_COMPOUNDING);
        let digest = Sha256::digest(&file[64..]);
        assert_eq!(&file[24..56], digest.as_slice());
        assert_ne!(&file[64..68], b"SOMA");
    }

    #[test]
    fn vehicle_file_has_header_body_id_and_frame_digest() {
        /* STREAM2 body 9 — 64 B file, not the 128 B Forge cache line */
        let frame = LastStateFrame64::pack_vehicle(
            4999, 0.148, 3.2, 0.12, 41.0, 0.4, 0.02, true, true, false,
        );
        let bytes = frame.to_bytes();
        let decoded = LastStateFrame64::from_bytes(bytes);
        assert!((decoded.pos[0] - 0.148).abs() < 1e-5);
        assert_eq!(decoded.flags & FLAG_VEHICLE_HYDROPLANE, FLAG_VEHICLE_HYDROPLANE);
        assert_eq!(decoded.flags & FLAG_VEHICLE_CORNER_LOST, FLAG_VEHICLE_CORNER_LOST);
        assert_eq!(decoded.flags & FLAG_VEHICLE_GRIP, 0);

        let file = write_soma_file(BODY_VEHICLE, *b"VEHICLE1", &[bytes]);
        assert_eq!(&file[0..4], b"SOMA");
        let body = u16::from_le_bytes([file[6], file[7]]);
        assert_eq!(body, BODY_VEHICLE);
        let digest = Sha256::digest(&file[64..]);
        assert_eq!(&file[24..56], digest.as_slice());
        assert_ne!(&file[64..68], b"SOMA");
    }
}
