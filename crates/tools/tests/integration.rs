//! Every integration test of the crate in one binary, so an edit below yi-tools links one test
//! binary here, not 11. Cargo.toml sets `autotests = false`, so a new file here runs only
//! once it has a line below.

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;

mod documents;
mod endings;
mod hashline_parser;
mod hashline_patcher;
mod kernel_cell;
mod orient;
mod paths;
mod program_values;
mod sandbox;
mod tools;
