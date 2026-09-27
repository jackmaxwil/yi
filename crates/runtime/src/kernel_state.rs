use std::path::{Path, PathBuf};

pub(crate) fn state_dir(dir: &Path, per_session: bool, key: Option<&str>) -> Option<PathBuf> {
    if !per_session {
        return Some(dir.to_path_buf());
    }
    let id = key.filter(|id| yi_session::validate_session_id(id).is_ok())?;
    let state = dir.join("kernels").join(id);
    std::fs::create_dir_all(&state).ok()?;
    let (old, new) = (
        crate::kernel::snapshot_paths(dir, Some(id)),
        crate::kernel::snapshot_paths(&state, Some(id)),
    );
    for (from, to) in [(old.0, new.0), (old.1, new.1)] {
        if from.is_file() && !to.exists() {
            let _unmoved_stays_where_no_kernel_reads = std::fs::rename(&from, &to);
        }
    }
    Some(state)
}

pub(crate) fn register_harness_save(
    registry: &mut crate::kernel::HostRegistry,
    broker: Option<std::sync::Arc<crate::permission::PermissionBroker>>,
    home: &Path,
) {
    let file = home.join(".yi").join("harness").join("harness_state.json");
    registry.register("harness.save_global", move |payload| {
        let (broker, file) = (broker.clone(), file.clone());
        Box::pin(async move {
            let state = payload
                .get("state")
                .filter(|state| state.is_object())
                .ok_or_else(|| "harness.save_global requires a \"state\" object".to_owned())?;
            let text = serde_json::to_string_pretty(state).map_err(|error| error.to_string())?;
            let mut args = serde_json::Map::new();
            args.insert("path".to_owned(), file.display().to_string().into());
            args.insert("content".to_owned(), text.clone().into());
            let decided = tokio::task::spawn_blocking(move || {
                broker.map(|broker| {
                    let kind = yi_tools::ToolKind::Write;
                    broker.decide_call("write", kind, false, "harness.save_global", &args, None)
                })
            })
            .await
            .map_err(|error| error.to_string())?;
            if let Some(refused) = decided.filter(|outcome| !outcome.allowed) {
                return Err(format!("Permission denied: {}", refused.reason));
            }
            replace_store(&file, &text)?;
            Ok(serde_json::Map::new())
        })
    });
}

/// Incident: a pid-only temp name was shared by every kernel in the process, so two saves at
/// once wrote one temp file and the loser's rename failed.
fn replace_store(file: &Path, text: &str) -> Result<(), String> {
    let parent = file.parent().ok_or("the harness store has no directory")?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let fresh = file.with_extension(format!("tmp-{}", crate::plan::store::nonce()));
    std::fs::write(&fresh, text).map_err(|error| error.to_string())?;
    std::fs::rename(&fresh, file).map_err(|error| {
        let _staged_file_is_litter_only = std::fs::remove_file(&fresh);
        error.to_string()
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn concurrent_saves_leave_one_whole_store() -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join(format!("yi-harness-race-{}", std::process::id()));
        let file = dir.join("harness_state.json");
        let payloads: Vec<String> = (0..16)
            .map(|n| serde_json::json!({ "entries": "é".repeat(n * 4096) }).to_string())
            .collect();
        let saved: Vec<Result<(), String>> = std::thread::scope(|scope| {
            let saves: Vec<_> = payloads
                .iter()
                .map(|text| scope.spawn(|| super::replace_store(&file, text)))
                .collect();
            saves
                .into_iter()
                .map(|save| save.join().unwrap_or(Err("a save panicked".to_owned())))
                .collect()
        });
        let text = std::fs::read_to_string(&file)?;
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            saved.iter().all(Result::is_ok),
            "every save lands: {saved:?}"
        );
        serde_json::from_str::<serde_json::Value>(&text)?;
        assert!(payloads.contains(&text), "the store is one save, whole");
        Ok(())
    }
}
