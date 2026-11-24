//! Empty, loading, offline and error states of this pane: its words, as data.

use crate::copy::empty::said;
use yd_ui::states::status::Words;

/// What the pane says of its files when it has none to show.
pub const WORDS: Words = Words {
    loading: said("Opening the file", "", None),
    empty: said(
        "No file open",
        "Open one from Files, Changes or a chat step.",
        None,
    ),
    offline: said(
        "The file cannot be read now",
        "It shows once the folder is back.",
        None,
    ),
    error: said(
        "Could not open the file",
        "Check that the file is still there, then open it again.",
        None,
    ),
};
