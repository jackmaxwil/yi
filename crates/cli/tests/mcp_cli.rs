use std::error::Error;
use std::path::PathBuf;

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

struct McpHome {
    home: Scratch,
}

fn fixture_server_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mcp_fixture_server.py")
}

/// Isolated HOME with mcp enabled and a fixture stdio server entry.
fn mcp_home() -> Result<McpHome, Box<dyn Error>> {
    let home = Scratch::new("yi-mcp-e2e")?;
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

/// `yi <args>` run from `cwd`, the directory a stored relative command would resolve against.
fn yi_in(home: &McpHome, cwd: &std::path::Path, args: &[&str]) -> Result<Output, Box<dyn Error>> {
    #[expect(
        clippy::disallowed_methods,
        reason = "the contract under test is the spawned binary's stdio"
    )]
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_yi"))
        .args(args)
        .env("HOME", &home.home)
        .current_dir(cwd)
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
    Ok(())
}

#[test]
fn connect_list_call_grep_flow_matches_the_mcpc_examples() -> TestResult {
    let home = mcp_home()?;

    let connect = yi_mcp(&home, &["connect", "fixture", "@s", "--json"])?;
    assert_eq!(connect.code, 0, "stderr: {}", connect.stderr);
    let connected: serde_json::Value = serde_json::from_str(connect.stdout.trim())?;
    assert_eq!(connected["session"], "@s");
    assert_eq!(connected["state"], "live");
    assert_eq!(connected["server"]["serverInfo"]["name"], "yi-fixture");

    let listed = yi_mcp(&home, &["@s", "tools-list", "--json"])?;
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
        &["@s", "tools-call", "echo", "message:=hello mcp", "--json"],
    )?;
    assert_eq!(called.code, 0, "stderr: {}", called.stderr);
    let result: serde_json::Value = serde_json::from_str(called.stdout.trim())?;
    assert_eq!(result["content"][0]["text"], "hello mcp");

    let added = yi_mcp(
        &home,
        &["@s", "tools-call", "add", "a:=2", "b:=40", "--json"],
    )?;
    let result: serde_json::Value = serde_json::from_str(added.stdout.trim())?;
    assert_eq!(result["content"][0]["text"], "42");

    let json_call = yi_mcp(
        &home,
        &[
            "@s",
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
    assert_eq!(hits[0]["sessionName"], "s");
    assert_eq!(hits[0]["tools"][0]["name"], "echo");

    let grep_miss = yi_mcp(&home, &["grep", "zebra-nonexistent"])?;
    assert_eq!(grep_miss.code, 1, "grep convention: 1 on no matches");

    let ping = yi_mcp(&home, &["@s", "ping", "--json"])?;
    assert_eq!(ping.code, 0);

    Ok(())
}

#[test]
fn schema_snapshot_flags_breaking_changes() -> TestResult {
    let home = mcp_home()?;
    yi_mcp(&home, &["connect", "fixture", "@s"])?;

    let tool = yi_mcp(&home, &["@s", "tools-get", "echo", "--json"])?;
    assert_eq!(tool.code, 0, "stderr: {}", tool.stderr);
    let expected_path = home.home.join("expected.json");
    std::fs::write(&expected_path, tool.stdout.trim())?;

    let ok = yi_mcp(
        &home,
        &[
            "@s",
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
            "@s",
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

    Ok(())
}

#[test]
fn sessions_persist_states_and_survive_close_restart() -> TestResult {
    let home = mcp_home()?;
    yi_mcp(&home, &["connect", "fixture", "@s"])?;

    let closed = yi_mcp(&home, &["close", "@s", "--json"])?;
    assert_eq!(closed.code, 0);
    let sessions: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(
        home.home.join(".yi/mcp/sessions.json"),
    )?)?;
    assert_eq!(sessions["sessions"]["s"]["state"], "disconnected");

    let restarted = yi_mcp(&home, &["restart", "@s", "--json"])?;
    assert_eq!(restarted.code, 0, "stderr: {}", restarted.stderr);
    let sessions: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(
        home.home.join(".yi/mcp/sessions.json"),
    )?)?;
    assert_eq!(sessions["sessions"]["s"]["state"], "live");

    let unknown = yi_mcp(&home, &["@nope", "tools-list"])?;
    assert_eq!(unknown.code, 2);
    assert!(
        unknown.stderr.contains("unknown session"),
        "{}",
        unknown.stderr
    );

    Ok(())
}

#[test]
fn skill_document_is_printed_only_by_explicit_ask() -> TestResult {
    let home = mcp_home()?;
    let skill = yi_mcp(&home, &["help", "--skill"])?;
    assert_eq!(skill.code, 0);
    assert!(skill.stdout.contains("yi mcp: MCP command-line client"));
    assert!(skill.stdout.contains("progressive discovery") || skill.stdout.contains("grep"));
    Ok(())
}

#[test]
fn fetch_resolves_an_mcp_url_through_the_one_shot_cli() -> TestResult {
    let home = mcp_home()?;
    let connected = yi_mcp(&home, &["connect", "fixture", "@s"])?;
    assert_eq!(connected.code, 0, "stderr: {}", connected.stderr);

    let served = yi(&home, &["fetch", "mcp://s/note://alpha"])?;
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

    let addressless = yi(&home, &["fetch", "mcp://s"])?;
    assert_eq!(addressless.code, 1);
    assert!(
        addressless.stderr.contains("<server>/<resource uri>"),
        "{}",
        addressless.stderr
    );

    Ok(())
}

/// A stored stdio command that is a relative path is resolved against the server's cwd, the
/// way `npx` prefers `<cwd>/node_modules/.bin`. The host runs every MCP server from `~/.yi`,
/// outside every writable root, so a file planted in the workspace is never the one that runs.
#[test]
fn a_relative_server_command_is_not_resolved_in_the_workspace() -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    let home = mcp_home()?;
    let workspace = Scratch::new("yi-mcp-cwd")?;
    let bin = workspace.join("node_modules").join(".bin");
    std::fs::create_dir_all(&bin)?;
    let marker = home.home.join("yi-mcp-cwd-marker");
    let script = bin.join("yi-mcp-relative");
    std::fs::write(
        &script,
        format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
    )?;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))?;
    let config = home.home.join(".yi/mcp.json");
    let mut entries: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&config)?)?;
    entries["mcpServers"]["relative"] =
        serde_json::json!({"command": "node_modules/.bin/yi-mcp-relative"});
    std::fs::write(&config, entries.to_string())?;

    let connected = yi_in(
        &home,
        &workspace,
        &["mcp", "connect", "relative", "@rel", "--json"],
    )?;
    let ran_at_connect = marker.is_file();
    let _ = std::fs::remove_file(&marker);
    let fetched = yi_in(&home, &workspace, &["fetch", "mcp://rel/note://alpha"])?;
    let ran_at_fetch = marker.is_file();
    assert!(
        !ran_at_connect && !ran_at_fetch,
        "the host ran a file planted in the workspace (connect {ran_at_connect}, fetch {ran_at_fetch}): {} {}",
        connected.stderr,
        fetched.stderr
    );
    assert_ne!(connected.code, 0);
    assert!(
        connected
            .stderr
            .contains("could not start `node_modules/.bin/yi-mcp-relative` (MCP servers start in"),
        "the spawn fails by name and says where servers start: {}",
        connected.stderr
    );
    Ok(())
}

/// Incident (#584): a root kernel cell wrote a stdio entry into `~/.yi/mcp/sessions.json`, a
/// kernel writable root, and `fetch("mcp://<entry>/x")`, a read Auto mode never asks about,
/// made the unsandboxed host run its command. The store is host-only now (D296).
#[cfg(target_os = "macos")]
mod sealed_store {
    use super::{Scratch, TestResult};
    use std::path::PathBuf;
    use std::sync::Arc;
    use yi_runtime::{HostRegistry, KernelService, KernelServiceOptions};
    use yi_tools::{CancelFlag, KernelBridge, Sandbox};

    /// The host half of `fetch("mcp://…")`: `yi mcp --json @<server> resources-read <uri>`,
    /// run in-process past the `mcp.enabled` gate so the test needs no config in HOME.
    fn fetch_through_yi_mcp(registry: &mut HostRegistry) {
        registry.register("fetch", |payload| {
            Box::pin(async move {
                let url = payload
                    .get("url")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                let (server, uri) = url
                    .strip_prefix("mcp://")
                    .and_then(|rest| rest.split_once('/'))
                    .ok_or_else(|| format!("not an mcp url: {url}"))?;
                let args =
                    ["--json", &format!("@{server}"), "resources-read", uri].map(str::to_owned);
                match yi_mcp_cli::run(&args, None) {
                    0 => Ok(serde_json::Map::from_iter([(
                        "text".to_owned(),
                        "ok".into(),
                    )])),
                    code => Err(format!("yi mcp exited {code}")),
                }
            })
        });
    }

    #[tokio::test]
    async fn a_cell_cannot_plant_a_session_the_host_runs_on_a_fetch() -> TestResult {
        if !Sandbox::available() {
            return Ok(());
        }
        let root = Scratch::new("yi-584")?;
        let project = root.join("project");
        std::fs::create_dir_all(&project)?;
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or("HOME is unset")?;
        let sandbox = Sandbox::for_workspace(&project, &home, None);
        // Under a tmp HOME the store sits in a granted root and the test would prove nothing.
        let resolve = |path: &std::path::Path| path.canonicalize().unwrap_or(path.to_path_buf());
        if sandbox
            .writable
            .iter()
            .any(|writable| resolve(&home).starts_with(resolve(writable)))
        {
            return Ok(());
        }
        let store = home.join(".yi").join("mcp").join("sessions.json");
        let before = std::fs::read_to_string(&store).ok();
        let marker = home.join(format!("yi-584-marker-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let entry = format!("yi584-{}", std::process::id());
        let mut registry = HostRegistry::default();
        registry.register_mcp_stubs();
        fetch_through_yi_mcp(&mut registry);
        let kernel = Arc::new(KernelService::new(KernelServiceOptions {
            cwd: project,
            home: home.clone(),
            session_dir: None,
            family_dir: None,
            host: Arc::new(registry),
            on_restore: None,
            on_boot: None,
            sandbox: Some(sandbox),
            snapshot_key: None,
            per_session_state: false,
            cell_ceiling: None,
        }));
        let code = format!(
            "import json, os\nstore = r'{store}'\nrecord = {{'name': '{entry}', 'spec': {{'command': '/usr/bin/touch', 'args': [r'{marker}']}}, 'state': 'live', 'createdAt': 0, 'updatedAt': 0}}\ntry:\n    os.makedirs(os.path.dirname(store), exist_ok=True)\n    sessions = json.load(open(store)) if os.path.exists(store) else {{'sessions': {{}}}}\n    sessions['sessions']['{entry}'] = record\n    json.dump(sessions, open(store, 'w'))\n    print('planted')\nexcept OSError as e:\n    print('write', e.errno)\ntry:\n    print(await fetch('mcp://{entry}/x'))\nexcept Exception as e:\n    print('fetch:', e)",
            store = store.display(),
            marker = marker.display(),
        );
        let ran = tokio::task::spawn_blocking({
            let kernel = Arc::clone(&kernel);
            move || {
                let cancelled: CancelFlag = Arc::new(|| false);
                KernelBridge::execute_cell(kernel.as_ref(), &code, &cancelled, None)
            }
        })
        .await?;
        kernel.dispose().await;
        let after = std::fs::read_to_string(&store).ok();
        let ran_planted_command = marker.is_file();
        let _ = std::fs::remove_file(&marker);
        match &before {
            Some(text) => std::fs::write(&store, text)?,
            None => {
                let _ = std::fs::remove_file(&store);
            }
        }
        let stdout = ran?.result.stdout;
        assert!(
            !ran_planted_command,
            "the host ran a command a cell planted in the session store: {stdout}"
        );
        // `yi mcp` exits 2 for an unknown session and 3 once a server was spawned and failed.
        assert!(
            stdout.contains("write 1\n") && stdout.contains("fetch: yi mcp exited 2\n"),
            "the write must fail with EPERM and the fetch find no session: {stdout}"
        );
        assert_eq!(after, before, "the store must be untouched: {stdout}");
        Ok(())
    }
}
