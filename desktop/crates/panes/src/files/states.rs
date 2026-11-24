//! Empty, loading, offline and error states of this pane: its words, as data.

use crate::copy::empty::said;
use yd_ui::states::status::Words;

/// What the pane says of its files when it has none to show.
pub const WORDS: Words = Words {
    loading: said("Reading the working folder", "", None),
    empty: said(
        "No files read from the working folder yet",
        "Open a thread to read its folder.",
        None,
    ),
    offline: said(
        "The folder cannot be read now",
        "What was read last shows once it is back.",
        None,
    ),
    error: said(
        "Could not read the folder",
        "Check that the thread's folder is still there and can be read.",
        None,
    ),
};
