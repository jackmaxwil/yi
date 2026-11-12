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

/// One critical `cmd` item over a checker written into the plan's artifacts.
pub fn cmd_contract(
    store: &yi_runtime::plan::store::PlanStore,
    plan: &yi_types::plan::doc::PlanId,
    command: &str,
    class: &str,
) -> Result<yi_types::plan::contract::Contract, Box<dyn std::error::Error>> {
    let manifest = checker_manifest(command, 10_000, &[], false);
    let put = put_checker(&store.artifacts(plan), &manifest, &store.nonce())?;
    Ok(serde_json::from_value(serde_json::json!({
        "class": class,
        "items": [{"id": "check", "critical": true, "weight": 100,
                   "decider": {"cmd": {"checker": put, "timeout_ms": 10_000}}}],
        "threshold": 1000, "min_coverage": 1000
    }))?)
}
