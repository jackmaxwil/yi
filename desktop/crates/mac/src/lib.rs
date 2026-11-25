#![deny(unsafe_code)]
#![deny(clippy::string_slice)]
#![deny(
    clippy::undocumented_unsafe_blocks,
    clippy::multiple_unsafe_ops_per_block
)]

#[cfg(target_os = "macos")]
#[allow(
    unsafe_code,
    reason = "AppKit status items: the one module that calls into Objective-C"
)]
mod status_item;
#[cfg(not(target_os = "macos"))]
#[path = "no_bar.rs"]
mod status_item;

pub use status_item::StatusItem;

/// Which row of a status menu was picked. The caller chooses the numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RowId(pub u32);

/// One row of a status menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// A greyed section title, such as "2 need you".
    Header(String),
    /// A row the user can pick.
    Item {
        id: RowId,
        title: String,
    },
    /// A row the user can pick, which Return also picks while the menu is open.
    Return {
        id: RowId,
        title: String,
    },
    Separator,
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StatusError {
    #[error("status items are made on the main thread")]
    NotMainThread,
    #[error("the system gave the status item no button")]
    NoButton,
    #[error("the status menu has no row {index}")]
    NoRow { index: usize },
    #[error("this system has no menu bar for status items")]
    NoMenuBar,
}
