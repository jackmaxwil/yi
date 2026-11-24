//! Everything with no pixels: the model, its one `step`, feeds, the grid, addresses, commands,
//! tokens and settings. Tested without a window.
#![forbid(unsafe_code)]
#![deny(clippy::string_slice)]

pub mod address;
pub mod command;
pub mod feed;
pub mod findings;
pub mod grid;
pub mod ids;
pub mod inbox;
pub mod kind;
pub mod list;
pub mod model;
pub mod msg;
pub mod pane;
pub mod reduce;
pub mod row;
pub mod settings;
pub mod tokens;
pub mod trace;
pub mod wire;
