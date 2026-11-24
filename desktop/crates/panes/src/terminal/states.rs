//! Empty, loading, offline and error states of this pane: its words, as data.

use crate::copy::empty::said;
use yd_ui::states::status::Words;

/// What the pane says of its shells when it has none to show.
pub const WORDS: Words = Words {
    loading: said("Starting the shell", "", None),
    empty: said("The shell has printed nothing yet", "", None),
    offline: said(
        "The shell has ended",
        "Open a new tab with + to start another.",
        None,
    ),
    error: said(
        "The shell could not start",
        "Open a new tab with + to try again.",
        None,
    ),
};
