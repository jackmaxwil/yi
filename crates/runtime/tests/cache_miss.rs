//! Three live cache signatures, replayed from real session JSONL (cut to the assistant records'
//! token counts, model changes and prompt rewrites, timestamps rebased to 0), each named as its
//! cause; then one synthetic case per cause and tripwire rule the sessions do not cover.

use std::collections::BTreeMap;
use std::error::Error;

use yi_runtime::cache_miss::MissTracker;
use yi_types::entry::Entry;
use yi_types::message::AgentMessage;

type Replay = (BTreeMap<&'static str, usize>, Option<String>);

fn replay(name: &str) -> Result<Replay, Box<dyn Error>> {
    replay_keyed(name, |_| None)
}

/// `key(n)` stamps request `n`'s `cache` diagnostic, as a build after D310 records it.
fn replay_keyed(
    name: &str,
    key: impl Fn(usize) -> Option<&'static str>,
) -> Result<Replay, Box<dyn Error>> {
    let path = format!(
        "{}/tests/fixtures/cache/{name}.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut tracker = MissTracker::default();
    let mut causes = BTreeMap::new();
    let mut notice = None;
    let mut requests = 0;
    for line in std::fs::read_to_string(path)?.lines() {
        let mut entry: Entry = serde_json::from_str(line)?;
        if let Entry::Message {
            message: AgentMessage::Assistant { diagnostics, .. },
            ..
        } = &mut entry
        {
            requests += 1;
            if let Some(stable) = key(requests) {
                diagnostics.get_or_insert_with(Vec::new).push(serde_json::from_value(
                    serde_json::json!({"type": "cache", "timestamp": 0, "details": {"stable": stable}}),
                )?);
            }
        }
        if let Some(cause) = tracker.observe_entry(&entry) {
            *causes.entry(cause.label()).or_default() += 1;
        }
        notice = notice.or_else(|| tracker.take_notice());
    }
    Ok((causes, notice))
}

/// The unflagged Opus dogfood session (2026-09-27): OpenRouter got no mark, so no request
/// wrote or read; the orchestrate protocol rewrote the system prompt once in the middle.
#[test]
fn an_unmarked_claude_session_names_nothing_cached_and_raises_the_notice()
-> Result<(), Box<dyn Error>> {
    let (causes, notice) = replay("opus_no_marks")?;
    assert_eq!(
        causes,
        BTreeMap::from([("nothing written or read", 37), ("stable key changed", 1)])
    );
    let notice = notice.ok_or("no notice after two total misses")?;
    assert!(
        notice.contains("anthropic/claude-opus-5.5") && notice.contains("nothing written or read"),
        "{notice}"
    );
    Ok(())
}

/// GPT-sol with the environment tail last: the automatic breakpoint wrote on the tail every
/// request, and each next request read only the system (20,872 tokens) back.
#[test]
fn a_write_no_request_reads_back_names_the_previous_write() -> Result<(), Box<dyn Error>> {
    let (causes, notice) = replay("gpt_sol_tail_write")?;
    assert_eq!(
        causes,
        BTreeMap::from([
            ("previous write not read", 21),
            ("model switch", 1),
            ("unexplained", 1),
        ])
    );
    assert_eq!(notice, None, "a partial read is not a total miss");
    Ok(())
}

/// GLM: the orchestrate attach rewrote the system prompt after request 18, and request 19
/// read nothing of the 41k it had read before.
#[test]
fn a_late_system_rewrite_names_the_stable_key() -> Result<(), Box<dyn Error>> {
    let (causes, _) = replay("glm_orchestrate_attach")?;
    assert_eq!(causes, BTreeMap::from([("stable key changed", 1)]));
    Ok(())
}

/// After D310 the record carries the key: a prompt-state write that left the system alone is no
/// cause, and a key that moved is one.
#[test]
fn a_recorded_key_decides_over_the_prompt_state_write() -> Result<(), Box<dyn Error>> {
    let (causes, _) = replay_keyed("glm_orchestrate_attach", |_| Some("0a"))?;
    assert_eq!(causes, BTreeMap::from([("unexplained", 1)]));
    let (causes, _) = replay_keyed("glm_orchestrate_attach", |n| {
        Some(if n < 6 { "0a" } else { "0b" })
    })?;
    assert_eq!(causes, BTreeMap::from([("stable key changed", 1)]));
    Ok(())
}

const CLAUDE: (&str, &str) = ("openrouter", "anthropic/claude-opus-5.5");
const GLM: (&str, &str) = ("openrouter", "z-ai/glm-5.3-flash");

/// One assistant record: `input` uncached, `read` and `write` cached, at `at` ms.
fn request(
    route: (&str, &str),
    (input, read, write): (u64, u64, u64),
    at: u64,
    upstream: &str,
) -> Result<Entry, Box<dyn Error>> {
    Ok(serde_json::from_value(serde_json::json!({
        "type": "message", "id": "e", "parentId": null, "seq": 0, "timestamp": at,
        "message": {"role": "assistant", "content": [], "api": "openai-completions",
            "provider": route.0, "model": route.1, "stopReason": "stop", "timestamp": 0,
            "diagnostics": [{"type": "upstream", "timestamp": 0, "details": {"provider": upstream}}],
            "usage": {"input": input, "output": 10, "cacheRead": read, "cacheWrite": write,
                "totalTokens": input + read + write + 10,
                "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0}}},
    }))?)
}

/// Every cause and every notice the fold gives, in order.
fn fold(entries: &[Entry]) -> (Vec<&'static str>, Vec<String>) {
    let mut tracker = MissTracker::default();
    let (mut causes, mut notices) = (Vec::new(), Vec::new());
    for entry in entries {
        causes.extend(tracker.observe_entry(entry).map(|cause| cause.label()));
        notices.extend(tracker.take_notice());
    }
    (causes, notices)
}

#[test]
fn a_gap_past_five_minutes_is_an_expiry_and_a_new_upstream_a_switch() -> Result<(), Box<dyn Error>>
{
    let minute = 60_000;
    let (causes, _) = fold(&[
        request(CLAUDE, (100, 40_000, 0), 0, "Amazon Bedrock")?,
        request(CLAUDE, (40_100, 0, 0), 6 * minute, "Amazon Bedrock")?,
        request(CLAUDE, (100, 40_000, 0), 7 * minute, "Amazon Bedrock")?,
        request(CLAUDE, (40_100, 0, 0), 8 * minute, "Google Vertex")?,
    ]);
    assert_eq!(causes, ["gap exceeded TTL", "upstream switch"]);
    Ok(())
}

/// Two total misses in a row raise one notice for the session, on a write-billed route only.
#[test]
fn the_notice_fires_once_on_a_billed_route_and_never_on_an_implicit_cache()
-> Result<(), Box<dyn Error>> {
    let misses = |route| -> Result<Vec<Entry>, Box<dyn Error>> {
        (0..5)
            .map(|n| request(route, (40_000, 0, 0), n * 1000, "Amazon Bedrock"))
            .collect()
    };
    let (causes, notices) = fold(&misses(CLAUDE)?);
    assert_eq!(causes.len(), 4);
    assert_eq!(notices.len(), 1, "{notices:?}");
    let (causes, notices) = fold(&misses(GLM)?);
    assert_eq!(causes, ["unexplained"; 4]);
    assert_eq!(notices, Vec::<String>::new());
    Ok(())
}

/// A prompt below the minimum a Claude model caches never reads, so it raises nothing.
#[test]
fn a_prompt_below_the_cacheable_minimum_raises_no_notice() -> Result<(), Box<dyn Error>> {
    let entries: Vec<Entry> = (0..4)
        .map(|n| request(CLAUDE, (3_000, 0, 0), n * 1000, "Amazon Bedrock"))
        .collect::<Result<_, _>>()?;
    let (causes, notices) = fold(&entries);
    assert_eq!(causes.len(), 3);
    assert_eq!(notices, Vec::<String>::new());
    Ok(())
}

/// A compaction rewrote the history: reading only the system after it is no miss.
#[test]
fn a_compaction_owes_no_read() -> Result<(), Box<dyn Error>> {
    let compaction: Entry = serde_json::from_value(serde_json::json!({
        "type": "compaction", "id": "c", "parentId": null, "seq": 0, "timestamp": 1000,
        "summary": "", "retainedTail": [], "tokensBefore": 150_000,
    }))?;
    let (causes, _) = fold(&[
        request(CLAUDE, (1_000, 149_000, 0), 0, "Amazon Bedrock")?,
        compaction,
        request(CLAUDE, (5_000, 20_000, 5_000), 2000, "Amazon Bedrock")?,
    ]);
    assert_eq!(causes, Vec::<&str>::new());
    Ok(())
}
