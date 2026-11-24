//! Empty, loading, offline and error states of this pane: its words, as data.

use crate::copy::empty::said;
use yd_ui::states::status::Words;

/// What the pane says of its agents when it has none to show.
pub const WORDS: Words = Words {
    loading: said("Loading the agents", "", None),
    empty: said(
        "No agents in this thread",
        "Agents the thread starts show here.",
        None,
    ),
    offline: said(
        "The agents cannot be reached",
        "They show again once Yi is back.",
        None,
    ),
    error: said(
        "The agents could not load",
        "Open the thread again once Yi answers.",
        None,
    ),
};
