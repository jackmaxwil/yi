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

/// Invariant: `--exact` matches the whole name path, which starts at the caller's file module.
fn exact_path(module_path: &str, name: &str) -> String {
    module_path.split_once("::").map_or_else(
        || name.to_owned(),
        |(_, module)| format!("{module}::{name}"),
    )
}
