//! Provider login (D172). An OAuth provider is described by a file the user writes
//! at `~/.yi/oauth/<provider>.json`; this crate reads it and ships no identity itself.

#![forbid(unsafe_code)]
#![deny(clippy::string_slice)]

pub mod cli;
pub mod flow;
pub mod loopback;
pub mod pkce;
pub mod registry;
pub mod store;
