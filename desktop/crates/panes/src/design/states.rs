//! Empty, loading, offline and error states of this pane: its words, as data.

use crate::copy::empty::said;
use yd_ui::states::status::Words;

/// What the pane says before an element is picked, by what the page's source says.
pub const WORDS: Words = Words {
    loading: said("Opening the page", "", None),
    empty: said(
        "Nothing picked yet",
        "Choose Design in the web app's bar, then press an element of the page.",
        None,
    ),
    offline: said(
        "Chrome is not running",
        "Design works on a page the Web app pane shows.",
        None,
    ),
    error: said(
        "The page cannot be designed",
        "Open the page again in the Web app pane.",
        None,
    ),
};

/// Nothing is picked and the page's source said why: its note is the error's words.
pub const REFUSED: Words = Words {
    error: said(
        "Nothing was picked",
        "Attach the whole page instead: right-click the Web app pane, then Attach to Chat.",
        None,
    ),
    ..WORDS
};

/// Design mode is on and waits for its element.
pub const PICKING: Words = Words {
    empty: said(
        "Nothing picked yet",
        "Press an element of the web page to design it.",
        None,
    ),
    ..WORDS
};
