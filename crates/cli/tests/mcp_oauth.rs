use std::error::Error;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Stdio};

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

#[expect(
    clippy::disallowed_methods,
    reason = "the contract under test is real processes talking real HTTP"
)]
fn command(program: &str) -> std::process::Command {
    std::process::Command::new(program)
}

struct Fixture {
    child: Child,
    port: u16,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start_fixture() -> Result<Fixture, Box<dyn Error>> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mcp_oauth_fixture.py");
    let mut child = command("python3")
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let mut lines = BufReader::new(stdout).lines();
    let first = lines.next().ok_or("fixture died before printing port")??;
    let port: u16 = first
        .strip_prefix("PORT ")
        .ok_or("bad port line")?
        .trim()
        .parse()?;
    Ok(Fixture { child, port })
}

fn oauth_home(port: u16) -> Result<Scratch, Box<dyn Error>> {
    let home = Scratch::new(&format!("yi-mcp-oauth-{port}"))?;
    std::fs::create_dir_all(home.join(".yi"))?;
    std::fs::write(
        home.join(".yi/config.json"),
        r#"{"mcp": {"enabled": true, "tokenStore": "file"}}"#,
    )?;
    Ok(home)
}

struct Output {
    code: i32,
    stdout: String,
    stderr: String,
}

fn yi_mcp(home: &std::path::Path, args: &[&str]) -> Result<Output, Box<dyn Error>> {
    let output = command(env!("CARGO_BIN_EXE_yi"))
        .arg("mcp")
        .args(args)
        .env("HOME", home)
        .output()?;
    Ok(Output {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn curl(url: &str) -> Result<String, Box<dyn Error>> {
    let output = command("curl")
        .args(["-s", "-o", "/dev/null", "-w", "%{redirect_url}", url])
        .output()?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn drive_browser(auth_url: &str) -> Result<(), Box<dyn Error>> {
    let redirect = curl(auth_url)?;
    if redirect.is_empty() {
        return Err("authorize endpoint did not redirect".into());
    }
    let _ = command("curl").args(["-s", &redirect]).output()?;
    Ok(())
}

fn login(home: &std::path::Path, server: &str) -> Result<i32, Box<dyn Error>> {
    let mut child = command(env!("CARGO_BIN_EXE_yi"))
        .args(["mcp", "login", server, "--no-browser", "--json"])
        .env("HOME", home)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let mut lines = BufReader::new(stdout).lines();
    let mut auth_url = None;
    for line in lines.by_ref() {
        let line = line?;
        if let Some(url) = line.strip_prefix("Open this URL to authorize: ") {
            auth_url = Some(url.to_owned());
            break;
        }
    }
    drive_browser(&auth_url.ok_or("login never printed the authorize URL")?)?;
    let status = child.wait()?;
    Ok(status.code().unwrap_or(-1))
}

fn token_file(home: &std::path::Path, port: u16) -> PathBuf {
    home.join(".yi/mcp/tokens")
        .join(format!("default_127.0.0.1_{port}.json"))
}

#[test]
fn oauth_login_http_ops_refresh_and_logout_flow() -> TestResult {
    let fixture = start_fixture()?;
    let port = fixture.port;
    let home = oauth_home(port)?;
    let server = format!("http://127.0.0.1:{port}/mcp");

    assert_eq!(login(&home, &server)?, 0, "login must succeed");

    let tokens_path = token_file(&home, port);
    let stored: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&tokens_path)?)?;
    assert_eq!(stored["accessToken"], "tok-1");
    let profiles = std::fs::read_to_string(home.join(".yi/mcp/profiles.json"))?;
    assert!(
        !profiles.contains("tok-1") && !profiles.contains("ref-1"),
        "profiles.json must never hold token material"
    );

    let connect = yi_mcp(&home, &["connect", &server, "@web", "--json"])?;
    assert_eq!(connect.code, 0, "stderr: {}", connect.stderr);
    let connected: serde_json::Value = serde_json::from_str(connect.stdout.trim())?;
    assert_eq!(
        connected["server"]["serverInfo"]["name"],
        "yi-oauth-fixture"
    );

    let called = yi_mcp(
        &home,
        &["@web", "tools-call", "echo", "message:=over http", "--json"],
    )?;
    assert_eq!(called.code, 0, "stderr: {}", called.stderr);
    let result: serde_json::Value = serde_json::from_str(called.stdout.trim())?;
    assert_eq!(result["content"][0]["text"], "over http");

    let mut expired = stored;
    expired["expiresAtMs"] = serde_json::json!(1);
    std::fs::write(&tokens_path, serde_json::to_string(&expired)?)?;
    let refreshed_call = yi_mcp(&home, &["@web", "tools-list", "--json"])?;
    assert_eq!(refreshed_call.code, 0, "stderr: {}", refreshed_call.stderr);
    let after: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&tokens_path)?)?;
    assert_eq!(after["accessToken"], "tok-2", "expired token must refresh");
    assert_eq!(
        after["refreshToken"], "ref-2",
        "rotated refresh token must persist"
    );

    let logout = yi_mcp(&home, &["logout", &server, "--json"])?;
    assert_eq!(logout.code, 0);
    assert!(!tokens_path.exists(), "logout must delete stored tokens");
    let denied = yi_mcp(&home, &["@web", "tools-list"])?;
    assert_eq!(denied.code, 3);
    assert!(
        denied.stderr.contains("yi mcp login"),
        "after logout the error must name the login command: {}",
        denied.stderr
    );

    Ok(())
}

#[test]
fn a_401_never_starts_a_login_flow() -> TestResult {
    let fixture = start_fixture()?;
    let port = fixture.port;
    let home = oauth_home(port)?;
    let server = format!("http://127.0.0.1:{port}/mcp");

    let connect = yi_mcp(&home, &["connect", &server, "@web"])?;
    assert_eq!(connect.code, 3);
    assert!(
        connect.stderr.contains("unauthorized")
            && connect.stderr.contains(&format!("yi mcp login {server}")),
        "401 must hint the login command, never open a browser: {}",
        connect.stderr
    );
    let sessions: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(
        home.join(".yi/mcp/sessions.json"),
    )?)?;
    assert_eq!(sessions["sessions"]["web"]["state"], "unauthorized");

    Ok(())
}

#[test]
fn stale_refresh_locks_are_broken_not_fatal() -> TestResult {
    let fixture = start_fixture()?;
    let port = fixture.port;
    let home = oauth_home(port)?;
    let server = format!("http://127.0.0.1:{port}/mcp");
    assert_eq!(login(&home, &server)?, 0);
    yi_mcp(&home, &["connect", &server, "@web"])?;

    let tokens_path = token_file(&home, port);
    let mut stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&tokens_path)?)?;
    stored["expiresAtMs"] = serde_json::json!(1);
    std::fs::write(&tokens_path, serde_json::to_string(&stored)?)?;

    let locks = home.join(".yi/mcp/locks");
    std::fs::create_dir_all(&locks)?;
    let lock = locks.join(format!("default_127.0.0.1_{port}.lock"));
    std::fs::write(&lock, "")?;
    #[expect(
        clippy::disallowed_methods,
        reason = "backdating the lockfile mtime is the point of this staleness test"
    )]
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(300);
    let _ = filetime_set(&lock, old);

    let call = yi_mcp(&home, &["@web", "tools-list", "--json"])?;
    assert_eq!(call.code, 0, "stale lock must be broken: {}", call.stderr);

    Ok(())
}

fn filetime_set(path: &std::path::Path, to: std::time::SystemTime) -> std::io::Result<()> {
    let file = std::fs::OpenOptions::new().write(true).open(path)?;
    file.set_modified(to)?;
    file.sync_all()
}
