//! Empty, loading, offline and error states of this pane: its words, as data.

use crate::copy::empty::said;
use yd_ui::states::status::Words;

/// What the pane says of its containers when it has none to show.
pub const WORDS: Words = Words {
    loading: said("Asking Docker", "", None),
    empty: said("No containers", "Containers Docker runs show here.", None),
    offline: said(
        "Docker is not running",
        "Start Docker, then the list fills.",
        None,
    ),
    error: said(
        "Docker did not answer",
        "Check that Docker is running; the list fills when it answers.",
        None,
    ),
};
