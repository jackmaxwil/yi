//! Three live cache signatures, replayed from real session JSONL (cut to the assistant records'
//! token counts, model changes and prompt rewrites, timestamps rebased to 0), each named as its
//! cause; then one synthetic case per cause and tripwire rule the sessions do not cover.

use std::collections::BTreeMap;
use std::error::Error;

use yi_ai::breakpoints::Ttl;
use yi_runtime::cache_miss::MissTracker;
use yi_types::entry::Entry;
use yi_types::message::AgentMessage;
use yi_types::model::Model;

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
    let notice = notice.ok_or("a route that read back less than it wrote raised nothing")?;
    assert!(
        notice.contains("gpt-sol-latest")
            && notice.contains("over its last 10 requests")
            && !notice.contains("has read 0 cached tokens"),
        "{notice}"
    );
    Ok(())
}

/// Dies with the reuse alarm blaming the catalog for idle gaps: twelve single requests each past
/// the five-minute TTL write the whole prompt and read nothing, a cause the fold already names.
#[test]
fn writes_that_expired_between_requests_raise_no_reuse_notice() -> Result<(), Box<dyn Error>> {
    let entries: Vec<Entry> = (0..12u64)
        .map(|n| {
            request(
                CLAUDE,
                (500, 0, 40_000),
                n * 6 * 60 * 1000,
                "Amazon Bedrock",
            )
        })
        .collect::<Result<_, _>>()?;
    let (causes, notices) = fold(&entries);
    assert_eq!(causes, ["gap exceeded TTL"; 11]);
    assert_eq!(notices, Vec::<String>::new());
    Ok(())
}

/// Dies with the reuse alarm blaming the catalog for host hops: twelve requests alternating
/// between two upstreams each write the prompt to a cache the other host never saw.
#[test]
fn writes_lost_to_an_upstream_switch_raise_no_reuse_notice() -> Result<(), Box<dyn Error>> {
    let entries: Vec<Entry> = (0..12u64)
        .map(|n| {
            let host = if n % 2 == 0 {
                "Amazon Bedrock"
            } else {
                "Google Vertex"
            };
            request(CLAUDE, (500, 0, 40_000), n * 1000, host)
        })
        .collect::<Result<_, _>>()?;
    let (causes, notices) = fold(&entries);
    assert_eq!(causes, ["upstream switch"; 11]);
    assert_eq!(notices, Vec::<String>::new());
    Ok(())
}

/// Dies with the reuse alarm firing on a healthy cache: twelve requests that each read the
/// whole previous prompt back and write only the turn's growth.
#[test]
fn a_cache_that_reads_back_what_it_wrote_raises_no_reuse_notice() -> Result<(), Box<dyn Error>> {
    let entries: Vec<Entry> = (0..12u64)
        .map(|n| {
            let (read, write) = (20_000 + n * 3_000, if n == 0 { 20_000 } else { 3_000 });
            request(
                CLAUDE,
                (500, if n == 0 { 0 } else { read }, write),
                n * 1000,
                "Amazon Bedrock",
            )
        })
        .collect::<Result<_, _>>()?;
    let (_, notices) = fold(&entries);
    assert_eq!(notices, Vec::<String>::new());
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

/// Stamps `entry` with the `cache` diagnostic the loop writes: the request took `elapsed` ms and
/// sent its history for `ttl` (a record from before D315 names none).
fn took(mut entry: Entry, elapsed: u64, ttl: Option<Ttl>) -> Result<Entry, Box<dyn Error>> {
    if let Entry::Message {
        message: AgentMessage::Assistant { diagnostics, .. },
        ..
    } = &mut entry
    {
        let mut details = serde_json::json!({"elapsed_ms": elapsed});
        if let Some(ttl) = ttl {
            details["ttl"] = ttl.label().into();
        }
        diagnostics
            .get_or_insert_with(Vec::new)
            .push(serde_json::from_value(
                serde_json::json!({"type": "cache", "timestamp": 0, "details": details}),
            )?);
    }
    Ok(entry)
}

/// A TTL runs from the start of the request that last used the entry (#873): a first reply that
/// streamed for 3 minutes ended 4 minutes before the next one did, and started 6 before it.
#[test]
fn expiry_is_measured_between_request_starts() -> Result<(), Box<dyn Error>> {
    let minute = 60_000;
    let (causes, _) = fold(&[
        took(
            request(CLAUDE, (100, 0, 40_000), 3 * minute, "Amazon Bedrock")?,
            3 * minute,
            None,
        )?,
        took(
            request(CLAUDE, (40_100, 0, 0), 7 * minute, "Amazon Bedrock")?,
            minute,
            None,
        )?,
    ]);
    assert_eq!(causes, ["gap exceeded TTL"]);
    Ok(())
}

/// The fold reads the TTL each request recorded: after an hour-long write a 30-minute gap is
/// no expiry, after a five-minute one (or a record naming none) it is; and a request that first
/// sends an hour and reads nothing is a miss like any other, since a one-hour mark reads a live
/// five-minute entry at its own position (probe F8).
#[test]
fn the_fold_reads_the_ttl_each_request_was_sent_with() -> Result<(), Box<dyn Error>> {
    let minute = 60_000;
    let pair = |first: Option<Ttl>, gap: u64, second: Option<Ttl>| {
        Ok::<_, Box<dyn Error>>(
            fold(&[
                took(
                    request(CLAUDE, (100, 0, 40_000), 0, "Amazon Bedrock")?,
                    0,
                    first,
                )?,
                took(
                    request(CLAUDE, (40_100, 0, 0), gap, "Amazon Bedrock")?,
                    0,
                    second,
                )?,
            ])
            .0,
        )
    };
    let (five, hour) = (Some(Ttl::Min5), Some(Ttl::Hour1));
    assert_eq!(pair(hour, 30 * minute, hour)?, ["previous write not read"]);
    assert_eq!(pair(five, 30 * minute, hour)?, ["gap exceeded TTL"]);
    assert_eq!(pair(None, 30 * minute, None)?, ["gap exceeded TTL"]);
    assert_eq!(pair(five, minute, hour)?, ["previous write not read"]);
    Ok(())
}

/// Claude at `read` times the input price: input 1, a five-minute write 1.25.
fn claude_at(read: f64) -> Result<Model, Box<dyn Error>> {
    let number = |value: f64| serde_json::Number::from_f64(value).ok_or("price");
    let mut model = crate::compaction_faux::faux_model(200_000);
    (model.api, model.provider) = ("openai-completions".to_owned(), CLAUDE.0.to_owned());
    model.id = CLAUDE.1.to_owned();
    model.cost.input = number(1.0)?;
    model.cost.cache_write = number(1.25)?;
    model.cost.cache_read = number(read)?;
    Ok(model)
}

/// Requests `minutes` apart, each adding 500 tokens to a 40k prompt, sent for `ttl`.
fn spaced(minutes: &[u64], ttl: Ttl) -> Result<MissTracker, Box<dyn Error>> {
    let mut tracker = MissTracker::default();
    let mut at = 0;
    for (n, gap) in std::iter::once(&0).chain(minutes).enumerate() {
        at += gap * 60_000;
        let prompt = 40_000 + 500 * u64::try_from(n)?;
        let entry = request(CLAUDE, (0, prompt - 500, 500), at, "Amazon Bedrock")?;
        tracker.observe_entry(&took(entry, 0, Some(ttl))?);
    }
    Ok(tracker)
}

/// The cheapest TTL at Opus 5.5's prices (read 0.05x): a fresh session's second request, whose
/// growth is unknown, and a session that has not paused past five minutes write five minutes; once its own gaps put
/// real weight between five minutes and an hour, an hour; a request that will miss anyway (its
/// last five-minute entry is gone) writes its whole prompt, so it pays five minutes, while one
/// 20 minutes after an hour-long write still reads it and keeps the hour; a route with no hour
/// price never gets one.
#[test]
fn the_cheapest_ttl_prices_the_written_tokens_against_the_expected_rewrite()
-> Result<(), Box<dyn Error>> {
    let opus = claude_at(0.05)?;
    let choose = |tracker: &MissTracker, after_ms: u64, model: &Model| {
        tracker
            .estimate()
            .map(|estimate| estimate.cheapest(model, estimate.last_start + after_ms))
    };
    let (five, hour) = (Ttl::Min5, Ttl::Hour1);
    assert_eq!(choose(&spaced(&[], five)?, 10_000, &opus), Some(five));
    // One short gap: the prior alone would price an hour on a 500-token growth.
    assert_eq!(choose(&spaced(&[1], five)?, 10_000, &opus), Some(five));
    let short = spaced(&[1; 9], five)?;
    assert_eq!(choose(&short, 10_000, &opus), Some(five));
    let paused = spaced(&[30, 30], five)?;
    assert_eq!(choose(&paused, 10_000, &opus), Some(hour));
    assert_eq!(choose(&paused, 6 * 60_000, &opus), Some(five));
    let held = spaced(&[30, 30], hour)?;
    assert_eq!(choose(&held, 20 * 60_000, &opus), Some(hour));
    // A pause after which the prompt shrank: no growth has been seen, so the request is priced
    // as writing its whole prompt, not nothing.
    let mut unseen = MissTracker::default();
    for entry in [
        took(
            request(CLAUDE, (0, 39_500, 500), 0, "Amazon Bedrock")?,
            0,
            Some(five),
        )?,
        took(
            request(CLAUDE, (0, 20_000, 500), 30 * 60_000, "Amazon Bedrock")?,
            0,
            Some(five),
        )?,
    ] {
        unseen.observe_entry(&entry);
    }
    assert_eq!(unseen.estimate().and_then(|estimate| estimate.growth), None);
    assert_eq!(choose(&unseen, 10_000, &opus), Some(five));
    let mut glm = opus.clone();
    glm.id = GLM.1.to_owned();
    assert_eq!(choose(&paused, 10_000, &glm), Some(Ttl::Min5));
    assert_eq!(MissTracker::default().estimate(), None);
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum TtlRule {
    FiveMinutes,
    Hour,
    Cheapest,
}

/// Input-side spend of one TTL rule over one trace of `(start s, prompt)`, at `read` times
/// the input price. Each request marks its tail and reads the previous tail's entry whole while
/// that entry lives, whichever TTL either carries (probe F8); the rest is written at 1.25x (5m)
/// or 2x (1h). The fold sees each request as the loop records it.
fn replay_cost(trace: &[(u64, u32)], rule: TtlRule, read: f64) -> Result<f64, Box<dyn Error>> {
    let model = claude_at(read)?;
    let lives = |ttl| if ttl == Ttl::Hour1 { 3_600 } else { 300 };
    let mut tracker = MissTracker::default();
    let (mut total, mut entry) = (0.0, None::<(u32, u64, Ttl)>);
    for &(start, prompt) in trace {
        let ttl = match rule {
            TtlRule::FiveMinutes => Ttl::Min5,
            TtlRule::Hour => Ttl::Hour1,
            TtlRule::Cheapest => tracker.estimate().map_or(Ttl::Min5, |estimate| {
                estimate.cheapest(&model, start * 1000)
            }),
        };
        let cached = match entry {
            Some((written, at, held))
                if written <= prompt && start.saturating_sub(at) <= lives(held) =>
            {
                written
            }
            _ => 0,
        };
        let write = prompt - cached;
        let premium = if ttl == Ttl::Hour1 { 2.0 } else { 1.25 };
        total += f64::from(cached) * read + f64::from(write) * premium;
        let usage = (0, u64::from(cached), u64::from(write));
        let record = request(CLAUDE, usage, start * 1000, "Amazon Bedrock")?;
        tracker.observe_entry(&took(record, 0, Some(ttl))?);
        entry = Some((prompt, start, ttl));
    }
    Ok(total)
}

/// Each policy's spend summed over the traces.
fn spends(traces: &[Vec<(u64, u32)>], read: f64) -> Result<[f64; 3], Box<dyn Error>> {
    let mut sums = [0.0; 3];
    for trace in traces {
        for (sum, rule) in
            sums.iter_mut()
                .zip([TtlRule::FiveMinutes, TtlRule::Hour, TtlRule::Cheapest])
        {
            *sum += replay_cost(trace, rule, read)?;
        }
    }
    Ok(sums)
}

/// Gate (a): yi's own 45 sessions of 2026-09 (645 requests), reduced by
/// `scripts/cache_gap_trace.py` to request starts and prompt sizes. At read prices 0.1 and
/// 0.05 the cost-minimising choice costs at most 1.01x the better of five minutes and an hour
/// throughout, over all the sessions and again without the one it saves most on, so no single
/// session carries the result.
#[test]
fn the_cheapest_ttl_costs_no_more_than_either_static_ttl_on_yis_sessions()
-> Result<(), Box<dyn Error>> {
    let path = format!(
        "{}/tests/fixtures/cache/yi_gaps.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    let traces = std::fs::read_to_string(path)?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<Vec<Vec<(u64, u32)>>, _>>()?;
    for read in [0.1, 0.05] {
        let per_session = traces
            .iter()
            .map(|trace| spends(std::slice::from_ref(trace), read))
            .collect::<Result<Vec<_>, _>>()?;
        let saving = |[five, hour, cheapest]: [f64; 3]| five.min(hour) - cheapest;
        let best = per_session
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| saving(**a).total_cmp(&saving(**b)))
            .map(|(index, _)| index);
        for without in [None, best] {
            let mut sums = [0.0; 3];
            for (index, session) in per_session.iter().enumerate() {
                if Some(index) != without {
                    for (sum, value) in sums.iter_mut().zip(session) {
                        *sum += value;
                    }
                }
            }
            let [five, hour, cheapest] = sums;
            assert!(
                cheapest <= five.min(hour) * 1.01,
                "read {read}, without {without:?}: cheapest {cheapest:.0}, five minutes {five:.0}, an hour {hour:.0}"
            );
        }
    }
    Ok(())
}

/// A seeded trace of 400 requests: a 20k prompt growing 200-4,000 tokens a request and cut to
/// 30k past 150k, with gaps drawn per `kind`.
fn synthetic(kind: &str) -> Vec<(u64, u32)> {
    let mut state: u64 = 7;
    let mut draw = |low: u64, high: u64| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        low + (state >> 33) % (high - low + 1)
    };
    let (mut at, mut prompt, mut trace) = (0, 20_000u64, Vec::new());
    for n in 0..400u64 {
        trace.push((at, u32::try_from(prompt).unwrap_or(u32::MAX)));
        prompt += draw(200, 4_000);
        if prompt > 150_000 {
            prompt = 30_000;
        }
        at += match kind {
            "short" => draw(2, 120),
            "long" => draw(600, 3_000),
            "bursty" if n % 20 == 0 => 7_200,
            "bursty" => draw(2, 30),
            _ if draw(0, 5) > 0 => draw(2, 40),
            _ => match draw(0, 9) {
                0..=5 => draw(30, 280),
                6..=8 => draw(300, 3_500),
                _ => draw(3_700, 20_000),
            },
        };
    }
    trace
}

/// Gate (b): on seeded traces whose gaps are all short, all 10-50 minutes, mixed (tool bursts
/// and think time with real mass past five minutes) or bursty (bursts then two-hour idles), at
/// read prices 0.1 and 0.05, the cost-minimising choice lands within 1.05x of the trace's better
/// static TTL and below its worse one.
#[test]
fn the_cheapest_ttl_tracks_the_better_static_ttl_on_known_gap_mixes() -> Result<(), Box<dyn Error>>
{
    for kind in ["short", "long", "mixed", "bursty"] {
        let trace = synthetic(kind);
        for read in [0.1, 0.05] {
            let [five, hour, cheapest] = spends(std::slice::from_ref(&trace), read)?;
            let why = format!(
                "{kind} at read {read}: {cheapest:.0} against {five:.0} (5m) and {hour:.0} (1h)"
            );
            assert!(cheapest <= five.min(hour) * 1.05, "{why}");
            assert!(cheapest < five.max(hour), "{why}");
        }
    }
    Ok(())
}
