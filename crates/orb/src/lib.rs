//! Thought-orb engine: the agent's states as exact loops with morphs between them, and the
//! kitty painter.
#![forbid(unsafe_code)]
#![deny(clippy::string_slice)]

pub mod core;
pub mod kitty;
pub mod stage;
pub mod states;

pub use core::OrbFrame;
pub use stage::{Orb, mark_at, render};
pub use states::{MARK_STROKES, OrbState, Point, pose};
