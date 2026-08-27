// thinking-orbs port (A.13, D41): geometry-exact engine — verified against
// the library's own golden vectors — rendered through the kitty graphics
// protocol (plain spinner everywhere else).

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

/// The nine reference states; TUI activity maps onto them in `app`.
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

/// U34 bookkeeping for the one kitty image: whether it is currently placed,
/// where, and when its last animation step ran. The loop owns one of these for
/// the session.
pub struct Tick {
    pub shown: bool,
    at: Option<(u16, u16)>,
    last: std::time::Instant,
}

impl Default for Tick {
    fn default() -> Self {
        Self {
            shown: false,
            at: None,
            last: std::time::Instant::now() - std::time::Duration::from_secs(1),
        }
    }
}

/// U34: one image. The phase walks toward its target every frame, so the dots
/// visibly travel between the `Yi` mark and the orb; a settled phase at rest
/// needs no repaint at all.
pub fn tick<B>(
    app: &mut crate::app::App,
    terminal: &mut crate::terminal::Terminal<B>,
    state: &mut Tick,
) where
    B: ratatui::backend::Backend + std::io::Write,
{
    if !app.kitty {
        return;
    }
    let animating = app.logo_phase != app.logo_target || app.logo_target > 0.0;
    let due = animating
        && state.last.elapsed() >= std::time::Duration::from_millis(crate::logo::FRAME_MS);
    if !animating {
        state.last = std::time::Instant::now();
    }
    let stale = app.take_orb_stale();
    match app.orb_placement {
        Some((col, row)) if due || stale || !state.shown || state.at != Some((col, row)) => {
            if due {
                app.logo_phase =
                    crate::logo::advance(app.logo_phase, app.logo_target, state.last.elapsed());
                state.last = std::time::Instant::now();
            }
            let clock = app.started_at.elapsed().as_secs_f64();
            if let Some(frame) = crate::logo::frame(app.logo_phase, clock, 64) {
                let rgba = kitty::paint_rgba(&frame, 64.0, crate::app::ORB_PX);
                let _ = kitty::emit(
                    terminal.backend_mut(),
                    &rgba,
                    crate::app::ORB_PX,
                    col,
                    row,
                    crate::app::ORB_COLS,
                    crate::app::ORB_ROWS,
                );
                state.shown = true;
                state.at = Some((col, row));
            }
        }
        None if state.shown => {
            state.shown = false;
            let _ = kitty::delete(terminal.backend_mut());
        }
        _ => {}
    }
}
