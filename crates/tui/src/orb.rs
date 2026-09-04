//! The kitty image the session owns, and the phase walk that feeds it. The engine is
//! [`yi_orb`]; this is the part that knows about [`crate::app::App`] and the terminal.

pub use yi_orb::kitty;

/// The loop owns one of these for the session — one kitty image, double
/// buffered across [`kitty::IMAGE_IDS`].
pub struct Tick {
    pub shown: bool,
    at: Option<(u16, u16)>,
    last: std::time::Instant,
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
            last: std::time::Instant::now() - std::time::Duration::from_secs(1),
            front: 0,
            ids,
        }
    }

    pub fn hide(&mut self, out: &mut impl std::io::Write) {
        if self.shown {
            let _ = kitty::delete_id(out, self.ids[self.front]);
        }
        self.shown = false;
        self.at = None;
    }
}

/// The phase walks toward its target every frame, so the dots travel between the `Yi` mark
/// and the orb; settled it needs no repaint. Frames transmit, place, then delete the front.
pub fn tick(app: &mut crate::app::App, out: &mut impl std::io::Write, state: &mut Tick) {
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
        Some((col, row)) if due || stale || !state.shown => {
            if due {
                app.logo_phase =
                    crate::logo::advance(app.logo_phase, app.logo_target, state.last.elapsed());
                state.last = std::time::Instant::now();
            }
            let clock = app.started_at.elapsed().as_secs_f64();
            if let Some(frame) = crate::logo::frame(app.logo_phase, clock, 64) {
                let rgba = kitty::paint_rgba(&frame, 64.0, crate::app::ORB_PX);
                let back = 1 - state.front;
                let _ = kitty::transmit(out, state.ids[back], &rgba, crate::app::ORB_PX);
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
