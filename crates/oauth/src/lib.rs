//! Provider login (D191). An OAuth provider is described by a file the user writes
//! at `~/.yi/oauth/<provider>.json`; this crate reads it and ships no identity itself.

#![forbid(unsafe_code)]
#![deny(clippy::string_slice)]

pub mod cli;
pub mod flow;
pub mod loopback;
pub mod pkce;
pub mod registry;
pub mod store;
pub mod url;

/// Every public function returns this, so a caller matches instead of parsing
/// strings. `Message` carries the actionable texts (`token expired; run: yi login anthropic`).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Message(String),
}

impl From<String> for Error {
    fn from(text: String) -> Self {
        Self::Message(text)
    }
}

impl From<&str> for Error {
    fn from(text: &str) -> Self {
        Self::Message(text.to_owned())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
