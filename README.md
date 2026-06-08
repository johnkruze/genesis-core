# genesis-core

**The G^G physics engine. 13 domains, 113 Monte Carlo simulations, pure CPU Rust. No external physics libraries.**

[![Website](https://img.shields.io/badge/Web-zerotrustphysics.com-000000?style=flat-square)](https://zerotrustphysics.com)
[![Dataset](https://img.shields.io/badge/HuggingFace-gg--physical--ground--truth-FF6B00?style=flat-square)](https://huggingface.co/datasets/johnkruze/gg-physical-ground-truth)
[![Paper](https://img.shields.io/badge/Paper-Zero_Trust_Physics-1a1a2e?style=flat-square)](https://zerotrustphysics.com/Zero-Trust-Physics.pdf)
[![Kernel](https://img.shields.io/badge/FFI_Kernel-ztp--runtime-555555?style=flat-square)](https://github.com/johnkruze/ztp-runtime)

---

Every equation in this codebase traces to a verified mechanical reference. Every integration step is sealed with a running SHA-256 hash chain. The corpus it generates is on [HuggingFace](https://huggingface.co/datasets/johnkruze/gg-physical-ground-truth) and anchored on the Internet Computer.

```bash
cargo run --release --bin corpus_sweep -- 1000 --export data/corpus/
```

---

## Physics Modules

Thirteen physical domains, each implemented from first principles in `src/physics/`:

| Module | Physics |
|--------|---------|
| `orbital.rs` | Quaternion attitude, J2–J4 geopotential, gyroscopic coupling, Yoshida symplectic |
| `mars_edl.rs` | CO₂ atmospheric drag, EDL trajectory, powered descent suicide burn |
| `marine.rs` | Buoyancy, pressure gradients, GPS-denied 6-DOF nav, IMU drift |
| `terran.rs` | Boussinesq soil stress, glomalin coupling, compaction dynamics |
| `mycelial.rs` | Hyphal conductance, Kirchhoff flow distribution, percolation thresholds |
| `atheric.rs` | Shannon capacity, Friis path loss, frequency hopping, clock drift |
| `rubble.rs` (Asteroid) | O(N²) N-body gravity, Hookean contact, Metal GPU compute |
| `plutonian.rs` | Entropy decay, phase crystallization, deep-cold thermodynamics |
| `ephemeris.rs` (Celestial) | Yoshida 4th-order symplectic, N-body ephemeris, 1PN corrections |
| `energy.rs` | Swing equation, power flow dispatch, renewable intermittency |
| `tokamak.rs` | MHD plasma stability, Z-axis shear, beta-limit disruption, coil quench |
| `swing.rs` | Rotor angle swing, PLL tracking error, governor droop, IBR penetration |
| `reactor.rs` | Fission core neutron kinetics, 6-group delayed precursors, Xenon-135 poisoning |

---

## Simulation Library

113 Monte Carlo binaries in `src/bin/`. Each one models a specific physical failure mode from first principles.

### Humanoid & Legged Systems

| Binary | What it models |
|--------|---------------|
| `humanoid_impedance_monte_carlo` | ★ Centroidal momentum, active joint-impedance, slip cascade (50D) |
| `humanoid_bipedal_monte_carlo` | Bipedal gait stability across terrain friction sweep |
| `humanoid_gait_euler` | Euler gait with yaw accumulation and gyroscopic coupling |
| `humanoid_actuator_backlash_gait` | Gear backlash propagation through gait cycle |
| `humanoid_sensor_vibration_chatter` | IMU chatter under sustained joint vibration |
| `humanoid_yaw_accumulation` | Long-duration yaw drift from asymmetric ground contact |
| `dynamic_zmp_wind_shear` | Zero-Moment Point stability under wind shear tensor |
| `ankle_inversion_trip` | Ankle inversion failure on uneven terrain — exact fall geometry |
| `tendon_snap` | Achilles tendon creep and failure threshold under repeated load |
| `quadruped_shale_monte_carlo` | Quadruped contact on fractured shale substrate |
| `quadruped_thermal_monte_carlo` | Actuator thermal saturation during sustained quadruped gait |
| `quadruped_vibration_monte_carlo` | Structural resonance in quadruped frame under rough terrain |
| `quadruped_recoil_monte_carlo` | Recoil-induced gait destabilization on quadruped weapons platform |
| `quadruped_leg_thermal_sink` | Heat dissipation path through leg structure to ground contact |

### Ground Vehicles & Heavy Logistics

| Binary | What it models |
|--------|---------------|
| `vehicle_hydroplane_monte_carlo` | ★ Pacejka tire dynamics, aquaplaning boundary, ESC divergence (51D) |
| `autonomous_car_monte_carlo` | Full AV dynamics under sensor degradation |
| `titanhauler_monte_carlo` | Heavy logistics chassis under extreme load and terrain |
| `vehicle_brake_fade` | Hydraulic brake fade under sustained thermal load |
| `vehicle_hydroplaning` | Single-event hydroplane onset — yaw rate at departure |
| `vehicle_suspension_harmonic` | Suspension resonance frequency under road profile sweep |
| `suspension_resonance` | Chassis resonance coupling to sensor payload |
| `tire_casing_hysteresis` | Tire casing thermal hysteresis under load cycling |
| `trailer_whip_resonance` | Trailer oscillation coupling to tractor yaw — jackknife boundary |
| `track_pin_galling` | Track pin wear and seizure under sustained lateral load |
| `landing_gear_hysteresis` | Aircraft landing gear load-displacement hysteresis |
| `amr_hydraulic_monte_carlo` | Autonomous mobile robot hydraulic system under pressure spike |

### Aerial & Drone Systems

| Binary | What it models |
|--------|---------------|
| `drone_canopy_monte_carlo` | ★ EW jamming, GPS spoofing, VRS onset, asymmetric rotor icing (59D) |
| `cargo_drone_vortex_ring_state` | Vortex Ring State onset boundary under payload variation |
| `uav_icing_stall` | Fixed-wing UAV icing progression to aerodynamic stall |
| `rotor_icing` | Per-rotor icing asymmetry and angular momentum coupling |
| `autonomous_wingman_flutter` | Aeroelastic flutter onset for autonomous wingman under g-loading |
| `wing_flutter_divergence` | Wing flutter divergence boundary — Mach vs. structural damping |
| `booster_fin_monte_carlo` | Rocket booster fin loads under max-Q and asymmetric thrust |
| `wake_resonance` | Wake turbulence resonance between formation aircraft |
| `mald_ew_monte_carlo` | Miniature Air-Launched Decoy under active electronic warfare |

### Marine & Subsurface

| Binary | What it models |
|--------|---------------|
| `marine_monte_carlo` | GPS-denied 6-DOF navigation, buoyancy, IMU drift |
| `submarine_monte_carlo` | Submarine pressure hull, depth excursion, ballast failure |
| `autonomous_boat_monte_carlo` | USV navigation under sea state and sensor degradation |
| `usv_hull_slam_hydrodynamics` | Hull slamming loads under wave impact — structural stress |
| `usv_diesel_thermal_runaway` | Diesel engine thermal runaway in confined hull |
| `uuv_pressure_stiction` | UUV control surface stiction under pressure cycling |
| `uuv_sonar_thermocline_refraction` | Sonar beam refraction across thermocline boundaries |
| `propeller_cavitation_noise_floor` | Propeller cavitation onset and acoustic noise floor |
| `nav_radar_sea_clutter_saturation` | Navigation radar saturation from sea clutter at low grazing angles |

### Orbital & Space

| Binary | What it models |
|--------|---------------|
| `orbital_tumble_monte_carlo` | ★ Spacecraft attitude recovery under asymmetric fuel depletion (20D) |
| `orbital_monte_carlo` | Full orbital mechanics with J2–J4 perturbations |
| `satellite_monte_carlo` | Satellite bus under combined thermal, attitude, and power constraints |
| `lunar_lander_monte_carlo` | Lunar descent with terrain-relative navigation and thruster uncertainty |
| `maven_monte_carlo` | MAVEN-class orbiter solar conjunction autonomous operations |
| `solar_flare_magnetometer` | Magnetometer saturation and recovery under X-class solar flare |
| `thermal_outgassing` | Outgassing pressure spike effect on attitude control in LEO |

### Hypersonic & Reentry

| Binary | What it models |
|--------|---------------|
| `hgv_plasma_monte_carlo` | ★ CoG migration, aeroshell ablation, static margin inversion (47D) |
| `hypersonic_plasma_blackout` | GPS/comm blackout boundary during plasma sheath reentry |
| `terminal_dive_shadow` | Terminal dive radar shadow and guidance lockout boundary |

### Mars & Planetary Entry

| Binary | What it models |
|--------|---------------|
| `mars_monte_carlo` | CO₂ atmosphere EDL — drag, parachute deploy, suicide burn boundary |

### Defense & Weapons Physics

| Binary | What it models |
|--------|---------------|
| `armor_spall_sensor_shearing` | Armor spall fragmentation patterns and sensor shearing thresholds |
| `blast_overpressure_imu` | IMU response under blast overpressure — sensor survival boundary |
| `gun_barrel_warp` | Gun barrel thermal warp under sustained fire rate |
| `stealth_thermal_warp` | Stealth coating thermal warp under aerodynamic heating |
| `wargame_tensor_integrator` | Multi-domain wargame tensor integration across simultaneous engagements |

### Energy & Grid

| Binary | What it models |
|--------|---------------|
| `energy_monte_carlo` | Power flow dispatch, swing equation, renewable intermittency |
| `ai_grid_blackout` | AI-managed grid under cascading failure — latency-induced desynchronization |
| `swing_monte_carlo` | Rotor angle swing and PLL tracking under inertia floor collapse |

### Nuclear & Plasma

| Binary | What it models |
|--------|---------------|
| `reactor_monte_carlo` | Neutron kinetics, Xenon-135 poisoning, prompt criticality boundary |
| `reactor_demo` | Reactor kinetics demonstrator with control rod extraction scenarios |
| `tokamak_monte_carlo` | Plasma confinement, Z-axis shear, coil quench, wall breach |

### Thermal Physics

| Binary | What it models |
|--------|---------------|
| `battery_thermal_runaway` | Li-ion cell thermal runaway propagation and BMS trip |
| `battery_sag` | Battery voltage sag under transient current demand |
| `thermal_expansion_drift` | Structural thermal expansion effect on sensor alignment |
| `thermal_lens_warp` | Optical lens thermal warp and focal point drift |
| `thermal_seal_friction` | Elastomeric seal friction change across thermal cycle |
| `thermal_sensor_starvation` | Sensor thermal starvation in shadow-cycling LEO orbit |
| `thermal_sight_saturation` | Thermal imaging sight saturation under IR-bright backgrounds |
| `ip67_heat_soak` | IP67-sealed enclosure heat soak — internal component limits |
| `tunnel_thermal_evac_monte_carlo` | Tunnel thermal evacuation dynamics under fire event |

### Mechanical & Actuator Failure

| Binary | What it models |
|--------|---------------|
| `actuator_metallurgic_shear` | Actuator shaft metallurgic shear under torque spike |
| `actuator_tribology` | Tribological wear progression in actuator bearings |
| `cable_stretch_backlash` | Control cable stretch and backlash under repeated load |
| `gear_galling` | Gear surface galling initiation threshold under combined load |
| `hydraulic_fluid_compressibility` | Hydraulic fluid compressibility effect on control bandwidth |
| `hydraulic_shear_stiction` | Hydraulic actuator stiction under cold-start shear |
| `pneumatic_line_resonance` | Pneumatic line resonance under rapid valve cycling |
| `slip_ring_vibration` | Slip ring contact degradation under sustained vibration |
| `sensor_gimbal_resonance` | Sensor gimbal resonance coupling to platform dynamics |
| `cg_shift_resonance` | CG shift resonance coupling during asymmetric fuel burn |
| `acoustic_feedback` | Acoustic feedback path between vibration source and sensor |
| `asymmetric_part_drop` | CG shift and attitude disturbance from asymmetric payload release |

### RF, Lidar & Sensor Degradation

| Binary | What it models |
|--------|---------------|
| `atheric_monte_carlo` | RF Shannon capacity, Friis path loss, frequency hopping |
| `doppler_lidar_rain_scatter` | Doppler lidar return scatter in heavy rain |
| `lidar_water_refraction` | Lidar beam refraction at air-water interface |
| `optical_salt_occlusion` | Optical sensor salt spray occlusion in marine environment |
| `radar_mud_attenuation` | Radar signal attenuation through mud and wet soil |
| `vslam_smoke_occlusion` | V-SLAM feature tracking failure under smoke obscuration |

### Celestial, Asteroid & Planetary

| Binary | What it models |
|--------|---------------|
| `celestial_monte_carlo` | N-body celestial mechanics, Yoshida symplectic, 1PN corrections |
| `asteroid_monte_carlo` | O(N²) N-body gravity + Hookean contact, Metal GPU compute |
| `plutonian_monte_carlo` | Entropy decay and phase crystallization at deep-cold temperatures |

### Biological & Ecosystem

| Binary | What it models |
|--------|---------------|
| `mycelial_monte_carlo` | Hyphal conductance, Kirchhoff flow, percolation thresholds |

---

## G^G Product Binaries

Five commercial physics domains with open baseline data on HuggingFace:

```bash
cargo run --release --bin humanoid_impedance_monte_carlo -- --export data/products/
cargo run --release --bin vehicle_hydroplane_monte_carlo -- --export data/products/
cargo run --release --bin drone_canopy_monte_carlo -- --export data/products/
cargo run --release --bin orbital_tumble_monte_carlo -- --export data/products/
cargo run --release --bin hgv_plasma_monte_carlo -- --export data/products/
```

---

## Corpus Pipeline

Full corpus sweep across all domains:

```bash
# Run N trajectories, export to data/corpus/
cargo run --release --bin corpus_sweep -- 1000 --export data/corpus/

# Body sweep — all 8 body daemons sequentially
cargo run --release --bin body_sweep -- 1000 --root ../../data/corpus
```

Integration rates are live-measured per domain. The `corpus_sweep` binary reports current rates — do not trust constants in source.

---

## Integration Rates

*Live-measured on Apple Silicon M-series. Run corpus_sweep for current numbers.*

| Domain | Rate |
|--------|:----:|
| Energy Grid | 186,060/s |
| Tokamak | ~167,500/s |
| Atheric | 15,830/s |
| Terran | 10,750/s |
| Swing Grid | ~7,777/s |
| Reactor | ~5,578/s |
| Mycelial | 760/s |
| Orbital | 353/s |
| Mars | 144/s |
| Plutonian | 71/s |
| Celestial | 71/s |
| Marine | 11/s |
| Asteroid | 9/s |

---

## Verification

Every trajectory carries a running SHA-256 proof chain anchored on the Internet Computer:

- On-chain canister: `ad7wi-4aaaa-aaaad-aeijq-cai`
- Open dataset: [huggingface.co/datasets/johnkruze/gg-physical-ground-truth](https://huggingface.co/datasets/johnkruze/gg-physical-ground-truth)

---

## License

[John Kruze Commercial License v1](LICENSE)

Non-commercial use permitted. Commercial use and custom simulation runs require direct engagement.

→ [zerotrustphysics.com](https://zerotrustphysics.com) · kruze@zerotrustphysics.com

---

*John Kruze · [zerotrustphysics.com](https://zerotrustphysics.com)*
