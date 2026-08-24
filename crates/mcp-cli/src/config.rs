use std::path::Path;

use serde_json::Value;
use yi_types::mcp::McpServerSpec;

fn spec_from_entry(entry: &Value) -> Result<McpServerSpec, String> {
    if let Some(url) = entry.get("url").and_then(Value::as_str) {
        return Ok(McpServerSpec::Http {
            url: url.to_owned(),
        });
    }
    let Some(command) = entry.get("command").and_then(Value::as_str) else {
        return Err("config entry has neither \"command\" nor \"url\"".to_owned());
    };
    let args = entry
        .get("args")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let env = entry
        .get("env")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    Ok(McpServerSpec::Stdio {
        command: command.to_owned(),
        args,
        env,
    })
}

fn entries_of(config: &Value) -> Option<&serde_json::Map<String, Value>> {
    config
        .get("mcpServers")
        .or_else(|| config.get("servers"))
        .and_then(Value::as_object)
}

fn load_config(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    serde_json::from_str(&text).map_err(|error| format!("bad JSON in {}: {error}", path.display()))
}

fn from_file(path: &Path, entry: Option<&str>) -> Result<(String, McpServerSpec), String> {
    let config = load_config(path)?;
    let entries = entries_of(&config).ok_or_else(|| {
        format!(
            "{}: no \"mcpServers\" (or \"servers\") object",
            path.display()
        )
    })?;
    match entry {
        Some(name) => {
            let value = entries
                .get(name)
                .ok_or_else(|| format!("{}: no entry {name}", path.display()))?;
            Ok((name.to_owned(), spec_from_entry(value)?))
        }
        None => {
            let mut names: Vec<&String> = entries.keys().collect();
            names.sort();
            if let [only] = names.as_slice() {
                let value = entries
                    .get(only.as_str())
                    .ok_or_else(|| format!("{}: entry vanished", path.display()))?;
                return Ok(((*only).clone(), spec_from_entry(value)?));
            }
            Err(format!(
                "{}: multiple entries ({}); pick one with {}:<entry>",
                path.display(),
                names
                    .iter()
                    .map(|name| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                path.display()
            ))
        }
    }
}

/// Resolves `connect <server>` (design §5.2): a URL, a `<config-file>:<entry>`
/// reference, a config-file path, or a bare name looked up in `~/.yi/mcp.json`.
pub fn resolve_server(server: &str, home: &Path) -> Result<(String, McpServerSpec), String> {
    if server.contains("://") {
        return Ok((
            crate::sessions::default_session_name(server),
            McpServerSpec::Http {
                url: server.to_owned(),
            },
        ));
    }
    if let Some((path_part, entry)) = server.rsplit_once(':') {
        let path = Path::new(path_part);
        if path.is_file() {
            return from_file(path, Some(entry));
        }
    }
    if Path::new(server).is_file() {
        return from_file(Path::new(server), None);
    }
    let default_config = home.join(".yi").join("mcp.json");
    if default_config.is_file()
        && let Ok(found) = from_file(&default_config, Some(server))
    {
        return Ok(found);
    }
    for candidate in [".mcp.json", ".vscode/mcp.json", ".cursor/mcp.json"] {
        let path = Path::new(candidate);
        if path.is_file()
            && let Ok(found) = from_file(path, Some(server))
        {
            return Ok(found);
        }
    }
    Err(format!(
        "cannot resolve server {server}: not a URL, config path, or entry in ~/.yi/mcp.json / .mcp.json / .vscode/mcp.json / .cursor/mcp.json"
    ))
}
