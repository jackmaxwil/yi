//! What a pane's own window draws: the grid of its one pane, whose bar is the title bar.

use rune::{Cx, El, col};
use yd_ui::grid_view::view::grid;
use yd_ui::win::Win;

use crate::root;
use crate::shell::view::backdrop;

/// The window's root, and what it draws under the pane.
pub const KEY: &str = "window";
const BACKDROP: &str = "window.backdrop";

/// The root of a window that is not the main one: no shell, the grid as it draws a lone
/// pane, and what every window's root carries.
pub fn view(win: &Win, cx: &mut Cx<Win>) -> El {
    let Ok(hub) = win.hub.try_borrow() else {
        return col();
    };
    root::zoomed(cx, &hub, yd_ui::chrome::tabs::TITLED);
    // Incident: with glass alone the zoomed window was clear past 1/1.25 of its size, and a
    // click there went to the window behind; the main window's backdrop leaves no gap.
    let pane = grid(win, &hub, cx);
    let body = backdrop(&hub, cx, BACKDROP, [pane]);
    root::dressed(win, &hub, cx, KEY, body)
}
