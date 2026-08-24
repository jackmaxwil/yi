use std::io::Write;

use serde_json::json;

/// Append-only orphan pid journal (design K8): a record is written active on
/// spawn and inactive only on a confirmed kill — a wrong inactive write could
/// mask a reused pid. Env-gated; absent env means no journal.
pub fn record_orphan_process_state(pid: u32, active: bool, recorded_at: String) {
    let Some(path) = std::env::var_os("YI_ORPHAN_JOURNAL").filter(|value| !value.is_empty()) else {
        return;
    };
    let record = json!({
        "version": 1,
        "pid": pid,
        "ownerPid": std::process::id(),
        "active": active,
        "recordedAt": recorded_at,
    });
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    // Tracking must not make a successfully spawned kernel fail.
    let _ = writeln!(file, "{record}");
    let _ = file.sync_all();
}
