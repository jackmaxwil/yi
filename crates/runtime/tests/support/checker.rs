pub fn checker_manifest(
    command: &str,
    timeout_ms: u64,
    env: &[&str],
    reads_outside_snapshot: bool,
) -> serde_json::Value {
    serde_json::json!({
        "manifest": 1, "command": command, "cwd": "snapshot_root", "cwd_subdir": null,
        "protected": [], "timeout_ms": timeout_ms, "env": env,
        "reads_outside_snapshot": reads_outside_snapshot
    })
}

pub fn put_checker(
    artifacts: &yi_runtime::plan::artifact::Artifacts,
    value: &serde_json::Value,
    nonce: &str,
) -> Result<yi_types::plan::canonical::ArtifactRef, Box<dyn std::error::Error>> {
    Ok(artifacts.put(
        &serde_json::to_vec(value)?,
        "application/vnd.yi.checker-manifest+json",
        nonce,
    )?)
}
