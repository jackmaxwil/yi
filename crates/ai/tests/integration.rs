//! Every integration test of the crate in one binary, so an edit below yi-ai links one test
//! binary here, not thirteen. Cargo.toml sets `autotests = false`, so a new file here runs only
//! once it has a line below.
#[path = "../../types/tests/support/scratch.rs"]
mod scratch;

mod anthropic_mapper;
#[path = "../../types/tests/support/model.rs"]
mod common;
mod compat;
mod decide;
mod effort_catalog;
mod faux_events;
mod images;
mod leak;
mod model_headers;
mod oauth;
mod openai_mapper;
mod openai_responses;
mod openrouter;
mod proxy;
mod refresh;
mod resend;
mod strict_tools;
mod structured;
mod tool_choice;
