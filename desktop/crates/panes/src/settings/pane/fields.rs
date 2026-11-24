//! The text fields of the pane, drawn from the window's own while it has the keyboard.

use rune::TextEdit;
use yd_ui::pane::PaneCx;

use yd_ui::win::field;

use super::state::Part;

/// The field to draw: the window's own while it holds the keyboard, else one holding `kept`.
/// Invariant: Rune keeps focus and caret in the edit the view draws.
pub fn drawn(pane: &PaneCx<'_>, part: Part, kept: &str) -> TextEdit {
    let own = pane
        .edits
        .and_then(|edits| edits.get(&field(pane.pane, part.name())));
    match own {
        Some(own) if own.focused => own.clone(),
        _ => TextEdit::new(kept),
    }
}
