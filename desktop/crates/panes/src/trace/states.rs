//! Empty, loading, offline and error states of this pane: its words, as data.

use crate::copy::empty::said;
use yd_ui::states::status::Words;

/// What the pane says of its turns when it has none to show.
pub const WORDS: Words = Words {
    loading: said("Loading the trace", "", None),
    empty: said(
        "No turn to trace yet",
        "Send a prompt and its steps show here.",
        None,
    ),
    offline: said(
        "The trace cannot be reached",
        "It shows again once Yi is back.",
        None,
    ),
    error: said(
        "The trace could not load",
        "Open the thread again once Yi answers.",
        None,
    ),
};
