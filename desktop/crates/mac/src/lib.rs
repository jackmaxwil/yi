#![deny(unsafe_code)]
#![deny(clippy::string_slice)]
#![deny(
    clippy::undocumented_unsafe_blocks,
    clippy::multiple_unsafe_ops_per_block
)]

#[allow(
    unsafe_code,
    reason = "AppKit status items: the one module that calls into Objective-C"
)]
mod status_item;

pub use status_item::{Row, RowId, StatusError, StatusItem};
