//! Empty, loading, offline and error states of this pane: its words, as data.

use crate::copy::empty::said;
use yd_ui::states::status::Words;

/// What the pane says of its report pages when it has none to show.
pub const WORDS: Words = Words {
    loading: said("Opening the page", "", None),
    empty: said(
        "No page yet",
        "A page the agent writes in this thread shows here.",
        None,
    ),
    offline: said(
        "Chrome is not running",
        "The page shows once it is back.",
        None,
    ),
    error: said(
        "The page cannot show",
        "Check that Chrome is installed, then show the pane again.",
        None,
    ),
};
