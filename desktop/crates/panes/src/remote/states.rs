//! Empty, loading, offline and error states of this pane: its words, as data.

use crate::copy::empty::said;
use yd_ui::states::status::Words;

/// What the pane says while it has no screen to show.
pub const WORDS: Words = Words {
    loading: said("Opening the screen", "", None),
    empty: said(
        "No screen open",
        "Open one from VMs with Open screen.",
        None,
    ),
    offline: said(
        "The machine host is not answering",
        "The screen shows again once it is back.",
        None,
    ),
    error: said(
        "The screen cannot show",
        "Check the machine host in Settings.",
        None,
    ),
};
