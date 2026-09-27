//! Every integration test of the crate in one binary, so an edit below yi-tui links one test
//! binary here, not sixteen. Cargo.toml sets `autotests = false`, so a new file here runs only
//! once it has a line below.
mod common;
#[path = "../../types/tests/support/scratch.rs"]
mod scratch;

mod agents;
mod asks;
mod cards;
mod diffview;
mod highlight;
mod hud_todos;
mod logos;
mod markdown_render;
mod model_picker;
mod motion;
mod pane_scroll;
mod plantree;
mod pycell;
mod resume;
mod reveal;
mod settled;
mod streaming;
mod toolcells;
mod tui_e2e;
mod tui_unit;
mod waits;
