//! thinking-orbs port (A.13, D41): geometry-exact engine — verified against
//! the library's own golden vectors — and the kitty painter that puts it on
//! screen. No terminal framework and no Yi crate: the host owns placement.
#![forbid(unsafe_code)]

pub mod core;
pub mod kitty;
pub mod modes;
pub mod presets;

use std::collections::BTreeMap;

pub use core::OrbFrame;
pub use presets::{Resolved, resolve};

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

/// The nine reference states; the host maps its own activity onto them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrbState {
    Working,
    Searching,
    Solving,
    Listening,
    Connecting,
    Weaving,
    Composing,
    Breathing,
    Shaping,
}

impl OrbState {
    pub fn key(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Searching => "searching",
            Self::Solving => "solving",
            Self::Listening => "listening",
            Self::Connecting => "connecting",
            Self::Weaving => "weaving",
            Self::Composing => "composing",
            Self::Breathing => "breathing",
            Self::Shaping => "shaping",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Working => "Working…",
            Self::Searching => "Searching…",
            Self::Solving => "Solving…",
            Self::Listening => "Listening…",
            Self::Connecting => "Connecting…",
            Self::Weaving => "Weaving…",
            Self::Composing => "Composing…",
            Self::Breathing => "Breathing…",
            Self::Shaping => "Shaping…",
        }
    }
}

/// Evaluate a (state, size) preset at clock time `clock` (seconds): the
/// preset speed multiplies the shared clock, exactly as `ThinkingOrb` does.
pub fn evaluate(state: OrbState, size: u32, clock: f64) -> Option<OrbFrame> {
    // orbits-64's sparse particles read as noise in an ~80px cell rect;
    // ribbon's dense band survives the downscale, so the working/default
    // state borrows the composing geometry (user-directed, D41).
    let key = match state {
        OrbState::Working => "composing",
        other => other.key(),
    };
    let resolved = resolve(key, size)?;
    let opts = resolved.options();
    Some(frame(
        resolved.mode,
        f64::from(size),
        clock * resolved.speed,
        &opts,
    ))
}
