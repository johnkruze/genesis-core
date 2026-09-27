// Terran (Soil/Stone): Boussinesq Soil Mechanics + Robot Contact + Seeding
// THE TERRAN TRUTH: At what mass does the substrate reject the body?
//
// THE EMBODIMENT: A robot stands on living soil. Its weight creates a stress
// bulb that propagates downward — Boussinesq's 1885 equation. If the stress
// exceeds the soil's yield, compaction occurs. Compaction destroys pore space,
// kills mycorrhizal hyphae, reduces infiltration, and prevents seed emergence.
//
// But glomalin — the glycoprotein that mycorrhizal fungi exude — increases
// aggregate stability and yield stress. Biology strengthens the soil against
// the machine. This is the coupling to Song 4.

/// Fresh water — hydroplane / tire-terrain meridian.
pub const RHO_FRESHWATER: f64 = 997.0;

// ─── SOIL TYPES ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub enum SoilType {
    Sand,
    Loam,
    Clay,
    Andisol,  // Volcanic — highest glomalin response
}

impl SoilType {
    pub fn base_yield_stress(&self) -> f64 {
        match self {
            SoilType::Sand => 30_000.0,    // Pa — weakest
            SoilType::Loam => 60_000.0,    // Pa
            SoilType::Clay => 100_000.0,   // Pa — strongest dry
            SoilType::Andisol => 45_000.0, // Pa — moderate, but responds to glomalin
        }
    }

    pub fn field_capacity(&self) -> f64 {
        match self {
            SoilType::Sand => 0.10,
            SoilType::Loam => 0.27,
            SoilType::Clay => 0.40,
            SoilType::Andisol => 0.35,
        }
    }

    pub fn wilting_point(&self) -> f64 {
        match self {
            SoilType::Sand => 0.05,
            SoilType::Loam => 0.12,
            SoilType::Clay => 0.22,
            SoilType::Andisol => 0.15,
        }
    }

    /// How much glomalin increases yield stress per mg/g
    pub fn glomalin_coefficient(&self) -> f64 {
        match self {
            SoilType::Sand => 200.0,     // Pa per mg/g
            SoilType::Loam => 350.0,
            SoilType::Clay => 150.0,     // already strong, less benefit
            SoilType::Andisol => 500.0,  // volcanic soils love glomalin
        }
    }

    pub fn youngs_modulus(&self) -> f64 {
        match self {
            SoilType::Sand => 0.5e6,
            SoilType::Loam => 5.0e6,
            SoilType::Clay => 20.0e6,
            SoilType::Andisol => 3.0e6,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            SoilType::Sand => "sand",
            SoilType::Loam => "loam",
            SoilType::Clay => "clay",
            SoilType::Andisol => "andisol",
        }
    }
}

// ─── ROBOT CONTACT MODEL ────────────────────────────────────────

pub const GRAVITY: f64 = 9.81;

#[derive(Debug, Clone, Copy)]
pub enum Locomotion {
    Wheeled,   // small footprint, high pressure
    Tracked,   // moderate footprint
    Legged,    // variable footprint per step
    Drone,     // zero contact pressure (but downwash)
}

impl Locomotion {
    pub fn as_str(&self) -> &'static str {
        match self {
            Locomotion::Wheeled => "wheeled",
            Locomotion::Tracked => "tracked",
            Locomotion::Legged => "legged",
            Locomotion::Drone => "drone",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RobotContact {
    pub mass_kg: f64,
    pub footprint_m2: f64,
    pub locomotion: Locomotion,
}

impl RobotContact {
    /// Contact pressure at the surface (Pa)
    pub fn surface_pressure(&self) -> f64 {
        match self.locomotion {
            Locomotion::Drone => 0.0, // no ground contact
            _ => self.mass_kg * GRAVITY / self.footprint_m2,
        }
    }
}

// ─── BOUSSINESQ SOIL STRESS ─────────────────────────────────────

/// Boussinesq (1885): vertical stress from a point load at depth z, radial distance r.
/// sigma_z = (3P / 2pi) * z^3 / (r^2 + z^2)^(5/2)
pub fn boussinesq_stress(load_n: f64, z: f64, r: f64) -> f64 {
    if z <= 0.0 { return 0.0; }
    let denom = (r * r + z * z).powf(2.5);
    if denom < 1e-20 { return 0.0; }
    (3.0 * load_n / (2.0 * std::f64::consts::PI)) * z.powi(3) / denom
}

/// Approximate the stress from a distributed load (footprint) by integrating
/// Boussinesq over a circular equivalent area.
/// Returns max stress at depth z directly below center (r=0).
pub fn distributed_stress(pressure_pa: f64, footprint_m2: f64, z: f64) -> f64 {
    if z <= 0.0 { return pressure_pa; } // at surface, stress = applied pressure
    let radius = (footprint_m2 / std::f64::consts::PI).sqrt();
    // Newmark's formula for uniform circular load:
    // sigma_z = P * (1 - z^3 / (radius^2 + z^2)^(3/2))
    let ratio = z.powi(3) / (radius * radius + z * z).powf(1.5);
    pressure_pa * (1.0 - ratio)
}

// ─── SOIL STATE ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SoilProfile {
    pub soil_type: SoilType,
    pub moisture: f64,         // volumetric water content (0-1)
    pub glomalin_mg_g: f64,   // glomalin content (mg/g soil)
    pub compaction: f64,       // 0.0 = pristine, 1.0 = fully compacted
    pub depth_layers: usize,   // number of depth layers to evaluate
}

impl SoilProfile {
    /// Effective yield stress, modified by moisture and glomalin.
    /// Wet soil is weaker. Glomalin strengthens.
    pub fn effective_yield_stress(&self) -> f64 {
        let base = self.soil_type.base_yield_stress();
        // Moisture weakens soil (linear reduction above wilting point)
        let fc = self.soil_type.field_capacity();
        let moisture_factor = if self.moisture > fc {
            0.3 // saturated soil is very weak
        } else {
            1.0 - 0.5 * (self.moisture / fc)
        };
        // Glomalin strengthens (biology enables machinery)
        let glomalin_boost = self.glomalin_mg_g * self.soil_type.glomalin_coefficient();
        (base + glomalin_boost) * moisture_factor
    }

    /// Check if a robot's contact would cause compaction at a given depth.
    /// Returns the compaction increment (0 if no compaction).
    pub fn compaction_at_depth(&self, robot: &RobotContact, depth_m: f64) -> f64 {
        let pressure = robot.surface_pressure();
        if pressure <= 0.0 { return 0.0; } // drones don't compact
        let stress = distributed_stress(pressure, robot.footprint_m2, depth_m);
        let yield_stress = self.effective_yield_stress();
        if stress > yield_stress {
            // Plastic compaction: proportional to overstress
            let overstress_ratio = (stress - yield_stress) / yield_stress;
            (overstress_ratio * 0.1).min(0.5) // cap at 0.5 per step
        } else {
            0.0
        }
    }

    /// Evaluate compaction across the depth profile.
    /// Returns (max_compaction_increment, compaction_depth_m).
    pub fn evaluate_contact(&self, robot: &RobotContact) -> (f64, f64) {
        let max_depth = 1.0; // evaluate down to 1m
        let dz = max_depth / self.depth_layers as f64;
        let mut max_compact = 0.0_f64;
        let mut compact_depth = 0.0_f64;
        for i in 0..self.depth_layers {
            let z = (i as f64 + 0.5) * dz;
            let c = self.compaction_at_depth(robot, z);
            if c > max_compact {
                max_compact = c;
                compact_depth = z;
            }
        }
        (max_compact, compact_depth)
    }
}

// ─── SEEDING MODEL ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct SeedingResult {
    pub depth_ok: bool,        // planted at correct depth?
    pub moisture_ok: bool,     // adequate moisture for germination?
    pub compaction_ok: bool,   // soil above seed is uncompacted enough for emergence?
    pub emerged: bool,         // seed emerged?
}

/// Evaluate whether a seed can emerge given soil conditions.
/// seed_depth: planting depth (m), target_depth: ideal depth (m).
pub fn evaluate_seeding(
    soil: &SoilProfile,
    seed_depth: f64,
    target_depth: f64,
    depth_tolerance: f64,
) -> SeedingResult {
    let depth_ok = (seed_depth - target_depth).abs() < depth_tolerance;
    let moisture_ok = soil.moisture > soil.soil_type.wilting_point()
        && soil.moisture < soil.soil_type.field_capacity() * 1.3;
    // Compaction above 0.6 prevents emergence (roots can't push through)
    let compaction_ok = soil.compaction < 0.6;
    let emerged = depth_ok && moisture_ok && compaction_ok;

    SeedingResult { depth_ok, moisture_ok, compaction_ok, emerged }
}

// ─── MOISTURE DYNAMICS ─────────────────────────────────────────

/// Update moisture based on compaction (reduces infiltration).
pub fn moisture_step(soil: &mut SoilProfile, rainfall_rate: f64, dt: f64) {
    let infiltration_factor = 1.0 - soil.compaction * 0.8; // compaction reduces infiltration
    let infiltration = rainfall_rate * infiltration_factor * dt;
    let evaporation = 0.001 * soil.moisture * dt; // slow evaporative loss
    soil.moisture = (soil.moisture + infiltration - evaporation)
        .clamp(0.0, 0.95);
}
