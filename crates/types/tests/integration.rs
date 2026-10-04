//! Every integration test of the crate in one binary: yi-types is the root of the graph, so
//! each of fourteen binaries relinked on every edit to it. Cargo.toml sets `autotests = false`,
//! so a new file here runs only once it has a line below.
mod child_result;
mod config_migrate;
mod contract;
mod effort;
mod graph;
mod image;
mod journal_chain;
mod lease;
mod mail;
mod oauth_files;
mod plan_doc;
mod scratch_drop;
mod session_output;
mod stream_apply;
mod text_newtypes;
mod todo_record;
mod wire_roundtrip;
