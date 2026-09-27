//! Syntropic Forge — invert a desired physical end-state into Physical Sheet Music.
//!
//! The Recursive Forge (`src/bin/*_monte_carlo.rs`) sweeps forward: sample, integrate,
//! seal. This module walks the other way: a [`StrangeAttractor`] names a measurable
//! end-state, a [`Cultivator`] shoots a [`DiscreteDynamics`] plant with finite-difference
//! gradients, and the result is a reconstructible [`SheetMusic`] timeline of actuations.
//! [`stamp`] compresses that score into a 64-byte last-state frame (hand pinout) so the
//! body can carry it into the Dark Window.
//!
//! Sample plants: tactile grasp force (`evaluate_grasp_dynamics`) and the
//! 5-finger tendon chain (`evaluate_hand_tendon_dynamics`). Same dt = 0.001 s body-time
//! as `src/bin/grasp_loop_trace.rs` / `hand_loop_trace.rs`. SPECTRA is not on
//! this path. No autodiff crates; no adjoint of the C FFI domains yet.

pub mod attractor;
pub mod cultivator;
pub mod dynamics;
pub mod sheet_music;
pub mod stamp;

pub use attractor::{
    GraspHoldAttractor, KinematicGraspAttractor, ScalarAttractor, StrangeAttractor,
};
pub use cultivator::{invert_grasp_hold, invert_kinematic_grasp, Cultivator, InvertResult};
pub use dynamics::{
    DiscreteDynamics, GlomalinExudationDynamics, GraspForceDynamics, HandTendonDynamics,
    MycelialLineDynamics, GRASP_CLAMP_N, GRASP_DT, GRASP_STATE_DIM, HAND_STATE_DIM, MYCELIAL_DT,
};
pub use sheet_music::{ActuationChannel, ActuationEvent, ChannelKind, SheetMusic};
pub use stamp::{
    forge_dark_window, invert_shear_catch, stamp_hand_frames, write_hand_soma, ShearCatchScenario,
    SHEAR_CATCH_HORIZON_S,
};
