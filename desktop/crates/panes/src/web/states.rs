//! Empty, loading, offline and error states of this pane: its words, as data.

use crate::copy::empty::said;
use yd_ui::states::status::Words;

/// What the pane says of its pages when it has none to show.
pub const WORDS: Words = Words {
    loading: said("Opening the page", "", None),
    empty: said("No page open", "Type an address to open a page.", None),
    offline: said(
        "Chrome is not running",
        "The page shows once it is back.",
        None,
    ),
    error: said(
        "The page cannot show",
        "Check the address and that Chrome is installed, then open it again.",
        None,
    ),
};
