//! Empty, loading, offline and error states of this pane: its words, as data.

use crate::copy::empty::said;
use yd_ui::states::status::Words;

/// What the pane says of its plugins when it has none to show.
pub const WORDS: Words = Words {
    loading: said("Looking for plugins", "", None),
    empty: said(
        "No plugins installed",
        "Add builds one from a sentence.",
        None,
    ),
    offline: said(
        "The plugins folder cannot be read",
        "Check the app's panes folder; it is read again once a second.",
        None,
    ),
    error: said(
        "Could not load the plugins",
        "Fix the script; it loads again when its file is saved.",
        None,
    ),
};
