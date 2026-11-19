//! Empty, loading, offline and error states of this pane: its words, as data.

use crate::copy::empty::said;
use yd_ui::states::status::Words;

/// What the pane says of its log lines when it has none to show.
pub const WORDS: Words = Words {
    loading: said("Waiting for log lines", "", None),
    empty: said(
        "No log lines yet",
        "Lines from the app's sources show here.",
        None,
    ),
    offline: said(
        "The log sources are not running",
        "Lines show again once they are back.",
        None,
    ),
    error: said(
        "Could not read the logs",
        "Lines show again once their source can be read.",
        None,
    ),
};
