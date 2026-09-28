use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;

use serde_json::json;
use yi_ai::request::{ProxyConfig, send_with_retry};

/// A transport probe, not a model request: nothing to mark, so it takes the constructor for
/// wires without explicit breakpoints and reaches ureq the only way a body can.
fn transport_probe() -> yi_ai::breakpoints::Encoded {
    yi_ai::breakpoints::Encoded::provider_prefix(json!({"model": "probe"}), "messages", Vec::new())
}

type Res = Result<(), Box<dyn std::error::Error>>;

/// A typo'd HTTPS_PROXY that resolved to "no proxy" would send provider traffic
/// direct: in an air-gapped runner that is a hang nobody can attribute, so the
/// value has to be named back at the operator.
#[test]
fn a_malformed_proxy_value_is_refused_by_name() -> Res {
    let error = ProxyConfig::from_values(Some("ftp://squid:3128"), None, None)
        .err()
        .ok_or("a proxy scheme ureq cannot dial was accepted")?;
    assert!(error.contains("ftp://squid:3128"), "{error}");

    let socks = ProxyConfig::from_values(Some("socks5://squid:1080"), None, None)
        .err()
        .ok_or("socks was accepted, but the socks feature is not compiled in")?;
    assert!(socks.contains("socks5://squid:1080"), "{socks}");
    Ok(())
}

/// The refusal is printed to stderr, where CI logs and scrollback keep it: a
/// credentialed proxy url must be named back by host, never by password.
#[test]
fn a_refused_proxy_url_names_the_host_without_its_credentials() -> Res {
    for (value, host) in [
        ("https://bob:hunter2@proxy.corp:3128", "proxy.corp:3128"),
        (
            "https://bob:hunter2@proxy.corp:3128/route",
            "proxy.corp:3128",
        ),
        ("socks5://bob:hunter2@proxy.corp:1080", "proxy.corp:1080"),
        ("socks5:bob:hunter2@proxy.corp:1080", "proxy.corp:1080"),
    ] {
        let error = ProxyConfig::from_values(Some(value), None, None)
            .err()
            .ok_or("a proxy value that cannot be dialed was accepted")?;
        assert!(!error.contains("hunter2"), "password named back: {error}");
        assert!(!error.contains("bob:"), "username named back: {error}");
        assert!(
            error.contains(host),
            "the operator cannot recognise {value}: {error}"
        );
    }
    Ok(())
}

/// An empty variable is how a container turns the proxy off; treating "" as a
/// proxy url would fail startup for every unproxied run.
#[test]
fn an_absent_or_empty_value_configures_no_proxy() -> Res {
    assert!(ProxyConfig::from_values(None, None, None)?.is_none());
    assert!(ProxyConfig::from_values(Some(""), Some("  "), Some("*"))?.is_none());
    Ok(())
}

/// HTTPS_PROXY losing to HTTP_PROXY, or NO_PROXY failing to exempt a host,
/// both send provider traffic to the wrong place.
#[test]
fn https_outranks_http_and_no_proxy_exempts_by_suffix() -> Res {
    let config = ProxyConfig::from_values(
        Some("http://chosen:3128"),
        Some("http://ignored:3128"),
        Some("api.internal, .example.com"),
    )?
    .ok_or("two proxy values yielded no config")?;
    let expected = ureq::Proxy::new("http://chosen:3128")?;
    assert_eq!(config.proxy_for("api.anthropic.com"), Some(&expected));
    assert_eq!(config.proxy_for("api.internal"), None);
    assert_eq!(config.proxy_for("API.Internal"), None);
    assert_eq!(config.proxy_for("gw.example.com"), None);
    assert_eq!(config.proxy_for("notexample.com"), Some(&expected));

    let http_only = ProxyConfig::from_values(None, Some("http://fallback:3128"), None)?
        .ok_or("HTTP_PROXY alone yielded no config")?;
    assert_eq!(
        http_only.proxy_for("api.anthropic.com"),
        Some(&ureq::Proxy::new("http://fallback:3128")?)
    );

    let star = ProxyConfig::from_values(Some("http://chosen:3128"), None, Some("*"))?
        .ok_or("NO_PROXY=* yielded no config")?;
    assert_eq!(star.proxy_for("api.anthropic.com"), None);
    Ok(())
}

/// The configuration can parse and still never reach the agent. This asserts the
/// request arrived at the proxy in absolute form, which only happens when ureq
/// was actually built with the proxy: 127.0.0.1 stands in for the sidecar and
/// `yi.invalid` cannot resolve, so a direct request could not produce it.
#[test]
fn a_provider_request_reaches_the_configured_proxy() -> Res {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let proxy = std::thread::spawn(move || -> std::io::Result<String> {
        let (stream, _) = listener.accept()?;
        let mut reader = BufReader::new(stream);
        let mut request_line = String::new();
        reader.read_line(&mut request_line)?;
        let mut length = 0usize;
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header)? == 0 || header.trim().is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                length = value.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0u8; length];
        reader.read_exact(&mut body)?;
        reader
            .into_inner()
            .write_all(b"HTTP/1.1 400 Bad Request\r\ncontent-length: 0\r\n\r\n")?;
        Ok(request_line)
    });

    let config = ProxyConfig::from_values(Some(&format!("http://127.0.0.1:{port}")), None, None)?
        .ok_or("loopback proxy value yielded no config")?;
    let sent = send_with_retry(
        "http://yi.invalid/v1/messages",
        &[],
        &transport_probe(),
        Some(&config),
        &|_| {},
    );

    let error = sent.err().ok_or("the 400 reply was reported as success")?;
    assert!(
        error.starts_with("HTTP 400"),
        "the request never reached the loopback proxy: {error}"
    );
    let request_line = proxy.join().map_err(|_| "proxy thread panicked")??;
    assert!(
        request_line.starts_with("POST http://yi.invalid/v1/messages"),
        "proxy saw {request_line:?}, not an absolute-form request"
    );
    Ok(())
}

fn serve_ok_until_closed(stream: std::net::TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    let mut line = String::new();
    while reader.read_line(&mut line)? > 0 {
        let mut length = 0usize;
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header)? == 0 || header.trim().is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                length = value.trim().parse().unwrap_or(0);
            }
        }
        reader.read_exact(&mut vec![0u8; length])?;
        writer.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")?;
        line.clear();
    }
    Ok(())
}

/// Incident: every turn built a new agent, so every request paid a connect and, against a
/// provider, a TLS handshake; back-to-back requests now ride one pooled connection.
#[test]
fn back_to_back_requests_share_one_connection() -> Res {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let url = format!(
        "http://127.0.0.1:{}/v1/messages",
        listener.local_addr()?.port()
    );
    let accepted = std::sync::Arc::new(AtomicUsize::new(0));
    let counter = std::sync::Arc::clone(&accepted);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            counter.fetch_add(1, Ordering::SeqCst);
            std::thread::spawn(move || serve_ok_until_closed(stream));
        }
    });
    for _ in 0..3 {
        let response = send_with_retry(&url, &[], &transport_probe(), None, &|_| {})?;
        assert_eq!(response.into_string()?, "ok");
    }
    assert_eq!(
        accepted.load(Ordering::SeqCst),
        1,
        "a connection per request"
    );
    Ok(())
}

/// Dies with the retry slept out in silence: a 429 held the turn for its whole backoff while
/// the screen said only that the model was being waited on.
#[test]
fn a_retried_request_says_which_attempt_and_how_long_it_waits() -> Res {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = std::thread::spawn(move || -> std::io::Result<()> {
        for reply in [
            "HTTP/1.1 429 Too Many Requests\r\nretry-after-ms: 10\r\ncontent-length: 0\r\n\r\n",
            "HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}",
        ] {
            let (stream, _) = listener.accept()?;
            let mut reader = BufReader::new(stream);
            let mut length = 0usize;
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header)? == 0 || header.trim().is_empty() {
                    break;
                }
                if let Some((name, value)) = header.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; length];
            reader.read_exact(&mut body)?;
            reader.into_inner().write_all(reply.as_bytes())?;
        }
        Ok(())
    });
    let seen = std::sync::Mutex::new(Vec::new());
    let sent = send_with_retry(
        &format!("http://127.0.0.1:{port}/v1/messages"),
        &[],
        &transport_probe(),
        None,
        &|wait| seen.lock().map(|mut seen| seen.push(wait)).unwrap_or(()),
    );
    assert!(
        sent.is_ok(),
        "the second attempt succeeds: {:?}",
        sent.err()
    );
    server.join().map_err(|_| "server thread panicked")??;
    let seen = seen.lock().map_err(|_| "poisoned")?.clone();
    assert_eq!(
        seen,
        vec![yi_types::event::Wait::Retry {
            attempt: 1,
            of: 3,
            delay_ms: 10,
            cause: "HTTP 429".to_owned(),
        }]
    );
    Ok(())
}

/// Dies with the reason cut off: ureq writes the URL before the error, and an 80-char cut of
/// a long endpoint left the row saying where it failed but never why.
#[test]
fn a_dropped_connection_retry_names_the_reason_not_the_url() -> Res {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = std::thread::spawn(move || -> std::io::Result<()> {
        drop(listener.accept()?);
        let (stream, _) = listener.accept()?;
        let mut reader = BufReader::new(stream);
        let mut length = 0usize;
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header)? == 0 || header.trim().is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                length = value.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0u8; length];
        reader.read_exact(&mut body)?;
        reader
            .into_inner()
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}")?;
        Ok(())
    });
    let seen = std::sync::Mutex::new(Vec::new());
    let sent = send_with_retry(
        &format!(
            "http://127.0.0.1:{port}/api/v1/openai-compatible/chat/completions/with/a/long/deployment/path"
        ),
        &[],
        &transport_probe(),
        None,
        &|wait| seen.lock().map(|mut seen| seen.push(wait)).unwrap_or(()),
    );
    assert!(
        sent.is_ok(),
        "the second attempt succeeds: {:?}",
        sent.err()
    );
    server.join().map_err(|_| "server thread panicked")??;
    let seen = seen.lock().map_err(|_| "poisoned")?.clone();
    let [yi_types::event::Wait::Retry { cause, .. }] = seen.as_slice() else {
        return Err(format!("one retry expected: {seen:?}").into());
    };
    assert!(!cause.contains("127.0.0.1"), "{cause}");
    assert!(!cause.starts_with("http") && !cause.is_empty(), "{cause}");
    Ok(())
}
