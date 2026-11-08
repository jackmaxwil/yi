use std::path::{Path, PathBuf};

pub(crate) fn state_dir(dir: &Path, per_session: bool, key: Option<&str>) -> Option<PathBuf> {
    if !per_session {
        return Some(dir.to_path_buf());
    }
    let id = key.filter(|id| yi_session::validate_session_id(id).is_ok())?;
    let state = dir.join("kernels").join(id);
    std::fs::create_dir_all(&state).ok()?;
    let from = crate::kernel::snapshot_path(dir, Some(id));
    let to = crate::kernel::snapshot_path(&state, Some(id));
    if from.is_file() && !to.exists() {
        let _unmoved_stays_where_no_kernel_reads = std::fs::rename(&from, &to);
    }
    Some(state)
}

/// The host request for the store a kernel may not write itself: the MCP session store.
pub(crate) fn register_host_stores(
    registry: &mut crate::kernel::HostRegistry,
    wiring: &crate::wiring::RuntimeWiring,
) {
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
                    "mcp.connect: no {}; ask the user to add an entry {entry} there",
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
}
