//! The kitty image the session owns, and the phase walk that feeds it.
//! The engine itself is [`yi_orb`]; this is the part that knows about
//! [`crate::app::App`] and the terminal.

use yi_orb::kitty;

/// The loop owns one of these for the session — one kitty image, double
/// buffered across [`kitty::IMAGE_IDS`].
pub struct Tick {
    pub shown: bool,
    at: Option<(u16, u16)>,
    last: std::time::Instant,
    front: usize,
}

impl Default for Tick {
    fn default() -> Self {
        Self {
            shown: false,
            at: None,
            last: std::time::Instant::now() - std::time::Duration::from_secs(1),
            front: 0,
        }
    }
}

/// The phase walks toward its target every frame, so the dots visibly travel
/// between the `Yi` mark and the orb; settled at rest, it needs no repaint.
/// Frames land transmit-back/place-back/delete-front; a bare move re-places.
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
                let out = terminal.backend_mut();
                let _ = kitty::transmit(out, kitty::IMAGE_IDS[back], &rgba, crate::app::ORB_PX);
                let placed = kitty::place(
                    out,
                    kitty::IMAGE_IDS[back],
                    col,
                    row,
                    crate::app::ORB_COLS,
                    crate::app::ORB_ROWS,
                );
                if placed.is_ok() {
                    if state.shown {
                        let _ = kitty::delete_id(out, kitty::IMAGE_IDS[state.front]);
                    }
                    state.front = back;
                    state.shown = true;
                    state.at = Some((col, row));
                }
            }
        }
        Some((col, row)) if state.at != Some((col, row)) => {
            let placed = kitty::place(
                terminal.backend_mut(),
                kitty::IMAGE_IDS[state.front],
                col,
                row,
                crate::app::ORB_COLS,
                crate::app::ORB_ROWS,
            );
            if placed.is_ok() {
                state.at = Some((col, row));
            }
        }
        None if state.shown => {
            state.shown = false;
            state.at = None;
            let _ = kitty::delete(terminal.backend_mut());
        }
        _ => {}
    }
}
