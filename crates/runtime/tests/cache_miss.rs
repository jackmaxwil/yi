//! Three live cache signatures, replayed from real session JSONL (trimmed to the assistant
//! records, model changes and prompt rewrites; usage bytes verbatim), each named as its cause.

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
        if let Some(cause) = tracker.entry(&entry) {
            *causes.entry(cause.label()).or_default() += 1;
        }
        notice = notice.or_else(|| tracker.take_notice());
    }
    Ok((causes, notice))
}

/// The unflagged Opus dogfood session (2026-09-27): OpenRouter got no mark, so no request
/// wrote or read; the orchestrate protocol rewrote the system prompt once in the middle.
#[test]
fn an_unmarked_claude_session_names_no_marks_and_raises_the_notice() -> Result<(), Box<dyn Error>> {
    let (causes, notice) = replay("opus_no_marks")?;
    assert_eq!(
        causes,
        BTreeMap::from([("no marks", 37), ("stable key changed", 1)])
    );
    let notice = notice.ok_or("no notice after two total misses")?;
    assert!(
        notice.contains("anthropic/claude-opus-5.5") && notice.contains("no marks"),
        "{notice}"
    );
    Ok(())
}

/// GPT-sol with the environment tail last: the automatic breakpoint wrote on the tail every
/// request, and each next request read only the system (20,872 tokens) back.
#[test]
fn a_write_no_request_reads_back_names_the_automatic_mark_on_the_tail() -> Result<(), Box<dyn Error>>
{
    let (causes, notice) = replay("gpt_sol_tail_write")?;
    assert_eq!(
        causes,
        BTreeMap::from([
            ("automatic mark on tail", 21),
            ("model switch", 1),
            ("unexplained", 1),
        ])
    );
    assert_eq!(notice, None, "a partial read is not a total miss");
    Ok(())
}

/// GLM: the orchestrate attach rewrote the system prompt after request 18, and request 19
/// read nothing of the 41k it had read before. An implicit cache is not write-billed: no notice.
#[test]
fn a_late_system_rewrite_names_the_stable_key() -> Result<(), Box<dyn Error>> {
    let (causes, notice) = replay("glm_orchestrate_attach")?;
    assert_eq!(causes, BTreeMap::from([("stable key changed", 1)]));
    assert_eq!(notice, None);
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
