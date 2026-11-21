//! One module per pane. Most are descriptions over an engine in `yd-ui`.
#![forbid(unsafe_code)]
#![deny(clippy::string_slice)]

pub mod agents;
pub mod assets;
pub mod changes;
pub mod chat;
pub mod chat_doc;
pub mod code_view;
pub mod containers;
pub mod copy;
pub mod daemons;
pub mod debugger;
pub mod design;
pub mod files;
pub mod findings;
pub mod has;
pub mod ipython;
pub mod logs;
pub mod lsp;
pub mod new_chat;
pub mod notifications;
pub mod plugin_kit;
pub mod plugins;
pub mod registry;
pub mod remote;
pub mod report;
pub mod script_pane;
pub mod settings;
pub mod terminal;
pub mod themes;
pub mod trace;
pub mod vms;
pub mod web;
