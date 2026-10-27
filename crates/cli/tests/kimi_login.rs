//! A Kimi account end to end through the real binary, offline: the device login, the model list
//! it refreshes, the usage windows and one tool turn. A loopback fake replays what the account
//! answered on 2026-10-24 (crates/ai/tests/fixtures/kimi-*-2026-10-24.*), scrubbed of the token.

use std::error::Error;
use std::io::{BufReader, Write};
use std::path::Path;
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::loopback::read_request;
use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

const MODELS: &str = include_str!("../../ai/tests/fixtures/kimi-models-2026-10-24.json");
const USAGES: &str = include_str!("../../ai/tests/fixtures/kimi-usages-2026-10-24.json");
const BASH_TURN: &str = include_str!("../../ai/tests/fixtures/kimi-chat-2026-10-24-bash.sse");
const ANSWER_TURN: &str = include_str!("../../ai/tests/fixtures/kimi-chat-2026-10-24-answer.sse");

type Seen = Arc<Mutex<Vec<(String, String)>>>;

fn reply(status: &str, kind: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: {kind}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// The account's endpoints on 127.0.0.1; `denied` answers the token poll with `access_denied`.
/// Every request is kept as (request line, headers and body).
fn fake_kimi(denied: bool) -> std::io::Result<(u16, Seen)> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let seen: Seen = Arc::default();
    let kept = Arc::clone(&seen);
    std::thread::spawn(move || {
        let mut chats = 0usize;
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream);
            let request = read_request(&mut reader);
            let Some((start, head)) = request.split_once("\r\n") else {
                continue;
            };
            let (start, head) = (start.to_owned(), head.to_owned());
            let path = start
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_owned();
            let answer = match path.as_str() {
                "/device" => reply(
                    "200 OK",
                    "application/json",
                    &json!({
                        "device_code": "device-1", "user_code": "ABCD-1234",
                        "verification_uri": format!("http://127.0.0.1:{port}/verify"),
                        "interval": 1, "expires_in": 60,
                    })
                    .to_string(),
                ),
                "/token" if denied => reply(
                    "400 Bad Request",
                    "application/json",
                    r#"{"error":"access_denied"}"#,
                ),
                "/token" => reply(
                    "200 OK",
                    "application/json",
                    r#"{"access_token":"kimi-test-access","refresh_token":"kimi-test-refresh","expires_in":3600}"#,
                ),
                "/coding/v1/models" => reply("200 OK", "application/json", MODELS),
                "/coding/v1/usages" => reply("200 OK", "application/json", USAGES),
                "/coding/v1/chat/completions" => {
                    chats = chats.saturating_add(1);
                    let turn = if chats == 1 { BASH_TURN } else { ANSWER_TURN };
                    reply("200 OK", "text/event-stream", turn)
                }
                _ => reply("404 Not Found", "application/json", "{}"),
            };
            let _ = reader.get_mut().write_all(answer.as_bytes());
            if let Ok(mut all) = kept.lock() {
                all.push((start, head));
            }
        }
    });
    Ok((port, seen))
}

struct Account {
    dir: Scratch,
    home: std::path::PathBuf,
}

impl Account {
    fn new(tag: &str, port: u16) -> Result<Self, Box<dyn Error>> {
        let dir = Scratch::new(&format!("yi-kimi-{tag}"))?;
        let home = dir.home()?;
        std::fs::create_dir_all(dir.join("project"))?;
        std::fs::create_dir_all(home.join(".yi/oauth"))?;
        let base = format!("http://127.0.0.1:{port}");
        std::fs::write(
            home.join(".yi/oauth/kimi.json"),
            json!({
                "kind": "device-code",
                "client_id": "yi-test-client",
                "device_authorization": format!("{base}/device"),
                "token": format!("{base}/token"),
                "api_base": format!("{base}/coding/v1"),
            })
            .to_string(),
        )?;
        Ok(Self { dir, home })
    }

    fn yi(&self, args: &[&str]) -> std::io::Result<Output> {
        #[expect(
            clippy::disallowed_methods,
            reason = "the journey is the spawned binary's argv, exit code, stdout and files"
        )]
        let mut command = Command::new(env!("CARGO_BIN_EXE_yi"));
        command
            .args(args)
            .env("HOME", &self.home)
            .env_remove("HTTPS_PROXY")
            .env_remove("HTTP_PROXY")
            .current_dir(self.dir.join("project"))
            .output()
    }
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The recorded turn's reasoning, joined from its own deltas: the text Moonshot needs echoed.
fn recorded_reasoning() -> String {
    BASH_TURN
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str::<Value>(data).ok())
        .filter_map(|chunk| {
            chunk["choices"][0]["delta"]["reasoning_content"]
                .as_str()
                .map(str::to_owned)
        })
        .collect()
}

fn mode(path: &Path) -> std::io::Result<u32> {
    use std::os::unix::fs::PermissionsExt;
    Ok(std::fs::metadata(path)?.permissions().mode() & 0o777)
}

/// The first `--model kimi/<id>` after a login resolves, because the login fetched the account's
/// list: a K3.1 id is never compiled in. The tool turn goes out the way Moonshot checks it.
#[test]
fn a_device_login_refreshes_the_list_and_a_kimi_turn_streams_through_it() -> TestResult {
    let (port, seen) = fake_kimi(false)?;
    let account = Account::new("login", port)?;
    let login = account.yi(&["login", "kimi", "--no-browser"])?;
    assert_eq!(login.status.code(), Some(0), "{}", text(&login));
    let listed: Value = serde_json::from_str(MODELS)?;
    let printed = String::from_utf8_lossy(&login.stdout);
    for id in listed["data"]
        .as_array()
        .ok_or("models fixture has no data")?
        .iter()
        .filter_map(|entry| entry["id"].as_str())
    {
        assert!(
            printed.lines().any(|line| line == format!("kimi/{id}")),
            "{id} not printed: {printed}"
        );
    }
    assert_eq!(
        mode(&account.home.join(".yi/providers/tokens/kimi.json"))?,
        0o600
    );

    let usage = account.yi(&["catalog", "usage", "kimi"])?;
    assert_eq!(usage.status.code(), Some(0), "{}", text(&usage));
    let windows: Value = serde_json::from_slice(&usage.stdout)?;
    assert_eq!(
        windows,
        json!({
            "5h": {"used": 0.0, "reset": "2026-10-24T19:28:25Z"},
            "7d": {"used": 0.0, "reset": "2026-10-30T06:28:25Z"},
        })
    );

    let ask = account.yi(&[
        "ask",
        "--json",
        "--auto",
        "--model",
        "kimi/k3.1-pro",
        "Run echo one with the bash tool, then answer with its output.",
    ])?;
    assert_eq!(ask.status.code(), Some(0), "{}", text(&ask));
    let events = String::from_utf8_lossy(&ask.stdout);
    let last_line = events.lines().last().ok_or("no events")?;
    let stop: Value = serde_json::from_str(last_line)?;
    assert_eq!(stop["type"], "final", "a json run ends by saying why");
    let end = events.lines().rev().nth(1).ok_or("no agent_end")?;
    let last: Value = serde_json::from_str(end)?;
    let messages = last["messages"].as_array().ok_or("no messages")?;
    assert!(
        messages.iter().any(|message| message["content"]
            .as_array()
            .is_some_and(|blocks| blocks.iter().any(|block| block["name"] == "bash"))),
        "the recorded bash call did not run: {last}"
    );
    let answered = messages
        .iter()
        .rev()
        .find_map(|message| message["content"][0]["text"].as_str())
        .ok_or("no final text")?;
    assert_eq!(answered, "```text\none\ntwo\nthree\n```");

    let chats: Vec<String> = seen
        .lock()
        .map_err(|_| "fake lock")?
        .iter()
        .filter(|(start, _)| start.starts_with("POST /coding/v1/chat/completions"))
        .map(|(_, request)| request.clone())
        .collect();
    let second = chats.get(1).ok_or("no second chat request")?;
    assert!(
        second.to_ascii_lowercase().contains("user-agent: yi/"),
        "{second}"
    );
    let body: Value = serde_json::from_str(second.rsplit("\r\n").next().unwrap_or_default())?;
    assert_eq!(
        body["thinking"],
        json!({"type": "enabled", "effort": "high"})
    );
    assert!(body["prompt_cache_key"].is_string(), "{body}");
    let replayed = body["messages"]
        .as_array()
        .ok_or("no messages")?
        .iter()
        .find(|message| message["role"] == "assistant")
        .and_then(|message| message["reasoning_content"].as_str())
        .ok_or("no reasoning_content on the assistant turn")?;
    assert_eq!(replayed, recorded_reasoning());
    Ok(())
}

/// A login the browser denied leaves no list behind, even when an older token is still stored:
/// a refresh then would read the old account's models as this login's.
#[test]
fn a_login_that_fails_writes_no_catalog() -> TestResult {
    let (port, seen) = fake_kimi(true)?;
    let account = Account::new("denied", port)?;
    let tokens = account.home.join(".yi/providers/tokens");
    std::fs::create_dir_all(&tokens)?;
    std::fs::write(
        tokens.join("kimi.json"),
        r#"{"version":1,"kind":"oauth","access":"older-access","expires_ms":0}"#,
    )?;
    let login = account.yi(&["login", "kimi", "--no-browser"])?;
    assert_eq!(login.status.code(), Some(1), "{}", text(&login));
    assert!(
        text(&login).contains("kimi: the login was denied in the browser"),
        "{}",
        text(&login)
    );
    assert!(!account.home.join(".yi/catalog/kimi.json").exists());
    let listed = seen
        .lock()
        .map_err(|_| "fake lock")?
        .iter()
        .filter(|(start, _)| start.contains("/models"))
        .count();
    assert_eq!(listed, 0);
    Ok(())
}

/// `yi login --list kimi` lists profiles and logs nothing in, so it fetches no model list and says
/// no "logged in" over a stored token it never touched.
#[test]
fn listing_profiles_refreshes_nothing() -> TestResult {
    let (port, seen) = fake_kimi(false)?;
    let account = Account::new("list", port)?;
    let tokens = account.home.join(".yi/providers/tokens");
    std::fs::create_dir_all(&tokens)?;
    std::fs::write(
        tokens.join("kimi.json"),
        r#"{"version":1,"kind":"oauth","access":"stored-access","expires_ms":0}"#,
    )?;
    let listing = account.yi(&["login", "--list", "kimi"])?;
    assert_eq!(listing.status.code(), Some(0), "{}", text(&listing));
    assert!(!text(&listing).contains("logged in"), "{}", text(&listing));
    assert!(!account.home.join(".yi/catalog/kimi.json").exists());
    assert_eq!(seen.lock().map_err(|_| "fake lock")?.len(), 0);
    Ok(())
}

#[test]
fn a_refresh_for_a_run_uses_the_profile_and_never_prints_the_refresh_token() -> TestResult {
    let (port, seen) = fake_kimi(false)?;
    let account = Account::new("refresh-for-run", port)?;
    let tokens = account.home.join(".yi/providers/tokens");
    std::fs::create_dir_all(&tokens)?;
    std::fs::write(
        tokens.join("kimi.json"),
        r#"{"version":1,"kind":"oauth","access":"old-access","refresh":"old-refresh","expires_ms":1}"#,
    )?;
    let profile = account.home.join(".yi/oauth/kimi.json");
    let mut value: Value = serde_json::from_str(&std::fs::read_to_string(&profile)?)?;
    value["refresh_headers"] = json!({"x-fixture-refresh": "present"});
    std::fs::write(profile, value.to_string())?;

    let refreshed = account.yi(&["login", "kimi", "--refresh", "1800"])?;
    assert_eq!(refreshed.status.code(), Some(0), "{}", text(&refreshed));
    let access: Value = serde_json::from_slice(&refreshed.stdout)?;
    assert_eq!(access["access"], "kimi-test-access");
    assert!(access.get("refresh").is_none(), "{access}");
    let central: Value = serde_json::from_str(&std::fs::read_to_string(tokens.join("kimi.json"))?)?;
    assert_eq!(central["refresh"], "kimi-test-refresh");
    let requests = seen
        .lock()
        .map_err(|_| "fake lock")?
        .iter()
        .map(|(start, head)| format!("{start}\n{head}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        requests.contains("x-fixture-refresh: present"),
        "{requests}"
    );
    assert!(
        requests.contains("grant_type=refresh_token&refresh_token=old-refresh"),
        "{requests}"
    );
    assert!(
        !requests.contains("/models"),
        "a token refresh does not refresh the catalog: {requests}"
    );

    let too_short = account.yi(&["login", "kimi", "--refresh", "7200"])?;
    assert_eq!(too_short.status.code(), Some(1), "{}", text(&too_short));
    assert!(
        text(&too_short).contains("does not live 7200 s"),
        "{}",
        text(&too_short)
    );
    Ok(())
}
