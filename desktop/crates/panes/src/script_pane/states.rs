//! Empty, loading, offline and error states of this pane: its words, as data.

use crate::copy::empty::said;
use yd_ui::states::status::Words;

/// What the pane says of the script when it has none to show.
pub const WORDS: Words = Words {
    loading: said("Loading the script", "", None),
    empty: said(
        "No plugin panes yet",
        "Put one in the app's panes folder, or ask Yi to build one.",
        None,
    ),
    offline: said(
        "The script's data is not there",
        "It draws again once its plugin has run.",
        None,
    ),
    error: said(
        "The script failed",
        "Fix the script; it runs again when its file is saved.",
        None,
    ),
};
