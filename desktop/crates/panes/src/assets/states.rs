//! Empty, loading, offline and error states of this pane: its words, as data.

use crate::copy::empty::said;
use yd_ui::states::status::Words;

/// What the pane says of its assets when it has none to show.
pub const WORDS: Words = Words {
    loading: said("Scanning the assets", "", None),
    empty: said("No assets known", "Assets show once a scan has run.", None),
    offline: said(
        "No machines adapter is set up",
        "Name the host and how to reach it.",
        None,
    ),
    error: said(
        "The scan failed",
        "Check the machine host in Settings; the list fills when it answers.",
        None,
    ),
};
