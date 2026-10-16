//! Every integration test of the crate in one binary, so an edit below yi-cli links one test
//! binary here, not 12. Cargo.toml sets `autotests = false`, so a new file here runs only
//! once it has a line below.

#[path = "support/acp_schema.rs"]
mod acp_schema;
#[path = "support/config_option.rs"]
mod config_option;
#[path = "support/daemon.rs"]
mod daemon;
#[path = "support/prompt_slots.rs"]
mod prompt_slots_support;
#[path = "../../types/tests/support/repo.rs"]
mod repo;
#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
#[path = "support/session_files.rs"]
mod session_files;
#[path = "../../types/tests/support/usage_record.rs"]
mod usage_record;

mod acp_protocol;
mod cli_surfaces;
mod console_drive;
mod journeys;
mod mcp_cli;
mod mcp_oauth;
mod monkey;
mod plan_ops;
mod python_client;
mod rpc_protocol;
mod serve_daemon;
mod tui_drive;
