//! Empty, loading, offline and error states of this pane: its words, as data.

use crate::copy::empty::said;
use yd_ui::states::status::Words;

/// What the bell's panel says of its notifications when it has none to show.
pub const WORDS: Words = Words {
    loading: said("Loading notifications", "", None),
    empty: said(
        "Nothing needs you",
        "A thread that needs you or finishes shows up here.",
        None,
    ),
    offline: said(
        "Notifications are offline",
        "They come back once Yi is back.",
        None,
    ),
    error: said(
        "Notifications could not load",
        "They come back once Yi answers.",
        None,
    ),
};
