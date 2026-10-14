//! A turn cut at the reasoning budget (D163) drops the stream before its usage chunk, so
//! its cost would be a guess. OpenRouter bills a dropped stream for what was generated and
//! serves the record at `/generation?id=` within seconds, so the cut turn is settled from it.

use serde_json::Value;
use yi_types::message::Usage;
use yi_types::model::Model;

/// Where a settled usage and its upstream go once the turn they belong to has already ended.
#[derive(Clone)]
pub struct Settle(pub std::sync::Arc<dyn Fn(Usage, Option<String>) + Send + Sync>);

impl std::fmt::Debug for Settle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Settle")
    }
}

static PENDING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The schedule's 28 s of waiting plus the fetches' own time.
pub const DRAIN_CAP: std::time::Duration = std::time::Duration::from_secs(32);

struct Pending;

impl Drop for Pending {
    fn drop(&mut self) {
        PENDING.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

/// Invariant: the record lands seconds after the turn, so its wait runs on a thread of its own
/// that nothing joins; a one-shot process calls [`drain`] before it exits.
pub fn detach(job: impl FnOnce() + Send + 'static) {
    PENDING.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    let guard = Pending;
    let _detached = std::thread::Builder::new()
        .name("yi-settle".to_owned())
        .spawn(move || {
            let _guard = guard;
            job();
        });
}

/// Waits up to `cap` for the detached settlements of this process; what is still running then
/// is lost with the process, which costs only that turn's settled usage.
pub fn drain(cap: std::time::Duration) {
    let began = std::time::Instant::now();
    while PENDING.load(std::sync::atomic::Ordering::Acquire) > 0 && began.elapsed() < cap {
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

/// Settlement only exists where the record does; every other provider keeps the estimate.
pub fn settles(model: &Model) -> bool {
    model.base_url.contains("openrouter.ai")
}

/// The generation record as a measured usage and its upstream, or None without token counts.
pub fn usage_from_generation(data: &Value) -> Option<(Usage, Option<String>)> {
    let count = |key: &str| data.get(key).and_then(Value::as_i64);
    let input = count("tokens_prompt")?;
    let output = count("tokens_completion")?;
    let mut usage = Usage::zero();
    usage.input = input;
    usage.output = output;
    usage.cache_read = count("native_tokens_cached").unwrap_or(0);
    usage.reasoning = count("native_tokens_reasoning");
    usage.total_tokens = input.saturating_add(output);
    let total = data
        .get("total_cost")
        .and_then(Value::as_f64)
        .and_then(serde_json::Number::from_f64)?;
    usage.cost.total = total;
    usage.unknown = false;
    let upstream = data.get("provider_name").and_then(Value::as_str);
    Some((usage, upstream.map(str::to_owned)))
}

/// The record appears about ten seconds after the drop (a 404 at 2, 5 and 8 s, present at
/// 10 s, measured 2026-09-09): tried at 3, 8, 16 and 28 s, so a slow upstream still lands.
pub fn generation_usage(
    model: &Model,
    api_key: &str,
    proxy: Option<&crate::request::ProxyConfig>,
    id: &str,
) -> Option<(Usage, Option<String>)> {
    if !settles(model) || id.is_empty() {
        return None;
    }
    let base = model.base_url.trim_end_matches('/');
    let url = format!("{base}/generation?id={id}");
    let headers = [("authorization", format!("Bearer {api_key}"))];
    for wait_ms in [3_000_u64, 5_000, 8_000, 12_000] {
        std::thread::sleep(std::time::Duration::from_millis(wait_ms));
        if let Ok(body) = crate::request::get_json(&url, &headers, proxy, 64 * 1024)
            && let Some(settled) = body.get("data").and_then(usage_from_generation)
        {
            return Some(settled);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generation_record_becomes_a_measured_usage() {
        let data = serde_json::json!({
            "tokens_prompt": 23, "tokens_completion": 3089, "native_tokens_reasoning": 3089,
            "total_cost": 0.000773975, "finish_reason": null, "provider_name": "Z.AI"
        });
        let (usage, upstream) = usage_from_generation(&data).unwrap();
        assert_eq!(upstream.as_deref(), Some("Z.AI"));
        assert_eq!(
            (usage.input, usage.output, usage.reasoning),
            (23, 3089, Some(3089))
        );
        assert!(!usage.unknown);
        assert_eq!(usage.cost.total.as_f64(), Some(0.000773975));
        assert!(usage_from_generation(&serde_json::json!({"id": "gen-1"})).is_none());
    }
}
