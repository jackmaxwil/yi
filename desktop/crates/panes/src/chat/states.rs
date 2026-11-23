//! Empty, loading, offline and error states of this pane: its words, as data.

use crate::copy::empty::said;
use yd_ui::states::status::Words;

/// What the pane says of the conversation when it has none to show.
pub const WORDS: Words = Words {
    loading: said("Loading the thread", "", None),
    empty: said("No thread yet", "Start one with New chat.", None),
    offline: said(
        "The thread cannot be reached",
        "What was said shows again once Yi is back.",
        None,
    ),
    error: said(
        "The thread could not load",
        "Open the thread again from the sidebar.",
        None,
    ),
};
