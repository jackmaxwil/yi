//! Empty, loading, offline and error states of this pane: its words, as data.

use crate::copy::empty::said;
use yd_ui::states::status::Words;

/// What the pane says of its VMs when it has none to show.
pub const WORDS: Words = Words {
    loading: said("Asking the VM host", "", None),
    empty: said("No VMs", "VMs on the host show here.", None),
    offline: said(
        "The VM host is not set up",
        "Name the host and how to reach it.",
        None,
    ),
    error: said(
        "The VM host did not answer",
        "Check the machine host in Settings; the list fills when it answers.",
        None,
    ),
};
