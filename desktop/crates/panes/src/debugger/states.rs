//! Empty, loading, offline and error states of this pane: its words, as data.

use crate::copy::empty::said;
use yd_ui::states::status::Words;

/// What the pane says of its debug runs when it has none to show.
pub const WORDS: Words = Words {
    loading: said("Starting the debugger", "", None),
    empty: said(
        "Nothing is being debugged",
        "Stop Here in the Debugger, on a line, starts a run.",
        None,
    ),
    offline: said(
        "The debug adapter has ended",
        "Stop Here on a line starts a new run.",
        None,
    ),
    error: said(
        "The debugger could not start",
        "Check that the debug adapter is installed, then start again.",
        None,
    ),
};
