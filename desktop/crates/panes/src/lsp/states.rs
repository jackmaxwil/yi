//! Empty, loading, offline and error states of this pane: its words, as data.

use crate::copy::empty::said;
use yd_ui::states::status::Words;

/// What the pane says of its language servers when it has none to show.
pub const WORDS: Words = Words {
    loading: said("Starting the language servers", "", None),
    empty: said(
        "No language servers",
        "A server starts when a file of its language opens.",
        None,
    ),
    offline: said(
        "The language servers have stopped",
        "Restart all starts them again.",
        None,
    ),
    error: said(
        "A language server failed",
        "Restart all starts it again; its log says why.",
        None,
    ),
};

/// The problems list: the servers' own state is said above it, so it says only what it lacks.
pub const NO_PROBLEMS: Words = Words {
    loading: said("Waiting for the servers' problems", "", None),
    empty: said("No problems", "", None),
    offline: said("No problems while the servers are stopped", "", None),
    error: said("No problems could be read", "", None),
};
