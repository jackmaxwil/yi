use std::error::Error;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

type TestResult = Result<(), Box<dyn Error>>;

struct McpHome {
    home: PathBuf,
}

fn fixture_server_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mcp_fixture_server.py")
}

/// Isolated HOME with mcp enabled and a fixture stdio server entry.
fn mcp_home() -> Result<McpHome, Box<dyn Error>> {
    static DIR_ID: AtomicU64 = AtomicU64::new(0);
    let unique = DIR_ID.fetch_add(1, Ordering::Relaxed);
    let home = std::env::temp_dir().join(format!("yi-mcp-e2e-{}-{unique}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join(".yi"))?;
    std::fs::write(
        home.join(".yi/config.json"),
        r#"{"mcp": {"enabled": true}}"#,
    )?;
    let server = fixture_server_path();
    std::fs::write(
        home.join(".yi/mcp.json"),
        serde_json::to_string(&serde_json::json!({
            "mcpServers": {
                "fixture": {"command": "python3", "args": [server.display().to_string()]}
            }
        }))?,
    )?;
    Ok(McpHome { home })
}

struct Output {
    code: i32,
    stdout: String,
    stderr: String,
}

fn yi(home: &McpHome, args: &[&str]) -> Result<Output, Box<dyn Error>> {
    #[expect(
        clippy::disallowed_methods,
        reason = "the contract under test is the spawned binary's stdio"
    )]
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_yi"))
        .args(args)
        .env("HOME", &home.home)
        .output()?;
    Ok(Output {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn yi_mcp(home: &McpHome, args: &[&str]) -> Result<Output, Box<dyn Error>> {
    let mut full = vec!["mcp"];
    full.extend_from_slice(args);
    yi(home, &full)
}

#[test]
fn disabled_config_hides_the_subcommand() -> TestResult {
    let home = mcp_home()?;
    std::fs::write(home.home.join(".yi/config.json"), "{}")?;
    let out = yi_mcp(&home, &["connect", "fixture"])?;
    assert_eq!(out.code, 2);
    assert!(out.stderr.contains("mcp is disabled"), "{}", out.stderr);
    std::fs::remove_dir_all(&home.home)?;
    Ok(())
}

#[test]
fn connect_list_call_grep_flow_matches_the_mcpc_examples() -> TestResult {
    let home = mcp_home()?;

    let connect = yi_mcp(&home, &["connect", "fixture", "@fx", "--json"])?;
    assert_eq!(connect.code, 0, "stderr: {}", connect.stderr);
    let connected: serde_json::Value = serde_json::from_str(connect.stdout.trim())?;
    assert_eq!(connected["session"], "@fx");
    assert_eq!(connected["state"], "live");
    assert_eq!(connected["server"]["serverInfo"]["name"], "yi-fixture");

    let listed = yi_mcp(&home, &["@fx", "tools-list", "--json"])?;
    assert_eq!(listed.code, 0, "stderr: {}", listed.stderr);
    let listing: serde_json::Value = serde_json::from_str(listed.stdout.trim())?;
    let names: Vec<&str> = listing["tools"]
        .as_array()
        .ok_or("tools not an array")?
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert_eq!(names, vec!["echo", "add"]);

    let called = yi_mcp(
        &home,
        &["@fx", "tools-call", "echo", "message:=hello mcp", "--json"],
    )?;
    assert_eq!(called.code, 0, "stderr: {}", called.stderr);
    let result: serde_json::Value = serde_json::from_str(called.stdout.trim())?;
    assert_eq!(result["content"][0]["text"], "hello mcp");

    let added = yi_mcp(
        &home,
        &["@fx", "tools-call", "add", "a:=2", "b:=40", "--json"],
    )?;
    let result: serde_json::Value = serde_json::from_str(added.stdout.trim())?;
    assert_eq!(result["content"][0]["text"], "42");

    let json_call = yi_mcp(
        &home,
        &[
            "@fx",
            "tools-call",
            "echo",
            r#"{"message":"from json"}"#,
            "--json",
        ],
    )?;
    let result: serde_json::Value = serde_json::from_str(json_call.stdout.trim())?;
    assert_eq!(result["content"][0]["text"], "from json");

    let grep_hit = yi_mcp(&home, &["grep", "echo", "--json"])?;
    assert_eq!(grep_hit.code, 0, "stderr: {}", grep_hit.stderr);
    let hits: serde_json::Value = serde_json::from_str(grep_hit.stdout.trim())?;
    assert_eq!(hits[0]["sessionName"], "fx");
    assert_eq!(hits[0]["tools"][0]["name"], "echo");

    let grep_miss = yi_mcp(&home, &["grep", "zebra-nonexistent"])?;
    assert_eq!(grep_miss.code, 1, "grep convention: 1 on no matches");

    let ping = yi_mcp(&home, &["@fx", "ping", "--json"])?;
    assert_eq!(ping.code, 0);

    std::fs::remove_dir_all(&home.home)?;
    Ok(())
}

#[test]
fn schema_snapshot_flags_breaking_changes() -> TestResult {
    let home = mcp_home()?;
    yi_mcp(&home, &["connect", "fixture", "@fx"])?;

    let tool = yi_mcp(&home, &["@fx", "tools-get", "echo", "--json"])?;
    assert_eq!(tool.code, 0, "stderr: {}", tool.stderr);
    let expected_path = home.home.join("expected.json");
    std::fs::write(&expected_path, tool.stdout.trim())?;

    let ok = yi_mcp(
        &home,
        &[
            "@fx",
            "tools-get",
            "echo",
            "--schema",
            &expected_path.display().to_string(),
            "--schema-mode",
            "strict",
        ],
    )?;
    assert_eq!(ok.code, 0, "stderr: {}", ok.stderr);

    let mut tampered: serde_json::Value = serde_json::from_str(tool.stdout.trim())?;
    tampered["inputSchema"]["required"] = serde_json::json!(["message", "new_required_field"]);
    std::fs::write(&expected_path, serde_json::to_string(&tampered)?)?;
    let broken = yi_mcp(
        &home,
        &[
            "@fx",
            "tools-get",
            "echo",
            "--schema",
            &expected_path.display().to_string(),
        ],
    )?;
    assert_eq!(broken.code, 3);
    assert!(
        broken.stderr.contains("schema mismatch"),
        "{}",
        broken.stderr
    );

    std::fs::remove_dir_all(&home.home)?;
    Ok(())
}

#[test]
fn sessions_persist_states_and_survive_close_restart() -> TestResult {
    let home = mcp_home()?;
    yi_mcp(&home, &["connect", "fixture", "@fx"])?;

    let closed = yi_mcp(&home, &["close", "@fx", "--json"])?;
    assert_eq!(closed.code, 0);
    let sessions: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(
        home.home.join(".yi/mcp/sessions.json"),
    )?)?;
    assert_eq!(sessions["sessions"]["fx"]["state"], "disconnected");

    let restarted = yi_mcp(&home, &["restart", "@fx", "--json"])?;
    assert_eq!(restarted.code, 0, "stderr: {}", restarted.stderr);
    let sessions: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(
        home.home.join(".yi/mcp/sessions.json"),
    )?)?;
    assert_eq!(sessions["sessions"]["fx"]["state"], "live");

    let unknown = yi_mcp(&home, &["@nope", "tools-list"])?;
    assert_eq!(unknown.code, 2);
    assert!(
        unknown.stderr.contains("unknown session"),
        "{}",
        unknown.stderr
    );

    std::fs::remove_dir_all(&home.home)?;
    Ok(())
}

#[test]
fn skill_document_is_printed_only_by_explicit_ask() -> TestResult {
    let home = mcp_home()?;
    let skill = yi_mcp(&home, &["help", "--skill"])?;
    assert_eq!(skill.code, 0);
    assert!(skill.stdout.contains("yi mcp: MCP command-line client"));
    assert!(skill.stdout.contains("progressive discovery") || skill.stdout.contains("grep"));
    std::fs::remove_dir_all(&home.home)?;
    Ok(())
}

#[test]
fn fetch_resolves_an_mcp_url_through_the_one_shot_cli() -> TestResult {
    let home = mcp_home()?;
    let connected = yi_mcp(&home, &["connect", "fixture", "@fx"])?;
    assert_eq!(connected.code, 0, "stderr: {}", connected.stderr);

    let served = yi(&home, &["fetch", "mcp://fx/note://alpha"])?;
    assert_eq!(served.code, 0, "stderr: {}", served.stderr);
    let contents: serde_json::Value = serde_json::from_str(served.stdout.trim())?;
    assert_eq!(contents["contents"][0]["uri"], "note://alpha");
    assert_eq!(contents["contents"][0]["text"], "alpha");

    let unconnected = yi(&home, &["fetch", "mcp://ghost/note://alpha"])?;
    assert_eq!(unconnected.code, 1);
    assert!(
        unconnected.stderr.contains("backing store failed")
            && unconnected.stderr.contains("unknown session @ghost"),
        "{}",
        unconnected.stderr
    );

    let addressless = yi(&home, &["fetch", "mcp://fx"])?;
    assert_eq!(addressless.code, 1);
    assert!(
        addressless.stderr.contains("<server>/<resource uri>"),
        "{}",
        addressless.stderr
    );

    std::fs::remove_dir_all(&home.home)?;
    Ok(())
}
