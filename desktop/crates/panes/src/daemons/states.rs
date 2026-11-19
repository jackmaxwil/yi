//! Empty, loading, offline and error states of this pane: its words, as data.

use crate::copy::empty::said;
use yd_ui::states::status::Words;

/// What the pane says of its services when it has none to show.
pub const WORDS: Words = Words {
    loading: said("Asking launchd", "", None),
    empty: said(
        "No services listed",
        "Add labels to daemons.labels in Settings to list more.",
        None,
    ),
    offline: said(
        "launchd is not answering",
        "The list fills once it answers.",
        None,
    ),
    error: said(
        "Could not list the services",
        "The list is read again while this pane shows.",
        None,
    ),
};
