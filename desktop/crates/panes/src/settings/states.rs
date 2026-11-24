//! Empty, loading, offline and error states of this pane: its words, as data.

use crate::copy::empty::said;
use yd_ui::states::status::Words;

/// What the pane says of Yi's settings when it has none to show.
pub const WORDS: Words = Words {
    loading: said("Reading Yi's settings", "", None),
    empty: said("Yi sent no settings", "", None),
    offline: said(
        "Yi's settings cannot be reached",
        "They show again once Yi is back.",
        None,
    ),
    error: said(
        "Yi's settings could not load",
        "Reconnect, then they are read again.",
        None,
    ),
};
