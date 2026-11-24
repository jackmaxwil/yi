//! What each pane says when it has nothing to show: the words more than one pane uses.

use yd_core::ids::CommandId;
use yd_core::reduce::step::commands::RECONNECT;
use yd_ui::pane::PaneCx;
use yd_ui::states::status::{Said, Status, Words};

/// One state's words: what is so, what to do, and the button that does it.
pub const fn said(what: &'static str, next: &'static str, button: Option<CommandId>) -> Said {
    Said { what, next, button }
}

/// The one state every pane that needs Yi's server shows while the link is not up.
pub const SERVER: Words = Words {
    loading: said("Connecting to Yi", "", None),
    empty: said("Connected to Yi", "", None),
    offline: said(
        "Yi is not running",
        "Start it with yi serve, or reconnect.",
        Some(RECONNECT),
    ),
    error: said(
        "Yi could not be reached",
        "Reconnect once the cause is fixed.",
        Some(RECONNECT),
    ),
};

/// A pane that reads the thread its window shows, while the window shows none.
pub const NO_THREAD: &str = "No thread yet";
/// What to do about [`NO_THREAD`].
pub const START_ONE: &str = "Start one with New chat.";

/// A pane that reads the shown thread, while the window shows none and Yi is there.
pub const THREADLESS: Words = Words {
    empty: said(NO_THREAD, START_ONE, None),
    ..SERVER
};

/// What a pane that reads the shown thread draws while there is none: Yi's link while it is
/// not up, else that there is no thread. Nothing once a thread shows.
pub fn threadless(pane: &PaneCx<'_>) -> Option<(Status, &'static Words)> {
    if pane.thread().is_some() {
        return None;
    }
    Some(match Status::of_link(&pane.model().wire.link) {
        Some(link) => (link, &SERVER),
        None => (Status::Empty, &THREADLESS),
    })
}

/// The link to Yi when it is down: what a pane still showing a thread says in one line.
pub fn down(pane: &PaneCx<'_>) -> Option<Status> {
    let link = &pane.model().wire.link;
    Status::of_link(link).filter(|status| *status != Status::Loading)
}
