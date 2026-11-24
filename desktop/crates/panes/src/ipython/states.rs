//! Empty, loading, offline and error states of this pane: its words, as data.

use crate::copy::empty::said;
use yd_ui::states::status::Words;

/// What the pane says of the thread's kernel when it has no cell to show.
pub const WORDS: Words = Words {
    loading: said("Loading the kernel's cells", "", None),
    empty: said(
        "No cells in this thread yet",
        "Its kernel is not booted: Yi starts it on the agent's first ipython call.",
        None,
    ),
    offline: said(
        "The kernel's cells cannot be reached",
        "They show again once Yi is back.",
        None,
    ),
    error: said(
        "The kernel's cells could not load",
        "Open the thread again once Yi answers.",
        None,
    ),
};
