use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use yi_tools::{keyless_command, run_captured_until};

const LIST_TIMEOUT: Duration = Duration::from_secs(20);
const PROBE_TIMEOUT: Duration = Duration::from_secs(90);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
const OUTPUT_CAP: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpRow {
    pub name: String,
    pub source: String,
    pub path: PathBuf,
    pub transport: String,
    pub target: String,
    pub enabled: bool,
    pub session: String,
    pub state: Option<String>,
    pub tools: Option<usize>,
}

impl McpRow {
    pub fn is_http(&self) -> bool {
        self.transport == "http"
    }

    pub fn auth(&self) -> &'static str {
        match (self.is_http(), self.state.as_deref()) {
            (false, _) => "none",
            (true, Some("unauthorized")) => "login needed",
            (true, Some("expired")) => "expired",
            (true, Some("live")) => "signed in",
            (true, _) => "unknown",
        }
    }
}

pub fn parse_rows(json: &str) -> Result<Vec<McpRow>, String> {
    let value: Value = serde_json::from_str(json).map_err(|error| error.to_string())?;
    let items = value.as_array().ok_or("expected a list of servers")?;
    let text = |item: &Value, key: &str| {
        item.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    Ok(items
        .iter()
        .filter(|item| item.get("error").is_none())
        .map(|item| McpRow {
            name: text(item, "name"),
            source: text(item, "source"),
            path: PathBuf::from(text(item, "path")),
            transport: text(item, "transport"),
            target: text(item, "target"),
            enabled: item.get("enabled").and_then(Value::as_bool) != Some(false),
            session: text(item, "session"),
            state: item.get("state").and_then(Value::as_str).map(str::to_owned),
            tools: item
                .get("tools")
                .and_then(Value::as_u64)
                .and_then(|count| usize::try_from(count).ok()),
        })
        .collect())
}

fn own_exe() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|error| error.to_string())
}

fn run(
    exe: &Path,
    home: Option<&Path>,
    args: &[&str],
    timeout: Duration,
) -> Result<String, String> {
    let mut command = keyless_command(exe);
    command.arg("mcp").args(args);
    if let Some(home) = home {
        command.env("HOME", home);
    }
    let capture = run_captured_until(command, None, None, timeout, OUTPUT_CAP)?;
    if capture.cancelled {
        return Err(format!("yi mcp {} timed out", args.join(" ")));
    }
    if capture.exit_code == Some(0) {
        return Ok(capture.stdout);
    }
    let why = capture.stderr.trim();
    Err(if why.is_empty() {
        format!("yi mcp {} failed", args.join(" "))
    } else {
        why.to_owned()
    })
}

pub fn servers() -> Result<Vec<McpRow>, String> {
    servers_via(&own_exe()?, None)
}

pub fn servers_via(exe: &Path, home: Option<&Path>) -> Result<Vec<McpRow>, String> {
    parse_rows(&run(exe, home, &["servers", "--json"], LIST_TIMEOUT)?)
}

pub fn probe(row: &McpRow) -> Result<Vec<McpRow>, String> {
    probe_via(&own_exe()?, None, row)
}

pub fn probe_via(exe: &Path, home: Option<&Path>, row: &McpRow) -> Result<Vec<McpRow>, String> {
    let server = format!("{}:{}", row.path.display(), row.name);
    let session = format!("@{}", row.session);
    run(
        exe,
        home,
        &["connect", &server, &session, "--json"],
        PROBE_TIMEOUT,
    )?;
    servers_via(exe, home)
}

pub fn login(row: &McpRow) -> Result<String, String> {
    if !row.is_http() {
        return Err(format!("{} is a stdio server and has no login", row.name));
    }
    run(&own_exe()?, None, &["login", &row.target], LOGIN_TIMEOUT).map(|out| out.trim().to_owned())
}

pub fn enabled(home: &Path) -> bool {
    crate::settings::current(home)
        .ok()
        .and_then(|rows| rows.into_iter().find(|row| row.id == "mcp.enabled"))
        .is_some_and(|row| row.value.as_bool().unwrap_or(false))
}

pub fn set_enabled(home: &Path, on: bool) -> Result<(), String> {
    crate::settings::save(home, "mcp.enabled", Value::Bool(on)).map(|_| ())
}

pub fn open_config(path: &Path) -> Result<(), String> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut command = keyless_command(opener);
    command.arg(path);
    let capture = run_captured_until(command, None, None, Duration::from_secs(10), OUTPUT_CAP)?;
    if capture.exit_code == Some(0) {
        Ok(())
    } else {
        Err(format!("could not open {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = r#"[
        {"name":"demo","source":"~/.yi/mcp.json","path":"/h/.yi/mcp.json","transport":"stdio","target":"echo","enabled":true,"session":"demo","state":"live","tools":3},
        {"name":"web","source":".mcp.json","path":"/w/.mcp.json","transport":"http","target":"http://x/mcp","enabled":false,"session":"web","state":"unauthorized","tools":null},
        {"source":".cursor/mcp.json","error":"unreadable config"}
    ]"#;

    #[test]
    fn mcp_rows_parse_with_state_and_skip_unreadable_files() -> Result<(), String> {
        let rows = parse_rows(LISTING)?;
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].tools, Some(3));
        assert_eq!(rows[0].auth(), "none");
        assert!(!rows[1].enabled);
        assert_eq!(rows[1].auth(), "login needed");
        assert_eq!(rows[1].tools, None);
        Ok(())
    }

    #[test]
    fn mcp_rows_refuse_a_non_list() {
        assert!(parse_rows("{}").is_err());
    }
}
