use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;

use serde_json::json;
use yi_ai::request::{ProxyConfig, send_with_retry};

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
        &json!({"model": "probe"}),
        Some(&config),
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
