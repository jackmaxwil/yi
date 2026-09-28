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

fn register_harness_save(
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

/// The host requests for the two stores a kernel may not write itself: the global harness
/// state and the MCP session store.
pub(crate) fn register_host_stores(
    registry: &mut crate::kernel::HostRegistry,
    wiring: &crate::wiring::RuntimeWiring,
) {
    register_harness_save(registry, wiring.broker.clone(), &wiring.home);
    register_mcp_connect(registry, wiring.mcp_read.clone(), &wiring.home);
}

/// The kernel's `connect` runs on the host, from `~/.yi/mcp.json` alone: a cell names an entry
/// there, never a path, a URL or a workspace file it could have written itself.
fn register_mcp_connect(
    registry: &mut crate::kernel::HostRegistry,
    mcp: Option<std::sync::Arc<dyn crate::fetch::McpResourceRead>>,
    home: &Path,
) {
    let Some(mcp) = mcp else { return };
    let config = home.join(".yi").join("mcp.json");
    registry.register("mcp.connect", move |payload| {
        let (mcp, config) = (std::sync::Arc::clone(&mcp), config.clone());
        Box::pin(async move {
            let entry = plain_name(&payload, "server")?;
            let session = plain_name(&payload, "session")?;
            if !config.is_file() {
                return Err(format!(
                    "mcp.connect: no {}; add an entry {entry} there on the host",
                    config.display()
                ));
            }
            let reply =
                tokio::task::spawn_blocking(move || mcp.connect_server(&config, &entry, &session))
                    .await
                    .map_err(|error| error.to_string())??;
            serde_json::from_str(&reply).map_err(|error| format!("mcp.connect: {error}"))
        })
    });
}

/// One name with no separator, so `<config>:<entry>` and `@<session>` read as written.
fn plain_name(
    payload: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<String, String> {
    let value = payload
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let plain = !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'));
    if plain {
        Ok(value.to_owned())
    } else {
        Err(format!(
            "mcp.connect: {key} must be a name of letters, digits, `-`, `_` and `.`, not {value:?}"
        ))
    }
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
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use serde_json::{Map, Value};
    use yi_kernel::client::HostHandlers;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    struct Recording(Mutex<Vec<(PathBuf, String, String)>>);

    impl crate::fetch::McpResourceRead for Recording {
        fn read(&self, _server: &str, _resource: &str) -> Result<String, String> {
            Err("no read in this test".to_owned())
        }

        fn connect_server(
            &self,
            config: &Path,
            entry: &str,
            session: &str,
        ) -> Result<String, String> {
            self.0.lock().map_err(|error| error.to_string())?.push((
                config.to_path_buf(),
                entry.to_owned(),
                session.to_owned(),
            ));
            Ok(r#"{"session":"@krnl-x","state":"live"}"#.to_owned())
        }
    }

    async fn ask_connect(
        home: &Path,
        host: &Arc<Recording>,
        server: &str,
    ) -> Result<Map<String, Value>, String> {
        let mut registry = crate::kernel::HostRegistry::default();
        let mcp: Arc<dyn crate::fetch::McpResourceRead> = host.clone();
        super::register_mcp_connect(&mut registry, Some(mcp), home);
        let payload = serde_json::json!({"server": server, "session": "krnl-x"});
        let payload = payload.as_object().cloned().unwrap_or_default();
        registry
            .dispatch("mcp.connect", payload)
            .ok_or("mcp.connect is not registered")?
            .await
    }

    #[tokio::test]
    async fn a_kernel_connect_names_an_entry_of_the_home_config_or_is_refused() -> TestResult {
        let home = std::env::temp_dir().join(format!("yi-mcp-connect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".yi"))?;
        let host = Arc::new(Recording(Mutex::new(Vec::new())));
        let absent = ask_connect(&home, &host, "fixture").await;
        assert!(
            absent
                .as_ref()
                .is_err_and(|error| error.contains("mcp.json")),
            "no config, no spawn: {absent:?}"
        );
        std::fs::write(home.join(".yi/mcp.json"), "{}")?;
        for bad in [
            "../evil.json",
            "a:b",
            "/abs/mcp.json",
            "",
            "https://x",
            "a b",
        ] {
            let refused = ask_connect(&home, &host, bad).await;
            assert!(
                refused
                    .as_ref()
                    .is_err_and(|error| error.contains("must be a name")),
                "{bad:?}: {refused:?}"
            );
        }
        let live = ask_connect(&home, &host, "fixture").await?;
        assert_eq!(live.get("state").and_then(Value::as_str), Some("live"));
        let calls = host.0.lock().map(|calls| calls.clone()).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&home);
        assert_eq!(
            calls,
            vec![(
                home.join(".yi/mcp.json"),
                "fixture".to_owned(),
                "krnl-x".to_owned()
            )],
            "only the plain name reaches the host, through the home config"
        );
        Ok(())
    }

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
