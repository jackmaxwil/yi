//! A quota or balance refusal ends the request at once; a throttle still waits and retries.
//! Bodies: the Moonshot wording Kimi Code anchors on (kosong/src/providers/kimi-errors.ts:8-15)
//! and OpenAI's documented `insufficient_quota` error.

use std::error::Error;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use yi_ai::kimi::usage_line;
use yi_ai::retry::quota_exhausted;

type TestResult = Result<(), Box<dyn Error>>;

const MOONSHOT_QUOTA: &str = r#"{"error":{"message":"You exceeded your current token quota: please check your account balance","type":"exceeded_current_quota_error"}}"#;
const OPENAI_QUOTA: &str = r#"{"error":{"message":"You exceeded your current quota, please check your plan and billing details. For more information on this error, read the docs: https://platform.openai.com/docs/guides/error-codes/api-errors.","type":"insufficient_quota","param":null,"code":"insufficient_quota"}}"#;
const THROTTLE: &str =
    r#"{"error":{"message":"Rate limit reached for requests","type":"rate_limit_reached_error"}}"#;

/// Answers every request with a 429 carrying `body`; the count is every request that arrived.
fn refusing(body: &'static str) -> std::io::Result<(u16, Arc<AtomicUsize>)> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let seen = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream);
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let _ = reader.read_exact(&mut vec![0; length]);
            counted.fetch_add(1, Ordering::SeqCst);
            let reply = format!(
                "HTTP/1.1 429 Too Many Requests\r\ncontent-type: application/json\r\nretry-after: 0\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = reader.get_mut().write_all(reply.as_bytes());
        }
    });
    Ok((port, seen))
}

fn post(port: u16) -> Result<String, Box<dyn Error>> {
    let probe =
        yi_ai::breakpoints::Encoded::provider_prefix(json!({"model": "p"}), "messages", Vec::new());
    match yi_ai::request::send_with_retry(
        &format!("http://127.0.0.1:{port}/chat/completions"),
        &[],
        &probe,
        None,
        &|_| {},
    ) {
        Ok(_) => Err("a 429 was taken as a success".into()),
        Err(error) => Ok(error),
    }
}

/// A paid run on an exhausted subscription must stop and switch family, not spend the 300 s
/// retry wall on a refusal that lifts at the next window.
#[test]
fn a_quota_429_is_final_and_a_rate_limit_429_still_retries() -> TestResult {
    for body in [MOONSHOT_QUOTA, OPENAI_QUOTA] {
        let (port, seen) = refusing(body)?;
        let error = post(port)?;
        assert_eq!(seen.load(Ordering::SeqCst), 1, "{body}");
        assert_eq!(error, format!("HTTP 429 quota exhausted: {body}"));
    }
    let (port, seen) = refusing(THROTTLE)?;
    let error = post(port)?;
    assert!(
        seen.load(Ordering::SeqCst) >= 2,
        "a throttle was not retried"
    );
    assert_eq!(error, format!("HTTP 429: {THROTTLE}"));
    Ok(())
}

/// The degenerate pair: a gateway that flattens the body keeps only the billing words, and a
/// per-minute throttle says "quota" without meaning the balance.
#[test]
fn billing_wording_without_a_code_is_final_and_token_quota_per_minute_is_not() {
    let suspended = "Your account org-x is suspended due to insufficient balance, please recharge your account or check your plan and billing details";
    assert!(quota_exhausted(429, suspended));
    assert!(quota_exhausted(
        429,
        "You exceeded your current token quota: ..."
    ));
    assert!(!quota_exhausted(
        429,
        "Rate limit reached: token quota per minute exceeded"
    ));
    assert!(!quota_exhausted(429, THROTTLE));
    assert!(
        !quota_exhausted(500, MOONSHOT_QUOTA),
        "only a 429 is a quota refusal"
    );
}

/// Kimi Code's reference payload (packages/oauth/test/managed-usage.test.ts:97-103), a window
/// set with only the 5-hour limit, and the account's own reply recorded on 2026-10-24.
#[test]
fn usages_parse_every_window_and_omit_the_absent() -> TestResult {
    let full = json!({"goods_version": 2, "usages": {
        "limit_5h": {"used_ratio": 0.3, "reset_time": "2026-09-11T18:00:00Z"},
        "limit_7d": {"used_ratio": 0.2, "reset_time": "2026-09-17T00:00:00Z"},
        "limit_month_total": {"used_ratio": 0.4, "reset_time": "2026-10-01T00:00:00Z"},
        "limit_month_code": {"used_ratio": 0.25, "reset_time": "2026-10-01T00:00:00Z"},
    }});
    assert_eq!(
        usage_line(&full),
        json!({
            "5h": {"used": 0.3, "reset": "2026-09-11T18:00:00Z"},
            "7d": {"used": 0.2, "reset": "2026-09-17T00:00:00Z"},
            "month_total": {"used": 0.4, "reset": "2026-10-01T00:00:00Z"},
            "month_code": {"used": 0.25, "reset": "2026-10-01T00:00:00Z"},
        })
    );
    let five =
        json!({"usages": {"limit_5h": {"used_ratio": "0.5"}, "limit_7d": {"used_ratio": "n/a"}}});
    assert_eq!(usage_line(&five), json!({"5h": {"used": 0.5}}));
    let live: Value = serde_json::from_str(include_str!("fixtures/kimi-usages-2026-10-24.json"))?;
    assert_eq!(
        usage_line(&live),
        json!({
            "5h": {"used": 0.0, "reset": "2026-10-24T19:28:25Z"},
            "7d": {"used": 0.0, "reset": "2026-10-30T06:28:25Z"},
        })
    );
    Ok(())
}
