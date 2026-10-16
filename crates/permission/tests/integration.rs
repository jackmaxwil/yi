//! Every integration test of the crate in one binary, so an edit below yi-permission links one test
//! binary here, not 4. Cargo.toml sets `autotests = false`, so a new file here runs only
//! once it has a line below.

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;

mod decide;
mod open;
mod review;
mod safety;
