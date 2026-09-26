//! The kitty image the session owns. What it shows is [`yi_orb::Orb`]'s: the `Yi` mark at
//! rest and a looping pose per agent state; this is the part that knows the terminal.

use std::time::{Duration, Instant};

pub use yi_orb::kitty;

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

/// Paint a due frame; at rest, or only moved, re-place the image already sent.
pub fn tick(app: &mut crate::app::App, out: &mut impl std::io::Write, state: &mut Tick) {
    if !app.kitty {
        return;
    }
    let want = app.orb_state();
    let due = !app.orb.at_rest(want) && state.last.elapsed() >= FRAME;
    let stale = app.take_orb_stale();
    match app.orb_placement {
        Some((col, row)) if due || stale || !state.shown => {
            let frame = app.orb.frame(state.last.elapsed().as_secs_f64(), want);
            state.last = Instant::now();
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
}

const MAX_ORB_PX: usize = 384;

/// The orb's cell rect in the terminal's pixels, so the image lands unresampled; no pixel
/// size reported (tmux, some ssh hops) gets the square fallback, a huge font the 384 cap.
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
