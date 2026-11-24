//! Empty, loading, offline and error states of this pane: its words, as data.

use crate::copy::empty::said;
use yd_ui::states::status::Words;

/// What the pane says of its findings when it has none to show.
pub const WORDS: Words = Words {
    loading: said("Loading the findings", "", None),
    empty: said(
        "No findings yet",
        "What the agent checks and finds shows here.",
        None,
    ),
    offline: said(
        "The findings cannot be reached",
        "They show again once Yi is back.",
        None,
    ),
    error: said(
        "The findings could not load",
        "Open the thread again once Yi answers.",
        None,
    ),
};
