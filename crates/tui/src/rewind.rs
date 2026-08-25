use std::io::Write;
use std::sync::Arc;

use ratatui::backend::Backend;
use ratatui::layout::Rect;
use yi_runtime::AgentSession;

use crate::app::App;
use crate::cell::Cell;

/// A rewind is only believable if the screen agrees with it: the entries that
/// were undone leave the transcript, and a rewound user message goes back into
/// the composer unsent (OMP `navigateTree` + `renderInitialMessages`).
pub fn process_pending_rewind<B: Backend + Write>(
    app: &mut App,
    terminal: &mut crate::terminal::Terminal<B>,
    session: &Arc<AgentSession>,
) {
    let Some(entry_id) = app.pending_rewind.take() else {
        return;
    };
    let rewound = match yi_runtime::rewind_to(session, &entry_id) {
        Ok(rewound) => rewound,
        Err(error) => {
            app.commit_cell(&Cell::Notice {
                text: format!("rewind failed: {error}"),
            });
            return;
        }
    };
    clear_screen(terminal);
    app.reset_transcript();
    crate::app::replay_session(app, session);
    if let Some(text) = rewound.unsent
        && app.composer.is_empty()
    {
        app.composer.set_text(&text);
    }
    app.scheduler.request();
}

/// The transcript above the viewport belongs to a branch that no longer
/// exists, and it lives in the emulator's scrollback where no repaint reaches
/// it — so the scrollback goes too, and the viewport re-anchors at the top.
fn clear_screen<B: Backend + Write>(terminal: &mut crate::terminal::Terminal<B>) {
    // The screen itself is cleared through the backend so the headless screen
    // clears too; only the scrollback erase (`ESC[3J`) has no backend call.
    let _ = Backend::clear(terminal.backend_mut());
    let _ = terminal.backend_mut().write_all(b"\x1b[3J");
    let _ = Backend::set_cursor_position(terminal.backend_mut(), ratatui::layout::Position::ORIGIN);
    let _ = std::io::Write::flush(terminal.backend_mut());
    let area = terminal.viewport_area();
    terminal.set_viewport_area(Rect {
        x: 0,
        y: 0,
        width: area.width,
        height: area.height,
    });
    terminal.invalidate_viewport();
}
