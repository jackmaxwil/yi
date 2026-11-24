//! Empty, loading, offline and error states of this pane: its words, as data.

use crate::copy::empty::said;
use yd_ui::states::status::Words;

/// What the pane says of its changes when it has none to show.
pub const WORDS: Words = Words {
    loading: said("Reading the changes", "", None),
    empty: said(
        "Nothing uncommitted in the working folder",
        "Edits the agent makes show here.",
        None,
    ),
    offline: said(
        "Git is not answering",
        "The changes show once it is back.",
        None,
    ),
    error: said(
        "Could not read the changes",
        "Check that the thread's folder is a git repository.",
        None,
    ),
};
