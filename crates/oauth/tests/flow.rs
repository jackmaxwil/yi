//! The refresh and login paths against a token endpoint on 127.0.0.1: refresh,
//! rotation, expiry, the cross-process lock, a state mismatch, and the refusal
//! messages — the behaviors D191's review named as untested.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::json;
use yi_oauth::flow;
use yi_oauth::registry::{self, OauthCode};
use yi_oauth::store::{self, Credential, Kind, Store};

type Res<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn spec(port: u16) -> OauthCode {
    OauthCode {
        id: "fixture".to_owned(),
        client_id: "test-client".to_owned(),
        client_secret: None,
        authorize: "https://example.invalid/authorize".to_owned(),
        token: format!("http://127.0.0.1:{port}/token"),
        scopes: String::new(),
        callback_port: 0,
        callback_path: "/callback".to_owned(),
        redirect_host: "localhost".to_owned(),
        port_fallback: false,
        json_token: false,
        extra_authorize: Vec::new(),
        stream_headers: Vec::new(),
        refresh_headers: Vec::new(),
    }
}

fn expired_credential() -> Credential {
    Credential {
        kind: Kind::Oauth,
        access: "old-access".to_owned(),
        refresh: Some("old-refresh".to_owned()),
        expires: Some(store::now() - Duration::from_secs(60)),
        account: None,
        org: None,
        extra: Default::default(),
    }
}

/// A token endpoint that answers `hits` requests with `body` and counts them. The
/// request is read to the end of its headers and the write side shut down before the
/// socket drops: closing with unread request bytes RSTs the response on macOS.
fn serve_tokens(body: String, hits: usize) -> Res<(u16, Arc<AtomicUsize>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let count = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&count);
    std::thread::spawn(move || {
        for _ in 0..hits {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            seen.fetch_add(1, Ordering::SeqCst);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let mut request = Vec::new();
            let mut buf = [0u8; 4096];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&buf[..n]),
                }
            }
            // A form body follows the headers; drain it to its content-length. A bare read
            // here sat out the whole 5 s timeout whenever the body came with the headers.
            let head = request
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .map_or(request.len(), |i| i + 4);
            let body_len = String::from_utf8_lossy(&request[..head])
                .lines()
                .find_map(|line| {
                    let line = line.to_ascii_lowercase();
                    line.strip_prefix("content-length:")?
                        .trim()
                        .parse::<usize>()
                        .ok()
                })
                .unwrap_or(0);
            while request.len() < head + body_len {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&buf[..n]),
                }
            }
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                    body.len(),
                    body
                )
                .as_bytes(),
            );
            let _ = stream.shutdown(std::net::Shutdown::Write);
            let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
            while stream.read(&mut buf).map(|n| n > 0).unwrap_or(false) {}
        }
    });
    Ok((port, count))
}

fn store(tag: &str) -> Store {
    let root = std::env::temp_dir().join(format!("yi-oauth-flow-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    Store::open(root)
}

#[test]
fn an_expired_token_refreshes_and_rotates() -> Res {
    let body = json!({
        "access_token": "new-access",
        "refresh_token": "new-refresh",
        "expires_in": 3600
    })
    .to_string();
    let (port, hits) = serve_tokens(body, 1)?;
    let store = store("refresh");
    store.save("fixture", &expired_credential())?;

    let live = flow::live_oauth(&spec(port), &store, None)?;
    assert_eq!(live.access, "new-access");
    assert_eq!(live.refresh.as_deref(), Some("new-refresh"));
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    // And it persisted: the next load is the rotated token.
    let on_disk = store.load("fixture").ok_or("saved")?;
    assert_eq!(on_disk.access, "new-access");
    assert_eq!(on_disk.refresh.as_deref(), Some("new-refresh"));
    Ok(())
}

#[test]
fn a_chatgpt_id_token_names_the_account_and_a_huge_expiry_does_not_panic() -> Res {
    // The payload is {"https://api.openai.com/auth":{"chatgpt_account_id":"acct-7"}}.
    let body = json!({
        "access_token": "new-access",
        "expires_in": u64::MAX,
        "id_token": "e30.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiYWNjdC03In19.sig"
    })
    .to_string();
    let (port, _) = serve_tokens(body, 1)?;
    let store = store("idtoken");
    store.save("fixture", &expired_credential())?;

    let live = flow::live_oauth(&spec(port), &store, None)?;
    assert_eq!(live.org.as_deref(), Some("acct-7"));
    Ok(())
}

#[test]
fn a_refresh_without_a_new_refresh_token_keeps_the_old_one() -> Res {
    let body = json!({"access_token": "new-access", "expires_in": 3600}).to_string();
    let (port, _) = serve_tokens(body, 1)?;
    let store = store("rotate");
    store.save("fixture", &expired_credential())?;

    let live = flow::live_oauth(&spec(port), &store, None)?;
    assert_eq!(live.refresh.as_deref(), Some("old-refresh"));
    Ok(())
}

#[test]
fn a_live_token_makes_no_network_call() -> Res {
    let (port, hits) = serve_tokens("{}".to_owned(), 1)?;
    let store = store("fresh");
    let mut current = expired_credential();
    current.expires = Some(store::now() + Duration::from_secs(3600));
    store.save("fixture", &current)?;

    let live = flow::live_oauth(&spec(port), &store, None)?;
    assert_eq!(live.access, "old-access");
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(hits.load(Ordering::SeqCst), 0, "no refresh, no request");
    Ok(())
}

#[test]
fn a_stale_lock_is_broken_and_the_refresh_proceeds() -> Res {
    let body = json!({"access_token": "new-access", "expires_in": 3600}).to_string();
    let (port, _) = serve_tokens(body, 1)?;
    let store = store("stale");
    store.save("fixture", &expired_credential())?;
    // A lock abandoned by a crashed process, older than the stale threshold.
    let locks = std::env::temp_dir()
        .join(format!("yi-oauth-flow-stale-{}", std::process::id()))
        .join("locks");
    std::fs::create_dir_all(&locks)?;
    let lock = locks.join("fixture.lock");
    std::fs::write(&lock, "pid")?;
    let old = std::fs::FileTimes::new().set_modified(store::now() - Duration::from_secs(120));
    std::fs::File::options()
        .write(true)
        .open(&lock)?
        .set_times(old)?;

    let live = flow::live_oauth(&spec(port), &store, None)?;
    assert_eq!(live.access, "new-access");
    Ok(())
}

#[test]
fn concurrent_expired_reads_refresh_once() -> Res {
    let body = json!({"access_token": "new-access", "expires_in": 3600}).to_string();
    let (port, hits) = serve_tokens(body, 8)?;
    let root = std::env::temp_dir().join(format!("yi-oauth-flow-once-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let store = Store::open(root.clone());
    store.save("fixture", &expired_credential())?;

    let mut threads = Vec::new();
    for _ in 0..4 {
        let root = root.clone();
        let spec = spec(port);
        threads.push(std::thread::spawn(move || {
            flow::live_oauth(&spec, &Store::open(root), None).map(|live| live.access)
        }));
    }
    for thread in threads {
        assert_eq!(thread.join().unwrap()?, "new-access");
    }
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "the lock funnels the refresh"
    );
    Ok(())
}

#[test]
fn a_state_mismatch_aborts_the_login() -> Res {
    // A free port for the loopback listener.
    let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
    let mut spec = spec(9); // the token endpoint is never reached
    spec.callback_port = port;
    let (token_port, _) = serve_tokens("{}".to_owned(), 0)?;
    spec.token = format!("http://127.0.0.1:{token_port}/token");

    let login = std::thread::spawn(move || {
        flow::login_oauth(&spec, &flow::LoginOptions { no_browser: true }, None)
    });
    // Wait for the listener, then answer with the wrong state.
    std::thread::sleep(Duration::from_millis(150));
    let mut conn = std::net::TcpStream::connect(("127.0.0.1", port))?;
    conn.write_all(b"GET /callback?code=x&state=wrong HTTP/1.1\r\nhost: localhost\r\n\r\n")?;
    let mut buf = [0u8; 1024];
    let _ = conn.read(&mut buf);

    let outcome = login.join().unwrap();
    let text = outcome.map(|_| String::new()).unwrap_err().to_string();
    assert!(text.contains("state mismatch"), "{text}");
    Ok(())
}

#[test]
fn a_denial_callback_says_so_instead_of_missing_code() -> Res {
    let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
    let mut spec = spec(9);
    spec.callback_port = port;

    let login = std::thread::spawn(move || {
        flow::login_oauth(&spec, &flow::LoginOptions { no_browser: true }, None)
    });
    std::thread::sleep(Duration::from_millis(150));
    let mut conn = std::net::TcpStream::connect(("127.0.0.1", port))?;
    conn.write_all(
        b"GET /callback?error=access_denied&error_description=nope HTTP/1.1\r\nhost: localhost\r\n\r\n",
    )?;
    let mut buf = [0u8; 1024];
    let _ = conn.read(&mut buf);

    let outcome = login.join().unwrap();
    let text = outcome.map(|_| String::new()).unwrap_err().to_string();
    assert!(
        text.contains("authorization failed: access_denied: nope"),
        "{text}"
    );
    Ok(())
}

#[test]
fn a_broken_profile_is_named_not_reported_missing() {
    // Unknown key: the typo `stream_header` must not silently send no headers.
    let bad = json!({
        "kind": "oauth-code",
        "client_id": "c",
        "authorize": "https://example.invalid/a",
        "token": "https://example.invalid/t",
        "callback_port": 1,
        "stream_header": {"user-agent": "x"}
    });
    let text = registry::parse("anthropic", &bad).unwrap_err().to_string();
    assert!(text.contains("unknown profile key"), "{text}");
    assert!(text.contains("stream_header"), "{text}");

    // Missing callback_port: named, not "no login profile".
    let missing = json!({
        "kind": "oauth-code",
        "client_id": "c",
        "authorize": "https://example.invalid/a",
        "token": "https://example.invalid/t"
    });
    let text = registry::parse("anthropic", &missing)
        .unwrap_err()
        .to_string();
    assert!(text.contains("callback_port"), "{text}");
}

#[test]
fn the_refusal_messages_name_the_login_verb() {
    let store = store("refusal");
    let text = flow::live_oauth(&spec(9), &store, None)
        .map(|_| String::new())
        .unwrap_err()
        .to_string();
    assert!(text.contains("run: yi login fixture"), "{text}");

    // An expired token with no refresh token says the same.
    let mut stranded = expired_credential();
    stranded.refresh = None;
    store.save("fixture", &stranded).unwrap();
    let text = flow::live_oauth(&spec(9), &store, None)
        .map(|_| String::new())
        .unwrap_err()
        .to_string();
    assert!(text.contains("run: yi login fixture"), "{text}");
}
