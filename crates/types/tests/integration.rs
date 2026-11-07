//! Every integration test of the crate in one binary: yi-types is the root of the graph, so
//! each of fourteen binaries relinked on every edit to it. Cargo.toml sets `autotests = false`,
//! so a new file here runs only once it has a line below.
mod child_result;
mod config_migrate;
mod contract;
mod diff;
mod effort;
mod encoding;
mod graph;
mod image;
mod journal_chain;
mod json_salvage_prefix;
mod lease;
mod mail;
mod oauth_files;
mod plan_doc;
mod scratch_drop;
mod session_output;
mod settings;
mod stream_apply;
mod text_newtypes;
mod todo;
mod todo_record;
mod url;
mod wire_roundtrip;

fn workspace_root() -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let current = std::env::current_dir()?;
    if let Some(root) = current
        .ancestors()
        .find(|path| path.join("crates/types").is_dir())
    {
        return Ok(root.to_path_buf());
    }
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .find(|path| path.join("crates/types").is_dir())
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| "workspace root".into())
}

#[test]
fn the_suite_wrapper_can_observe_tmpdir() {
    let tmp = std::env::temp_dir();
    assert!(tmp.is_dir());
    println!("TEST_TMPDIR={}", tmp.display());
}
