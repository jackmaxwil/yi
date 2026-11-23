//! Things outside the app, each a feed on a worker thread: disk, git, a PTY, the browser,
//! logs, Docker, launchd, language and debug servers, machines. Only here is a process started.
#![forbid(unsafe_code)]
#![deny(clippy::string_slice)]

pub mod browser;
pub mod cdp;
pub mod dap;
pub mod docker;
pub mod fetch;
pub mod fs;
pub mod git;
pub mod launchd;
pub mod logs;
pub mod lsp;
pub mod machines;
pub mod pty;
pub mod registry;
pub mod server;
