#![forbid(unsafe_code)]
#![deny(clippy::string_slice)]

pub mod acp;
pub mod advisor;
pub mod backoff;
pub mod channel;
pub mod checkpoint;
pub mod compaction;
pub mod config;
pub mod entry;
pub mod event;
pub mod fetch;
pub mod goal;
pub mod graph;
pub mod harness;
pub mod json_salvage;
pub mod kernel;
pub mod lane;
pub mod lease;
pub mod mail;
pub mod mcp;
pub mod message;
pub mod model;
pub mod node;
pub mod oauth;
pub mod permission;
pub mod plan;
pub mod record;
pub mod schedule;
pub mod subagent;
pub mod tape;
pub mod telemetry;
pub mod todo;
pub mod trace;
pub mod url;
pub mod wire;
