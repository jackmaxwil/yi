//! Every integration test of the crate in one binary, so an edit below yi-console links one test
//! binary here, not 7. Cargo.toml sets `autotests = false`, so a new file here runs only
//! once it has a line below.

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;

mod copy;
mod drive;
mod editor_view;
mod keys;
mod redraw;
mod review;
mod sessions;
