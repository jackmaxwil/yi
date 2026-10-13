//! The kitty image the session owns. What it shows is [`yi_orb::Orb`]'s: the `Yi` mark at
//! rest and a looping pose per agent state; this is the part that knows the terminal.

use std::time::{Duration, Instant};

pub use yi_orb::{Orb, OrbState, kitty};

/// 60 fps: dots move half a pixel a frame; 120 would double pty bytes and CPU, unseen.
pub const FRAME: Duration = Duration::from_millis(16);

/// The loop owns one of these for the session — one kitty image, double
/// buffered across [`kitty::IMAGE_IDS`].
pub struct Tick {
    pub shown: bool,
    at: Option<(u16, u16)>,
    last: Instant,
    front: usize,
    ids: [u32; 2],
}

impl Default for Tick {
    fn default() -> Self {
        Self::with_ids(kitty::IMAGE_IDS)
    }
}

impl Tick {
    pub fn with_ids(ids: [u32; 2]) -> Self {
        Self {
            shown: false,
            at: None,
            last: Instant::now(),
            front: 0,
            ids,
        }
    }

    pub fn wake(&self) -> Duration {
        FRAME
            .saturating_sub(self.last.elapsed())
            .max(Duration::from_millis(1))
    }

    pub fn hide(&mut self, out: &mut impl std::io::Write) {
        if self.shown {
            let _ = kitty::delete_id(out, self.ids[self.front]);
        }
        self.shown = false;
        self.at = None;
    }
}

#[derive(Default)]
pub struct Images {
    pub orb: Tick,
    pub logos: crate::logos::Tick,
}

impl Images {
    pub fn tick(&mut self, app: &mut crate::app::App, out: &mut impl std::io::Write) {
        tick(app, out, &mut self.orb);
        crate::logos::tick(app, out, &mut self.logos);
    }
}

/// Paint a due frame on the app's clock; at rest, or only moved, re-place the image sent.
pub fn tick(app: &mut crate::app::App, out: &mut impl std::io::Write, state: &mut Tick) {
    if !app.kitty {
        return;
    }
    let want = app.orb_state();
    let resting = app.orb.at_rest(want);
    let due = !resting && app.clock.since(state.last) >= FRAME;
    let stale = app.take_orb_stale();
    match app.orb_placement {
        Some((col, row)) if due || stale || !state.shown => {
            let frame = app.orb.frame(app.clock.since(state.last), want);
            state.last = app.clock.now();
            let (width, height) = pixel_size();
            let rgba = kitty::paint_rgba(&frame, 64.0, width, height);
            let back = 1 - state.front;
            let _ = kitty::transmit(out, state.ids[back], &rgba, (width, height));
            let placed = kitty::place(
                out,
                state.ids[back],
                col,
                row,
                crate::app::ORB_COLS,
                crate::app::ORB_ROWS,
            );
            if placed.is_ok() {
                if state.shown {
                    let _ = kitty::delete_id(out, state.ids[state.front]);
                }
                state.front = back;
                state.shown = true;
                state.at = Some((col, row));
            }
        }
        Some((col, row)) if state.at != Some((col, row)) => {
            let placed = kitty::place(
                out,
                state.ids[state.front],
                col,
                row,
                crate::app::ORB_COLS,
                crate::app::ORB_ROWS,
            );
            if placed.is_ok() {
                state.at = Some((col, row));
            }
        }
        None if state.shown => state.hide(out),
        _ => {}
    }
    if resting {
        state.last = app.clock.now();
    }
}

/// Show the orb without animating it: transmit once when first shown, re-place when moved; the
/// focused pane keeps animating through [`tick`].
pub fn place(app: &mut crate::app::App, out: &mut impl std::io::Write, state: &mut Tick) {
    if !app.kitty {
        return;
    }
    let stale = app.take_orb_stale();
    let Some((col, row)) = app.orb_placement else {
        if state.shown {
            state.hide(out);
        }
        return;
    };
    if state.shown && !stale && state.at == Some((col, row)) {
        state.last = app.clock.now();
        return;
    }
    if !state.shown {
        let want = app.orb_state();
        let frame = app.orb.frame(app.clock.since(state.last), want);
        let (width, height) = pixel_size();
        let rgba = kitty::paint_rgba(&frame, 64.0, width, height);
        let _ = kitty::transmit(out, state.ids[state.front], &rgba, (width, height));
    }
    let placed = kitty::place(
        out,
        state.ids[state.front],
        col,
        row,
        crate::app::ORB_COLS,
        crate::app::ORB_ROWS,
    );
    if placed.is_ok() {
        state.shown = true;
        state.at = Some((col, row));
    }
    state.last = app.clock.now();
}

const MAX_ORB_PX: usize = 384;

/// The orb's rect in the terminal's own pixels (square fallback without one, 384 cap).
fn pixel_size() -> (usize, usize) {
    let fallback = (crate::app::ORB_PX, crate::app::ORB_PX);
    let Ok(size) = ratatui::crossterm::terminal::window_size() else {
        return fallback;
    };
    if size.columns == 0 || size.rows == 0 || size.width == 0 || size.height == 0 {
        return fallback;
    }
    let side = |pixels: u16, cells: u16, span: u16| {
        (usize::from(pixels) / usize::from(cells) * usize::from(span)).clamp(8, MAX_ORB_PX)
    };
    (
        side(size.width, size.columns, crate::app::ORB_COLS),
        side(size.height, size.rows, crate::app::ORB_ROWS),
    )
}
