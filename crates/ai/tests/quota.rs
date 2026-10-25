//! A quota or balance refusal ends the request at once; a throttle still waits and retries.
//! Bodies: the Moonshot wording Kimi Code anchors on (kosong/src/providers/kimi-errors.ts:8-15)
//! and OpenAI's documented `insufficient_quota` error.

use std::error::Error;
use std::io::{BufReader, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use yi_ai::kimi::usage_line;
use yi_ai::retry::quota_exhausted;

use crate::loopback::read_request;

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
            let _ = read_request(&mut reader);
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

/// The degenerate pair: a gateway that flattens Moonshot's body keeps only the billing words, and
/// a per-minute throttle says "quota" without meaning the balance. Gemini's throttle text is
/// Google's published RESOURCE_EXHAUSTED message, typed, not recorded.
#[test]
fn billing_wording_without_a_code_is_final_on_moonshot_and_a_throttle_is_not() {
    const KIMI: &str = "https://api.kimi.ai/coding/v1/chat/completions";
    const GEMINI: &str = "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions";
    const OPENROUTER: &str = "https://openrouter.ai/api/v1/chat/completions";
    let suspended = "Your account org-x is suspended due to insufficient balance, please recharge your account or check your plan and billing details";
    assert!(quota_exhausted(429, suspended, KIMI));
    assert!(quota_exhausted(
        429,
        "You exceeded your current token quota: ...",
        KIMI
    ));
    assert!(!quota_exhausted(
        429,
        "Rate limit reached: token quota per minute exceeded",
        KIMI
    ));
    assert!(!quota_exhausted(429, THROTTLE, KIMI));
    let gemini = r#"{"error":{"code":429,"message":"You exceeded your current quota, please check your plan and billing details. Please retry in 7.06s.","status":"RESOURCE_EXHAUSTED"}}"#;
    assert!(
        !quota_exhausted(429, gemini, GEMINI),
        "gemini RPM throttle final"
    );
    let relayed = json!({"error": {"code": 429, "message": "Provider returned error",
        "metadata": {"raw": gemini, "provider_name": "Google AI Studio"}}});
    assert!(
        !quota_exhausted(429, &relayed.to_string(), OPENROUTER),
        "relayed throttle final"
    );
    assert!(
        quota_exhausted(429, OPENAI_QUOTA, GEMINI),
        "a quota code is final on any route"
    );
    assert!(
        !quota_exhausted(500, MOONSHOT_QUOTA, KIMI),
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
