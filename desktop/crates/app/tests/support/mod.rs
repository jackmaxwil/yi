//! Test support shared by this crate's tests: scenes drawn in process, tree assertions, goldens.
#![allow(dead_code, reason = "each test binary uses a part")]

pub mod scene;
#[path = "../../../ui/tests/support/tree.rs"]
pub mod tree;
