//! Thought-orb engine: the agent's states as exact loops with morphs between them, the modes
//! verified dot for dot against pinned golden vectors, and the kitty painter.
#![forbid(unsafe_code)]
#![deny(clippy::string_slice)]

pub mod core;
pub mod kitty;
pub mod modes;
pub mod presets;
pub mod stage;
pub mod states;

pub use core::OrbFrame;
pub use presets::{Resolved, resolve};
pub use stage::{Orb, render};
pub use states::{OrbState, Point, pose};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Orbits,
    Globe,
    Rubik,
    Wave,
    Web,
    Braid,
    Ribbon,
    Ring,
    Morph,
}

/// Mode options keyed exactly as the TS engine keys them, so the baked
/// preset rows and the golden fixtures line up name-for-name.
pub struct Opts(pub BTreeMap<&'static str, f64>);

impl Opts {
    pub fn get(&self, key: &str, default: f64) -> f64 {
        self.0.get(key).copied().unwrap_or(default)
    }
}

pub fn frame(mode: Mode, size: f64, t: f64, opts: &Opts) -> OrbFrame {
    match mode {
        Mode::Orbits => modes::frame_orbits(size, t, opts),
        Mode::Globe => modes::frame_globe(size, t, opts),
        Mode::Rubik => modes::frame_rubik(size, t, opts),
        Mode::Wave => modes::frame_wave(size, t, opts),
        Mode::Web => modes::frame_web(size, t, opts),
        Mode::Braid => modes::frame_braid(size, t, opts),
        Mode::Ribbon | Mode::Ring => modes::frame_ribbon(size, t, opts),
        Mode::Morph => modes::frame_morph(size, t, opts),
    }
}
