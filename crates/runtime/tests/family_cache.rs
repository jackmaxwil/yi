//! A family's requests share one warm prefix (C3): every member sends the root session's id
//! as its affinity key, a reader marks its partition's end when a sibling sent it or a fan-out
//! shares it, and a child's stable prefix is written for five minutes, not the root's hour.
//!
//! | test | tier | claim | mechanism | contrast |
//! |---|---|---|---|---|
//! | `siblings_carry_the_root_id_and_the_second_marks_the_shared_partition` | T1 | Two readers over one partition, spawned by the production wiring on an OpenRouter Claude route, both send `session_id` = the root id; the second one's partition message is the first one's, byte for byte, plus one `cache_control`. | `ProviderStream::for_child` keeps the family key; `Breakpoints` honours `shared_through`. | The key is `None` on every request and `shared_through` is read by no encoder: each sibling may land on another upstream and writes nothing a sibling reads. |
//! | `a_fan_out_marks_the_partition_on_its_first_reader_too` | T1 | Two readers spawned as one fan-out (`readers: 2`, what `rlm.run` sends when gathered calls share a partition) both mark the partition, the first's byte for byte the second's. | `reader::share` gives `Some(0)` to every reader of a fan-out. | Only a repeat got `Some(0)`: nobody wrote the entry, and the second sibling paid the write. |
//! | `a_child_of_an_hour_long_root_writes_five_minute_marks` | T1 | Under a root that holds its stable prefix for an hour, the root's request carries `"ttl":"1h"` and its reader child's carries none. | `for_child` clears the long cache. | The child shares the root's `ProviderStream` and writes its own prompt at the 1h price. |
//! | `a_root_whose_ledger_shows_long_pauses_holds_its_history_an_hour_and_its_child_none` | T1 | Replies whose starts sit half an hour apart, folded by `cache_miss::attach` as in production, make the root's next OpenRouter request carry `"ttl":"1h"` on every `cache_control`, and its worker child's (a loop) on none. | `attach` hands `TtlEstimate` to `ProviderStream::set_ttl_estimate`; the loop asks `StreamFn::cache_ttl`; `for_child` has no estimate (D315). | `attach` never feeds the provider and every pause past five minutes rewrites the prompt; or a child inherits the root's estimate and writes its own prompt at the 1h price. |

use crate::compaction_faux::{faux_model, sse_reply};
use crate::scratch::Scratch;

use std::error::Error;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::{Arc, mpsc};

use serde_json::{Map, Value, json};
use yi_runtime::{AgentSession, ProviderStream, SessionConfig, SubagentHost};
use yi_types::message::AgentMessage;
use yi_types::model::Model;

type TestResult = Result<(), Box<dyn Error>>;

const ROOT_ID: &str = "root-7";

fn key() -> yi_runtime::auth::Resolved {
    yi_runtime::auth::Resolved {
        secret: yi_runtime::auth::Secret::new("sk-test".to_owned()),
        kind: yi_runtime::auth::AuthKind::ApiKey,
        org: None,
        expires: None,
        headers: Vec::new(),
    }
}

pub(crate) fn route(id: &str, api: &str, provider: &str, base_url: &str) -> Model {
    let mut model = faux_model(200_000);
    (model.id, model.api) = (id.to_owned(), api.to_owned());
    (model.provider, model.base_url) = (provider.to_owned(), base_url.to_owned());
    model
}

/// A root session wired the way `yi` wires one, its requests proxied to the stand-in.
pub(crate) fn root(
    scratch: &Scratch,
    model: Model,
    port: u16,
    long_cache: bool,
) -> Result<(AgentSession, Arc<SubagentHost>), Box<dyn Error>> {
    root_with(scratch, model, port, long_cache, None)
}

pub(crate) fn root_with(
    scratch: &Scratch,
    model: Model,
    port: u16,
    long_cache: bool,
    broker: Option<Arc<yi_runtime::PermissionBroker>>,
) -> Result<(AgentSession, Arc<SubagentHost>), Box<dyn Error>> {
    let (cwd, home) = (scratch.join("ws"), scratch.join("home"));
    std::fs::create_dir_all(&cwd)?;
    std::fs::create_dir_all(&home)?;
    std::fs::write(cwd.join("notes.txt"), "red sun\nblue sky\ngreen sea\n")?;
    let provider = Arc::new(
        ProviderStream::new(Some(ROOT_ID.to_owned()))
            .with_auth(&model.provider, key())
            .with_long_cache(long_cache)
            .with_proxy(yi_ai::request::ProxyConfig::from_values(
                Some(&format!("http://127.0.0.1:{port}")),
                None,
                None,
            )?),
    );
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: String::new(),
            model,
            thinking_level: None,
            tool_execution: yi_loop::ExecutionMode::Sequential,
        },
        Arc::clone(&provider),
    );
    let host = yi_runtime::attach_runtime(
        &mut session,
        yi_runtime::RuntimeWiring {
            provider,
            system_prompt: String::new(),
            tool_execution: yi_loop::ExecutionMode::Sequential,
            cwd,
            home: home.clone(),
            lane_slots: 1,
            broker,
            tools: Arc::new(yi_tools::builtin_tools),
            depth: 0,
            max_depth: 1,
            rlm_dir: scratch.join("rlm"),
            family_dir: None,
            summarizer: None,
            advisor: None,
            auto_review: None,
            plan_stale_turns: None,
            plans_dir: Some(scratch.join("plans")),
            parent_link: None,
            wall: yi_runtime::Wall::default(),
            auto_background: None,
            deadline: None,
            kernel_prewarm: false,
            mcp_read: None,
            sessions_dir: None,
            kernels: yi_runtime::fetch::KernelServiceMap::new(),
        },
    );
    Ok((session, host))
}

/// A tool-less reader over `notes.txt`; `readers` is the count `rlm.run` sends for a fan-out.
fn reader(question: &str, readers: Option<u64>) -> (String, Map<String, Value>) {
    reader_over("local://notes.txt", question, readers)
}

fn reader_over(url: &str, question: &str, readers: Option<u64>) -> (String, Map<String, Value>) {
    let mut kwargs = json!({"role": "reader", "tools": [], "partition": [url]});
    if let Some(readers) = readers {
        kwargs["readers"] = json!(readers);
    }
    (
        question.to_owned(),
        kwargs.as_object().cloned().unwrap_or_default(),
    )
}

/// Every request body, sent on as it arrives, each answered with `reply`. A reader's finish
/// wakes the root, whose own requests come here too, so bodies are found by what they ask.
pub(crate) fn stand_in(reply: String) -> std::io::Result<(u16, mpsc::Receiver<Value>)> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let (sender, bodies) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream);
            let mut length = 0usize;
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 && !line.trim().is_empty() {
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().unwrap_or(0);
                }
                line.clear();
            }
            let mut body = vec![0u8; length];
            let _ = reader.read_exact(&mut body);
            let _ = sender.send(serde_json::from_slice(&body).unwrap_or(Value::Null));
            let _ = write!(
                reader.into_inner(),
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                reply.len()
            );
        }
    });
    Ok((port, bodies))
}

/// The first body that asks `question`, waiting up to ten seconds for it.
pub(crate) async fn body_asking(
    bodies: &mut Vec<Value>,
    from: &mpsc::Receiver<Value>,
    question: &str,
) -> Result<Value, Box<dyn Error>> {
    for _ in 0..200 {
        bodies.extend(from.try_iter());
        if let Some(body) = bodies
            .iter()
            .find(|body| body.to_string().contains(question))
        {
            return Ok(body.clone());
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    Err(format!("no request asked {question}: {bodies:?}").into())
}

#[tokio::test(flavor = "multi_thread")]
async fn siblings_carry_the_root_id_and_the_second_marks_the_shared_partition() -> TestResult {
    let scratch = Scratch::new("yi-family-cache")?;
    let (port, from) = stand_in(sse_reply(&json!({"content": "blue sky"}), "stop"))?;
    let model = route(
        "anthropic/claude-haiku-4.5",
        "openai-completions",
        "openrouter",
        "http://openrouter.ai.invalid/api/v1",
    );
    let (_session, host) = root(&scratch, model, port, false)?;
    let (question, kwargs) = reader("Which line names the sky, first?", None);
    host.spawn(question, kwargs)?;
    let (question, kwargs) = reader("Which line names the sky, second?", None);
    host.spawn(question, kwargs)?;
    let mut bodies = Vec::new();
    let first = body_asking(&mut bodies, &from, "sky, first?").await?;
    let second = body_asking(&mut bodies, &from, "sky, second?").await?;
    for body in [&first, &second] {
        assert_eq!(body["session_id"], ROOT_ID, "{body}");
    }
    // [system, partition, question]: the system end is marked on both.
    assert_eq!(first["messages"][0], second["messages"][0]);
    let mut shared = first["messages"][1].clone();
    let last = shared["content"]
        .as_array_mut()
        .and_then(|parts| parts.last_mut())
        .ok_or("the partition message has no parts")?;
    assert!(
        last.get("cache_control").is_none(),
        "a lone reader wrote its partition: {first}"
    );
    last["cache_control"] = json!({"type": "ephemeral"});
    assert_eq!(
        second["messages"][1].to_string(),
        shared.to_string(),
        "the second sibling marks the partition the first one sent"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_child_of_an_hour_long_root_writes_five_minute_marks() -> TestResult {
    let scratch = Scratch::new("yi-family-ttl")?;
    let (port, from) = stand_in(String::new())?;
    let model = route(
        "claude-probe",
        "anthropic-messages",
        "anthropic",
        "http://anthropic.invalid",
    );
    let (session, host) = root(&scratch, model, port, true)?;
    session.prompt("root turn")?;
    let (question, kwargs) = reader("Which line names the sky?", None);
    host.spawn(question, kwargs)?;
    let mut bodies = Vec::new();
    let hour = r#""ttl":"1h""#;
    let root = body_asking(&mut bodies, &from, "root turn").await?;
    assert!(root.to_string().contains(hour), "{root}");
    let child = body_asking(&mut bodies, &from, "names the sky?").await?;
    assert!(!child.to_string().contains(hour), "{child}");
    Ok(())
}

/// Every `cache_control` in a body, in order.
fn cache_controls(value: &Value) -> Vec<Value> {
    match value {
        Value::Object(map) => map
            .iter()
            .flat_map(|(key, value)| {
                if key == "cache_control" {
                    vec![value.clone()]
                } else {
                    cache_controls(value)
                }
            })
            .collect(),
        Value::Array(items) => items.iter().flat_map(cache_controls).collect(),
        _ => Vec::new(),
    }
}

/// A reply as the loop records it: `prompt` tokens, the last 500 written, started `elapsed` ms
/// before it ended, its history sent for five minutes.
fn recorded_reply(
    model: &Model,
    prompt: u64,
    elapsed: u64,
) -> Result<AgentMessage, Box<dyn Error>> {
    Ok(serde_json::from_value(json!({
        "role": "assistant", "content": [], "api": model.api, "provider": model.provider,
        "model": model.id, "stopReason": "stop", "timestamp": 0,
        "diagnostics": [{"type": "cache", "timestamp": 0,
            "details": {"elapsed_ms": elapsed, "ttl": "5m"}}],
        "usage": {"input": 0, "output": 10, "cacheRead": prompt - 500, "cacheWrite": 500,
            "totalTokens": prompt + 10,
            "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0}},
    }))?)
}

/// Through the production wiring (D315): `cache_miss::attach` folds the root's replies, and once
/// they show two half-hour pauses the next OpenRouter request holds every history breakpoint an
/// hour; the root's worker child, a loop whose stream no ledger feeds, stays at five minutes.
#[tokio::test(flavor = "multi_thread")]
async fn a_root_whose_ledger_shows_long_pauses_holds_its_history_an_hour_and_its_child_none()
-> TestResult {
    let scratch = Scratch::new("yi-family-ttl-choice")?;
    // Each reply reads 41k of a 41.5k prompt, so the root's own reply keeps the ledger's
    // estimate pricing an hour when the child's request goes out.
    let usage = json!({"prompt_tokens": 41_500, "completion_tokens": 5, "total_tokens": 41_505,
        "prompt_tokens_details": {"cached_tokens": 41_000}});
    let reply_stream = [
        json!({"choices": [{"index": 0, "delta": {"content": "blue sky"}}]}),
        json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": usage}),
    ]
    .iter()
    .map(|chunk| format!("data: {chunk}\n\n"))
    .collect::<String>()
        + "data: [DONE]\n\n";
    let (port, from) = stand_in(reply_stream)?;
    let mut model = openrouter_claude();
    let price = |value: f64| serde_json::Number::from_f64(value).ok_or("price");
    (model.cost.input, model.cost.cache_write) = (price(1.0)?, price(1.25)?);
    model.cost.cache_read = price(0.1)?;
    let (session, host) = root(&scratch, model.clone(), port, false)?;
    yi_runtime::cache_miss::attach(&session);
    let minute = 60_000;
    for (prompt, elapsed) in [(40_000, 60 * minute), (40_500, 30 * minute), (41_000, 0)] {
        session
            .events_sender()
            .send(yi_types::event::AgentEvent::MessageEnd {
                message: recorded_reply(&model, prompt, elapsed)?,
            })?;
    }
    let mut folded = false;
    for _ in 0..100 {
        folded = yi_loop::run::StreamFn::cache_ttl(session.provider(), &model)
            == yi_types::model::Ttl::Hour1;
        if folded {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(folded, "the ledger's estimate never reached the provider");
    session.prompt("root turn")?;
    let mut bodies = Vec::new();
    let root = cache_controls(&body_asking(&mut bodies, &from, "root turn").await?);
    let hour = json!({"type": "ephemeral", "ttl": "1h"});
    assert!(
        !root.is_empty() && root.iter().all(|mark| *mark == hour),
        "{root:?}"
    );
    // A worker loops (`Reuse::Loop`), so it asks its own stream, which no ledger feeds.
    let kwargs = json!({"role": "root"})
        .as_object()
        .cloned()
        .unwrap_or_default();
    host.spawn("Which line names the sea?".to_owned(), kwargs)?;
    let child = cache_controls(&body_asking(&mut bodies, &from, "names the sea?").await?);
    assert!(!child.is_empty() && !child.contains(&hour), "{child:?}");
    Ok(())
}

/// A fan-out's first reader writes the partition entry its siblings read: both mark it, and
/// the second's partition message is the first's, byte for byte.
#[tokio::test(flavor = "multi_thread")]
async fn a_fan_out_marks_the_partition_on_its_first_reader_too() -> TestResult {
    let scratch = Scratch::new("yi-family-fan-out")?;
    let (port, from) = stand_in(sse_reply(&json!({"content": "blue sky"}), "stop"))?;
    let model = route(
        "anthropic/claude-haiku-4.5",
        "openai-completions",
        "openrouter",
        "http://openrouter.ai.invalid/api/v1",
    );
    let (_session, host) = root(&scratch, model, port, false)?;
    for question in [
        "Which line names the sky, first?",
        "Which line names the sky, second?",
    ] {
        let (question, kwargs) = reader(question, Some(2));
        host.spawn(question, kwargs)?;
    }
    let mut bodies = Vec::new();
    let first = body_asking(&mut bodies, &from, "sky, first?").await?;
    let second = body_asking(&mut bodies, &from, "sky, second?").await?;
    let marked = &first["messages"][1]["content"]
        .as_array()
        .and_then(|parts| parts.last())
        .ok_or("the partition message has no parts")?["cache_control"];
    assert_eq!(*marked, json!({"type": "ephemeral"}), "{first}");
    assert_eq!(
        second["messages"][1].to_string(),
        first["messages"][1].to_string()
    );
    Ok(())
}

/// Every request body with the instant it was read. A request asking `lead` is answered after
/// `LEAD_DELAY`: with `status` and an error body, or on 200 with one text delta, then the rest
/// of the stream `HOLD` later, so its first event and its reply's end are apart.
fn timed_stand_in(
    lead: &'static str,
    status: u16,
) -> std::io::Result<(u16, mpsc::Receiver<(std::time::Instant, Value)>)> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let (sender, bodies) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let sender = sender.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream);
                let (mut length, mut line) = (0usize, String::new());
                while reader.read_line(&mut line).unwrap_or(0) > 0 && !line.trim().is_empty() {
                    if let Some((name, value)) = line.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        length = value.trim().parse().unwrap_or(0);
                    }
                    line.clear();
                }
                let mut body = vec![0u8; length];
                let _ = reader.read_exact(&mut body);
                let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                let leading = body.to_string().contains(lead);
                let _ = sender.send((std::time::Instant::now(), body));
                let mut out = reader.into_inner();
                if leading {
                    std::thread::sleep(LEAD_DELAY);
                }
                if leading && status != 200 {
                    let error = r#"{"error":{"message":"bad request","code":400}}"#;
                    let _ = write!(
                        out,
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{error}",
                        error.len()
                    );
                    return;
                }
                let delta = json!({"choices": [{"index": 0, "delta": {"content": "sky"}}]});
                let _ = write!(
                    out,
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\ndata: {delta}\n\n"
                );
                let _ = out.flush();
                if leading {
                    std::thread::sleep(HOLD);
                }
                let finish = json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}});
                let _ = write!(out, "data: {finish}\n\ndata: [DONE]\n\n");
            });
        }
    });
    Ok((port, bodies))
}

/// The instants the requests asking `first` and `second` were read, waiting up to 30 s.
async fn read_at(
    from: &mpsc::Receiver<(std::time::Instant, Value)>,
    first: &str,
    second: &str,
) -> Result<(std::time::Instant, Value, std::time::Instant, Value), Box<dyn Error>> {
    let mut seen: Vec<(std::time::Instant, Value)> = Vec::new();
    for _ in 0..600 {
        seen.extend(from.try_iter());
        let find = |question: &str| {
            seen.iter()
                .find(|(_, body)| body.to_string().contains(question))
                .cloned()
        };
        if let (Some(a), Some(b)) = (find(first), find(second)) {
            return Ok((a.0, a.1, b.0, b.1));
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    Err(format!("the pair never arrived: {seen:?}").into())
}

const LEAD_DELAY: std::time::Duration = std::time::Duration::from_millis(600);
const HOLD: std::time::Duration = std::time::Duration::from_secs(3);
/// A follower released by the lead's first event sends well before the lead's reply ends.
const RELEASED_BY: std::time::Duration = std::time::Duration::from_millis(2_600);

fn openrouter_claude() -> Model {
    route(
        "anthropic/claude-haiku-4.5",
        "openai-completions",
        "openrouter",
        "http://openrouter.ai.invalid/api/v1",
    )
}

async fn spawn_pair(host: &Arc<SubagentHost>, url: &str, first: &str, second: &str) -> TestResult {
    for question in [first, second] {
        let (question, kwargs) = reader_over(url, question, Some(2));
        host.spawn(question, kwargs)?;
    }
    Ok(())
}

async fn fan_out_pair(
    status: u16,
) -> Result<(std::time::Instant, Value, std::time::Instant, Value), Box<dyn Error>> {
    let scratch = Scratch::new("yi-family-stagger")?;
    let (port, from) = timed_stand_in("first?", status)?;
    let (_session, host) = root(&scratch, openrouter_claude(), port, false)?;
    let (first, second) = ("Which line, sky first?", "Which line, sky second?");
    spawn_pair(&host, "local://notes.txt", first, second).await?;
    read_at(&from, "sky first?", "sky second?").await
}

/// A fan-out's second reader sends once the first one's response has begun, at its first
/// streamed event and not its reply's end, since an entry is read only after the response
/// writing it begins (design §9.4, D314).
#[tokio::test(flavor = "multi_thread")]
async fn a_fan_out_follower_sends_after_the_leads_response_begins() -> TestResult {
    let (lead_at, lead, follow_at, follow) = fan_out_pair(200).await?;
    let gap = follow_at.duration_since(lead_at);
    assert!(
        gap >= LEAD_DELAY && gap < RELEASED_BY,
        "the follower sent {gap:?} after the lead"
    );
    assert_eq!(
        follow["messages"][1].to_string(),
        lead["messages"][1].to_string()
    );
    Ok(())
}

/// A lead that fails before it streams releases its follower at once, well inside the bound.
#[tokio::test(flavor = "multi_thread")]
async fn a_lead_that_fails_before_streaming_releases_its_follower() -> TestResult {
    let (lead_at, _, follow_at, _) = fan_out_pair(400).await?;
    let gap = follow_at.duration_since(lead_at);
    assert!(
        gap >= LEAD_DELAY,
        "the follower sent {gap:?} after the lead"
    );
    assert!(
        gap < std::time::Duration::from_secs(8),
        "the follower waited out the bound: {gap:?}"
    );
    Ok(())
}

/// A second fan-out over the same shape, after the first lead's response began, gets a lead of
/// its own that marks the partition, and a follower that waits for it, not for the old lead.
#[tokio::test(flavor = "multi_thread")]
async fn a_later_fan_out_over_the_same_partition_gets_its_own_lead() -> TestResult {
    let scratch = Scratch::new("yi-family-again")?;
    let (port, from) = timed_stand_in("first?", 200)?;
    let (_session, host) = root(&scratch, openrouter_claude(), port, false)?;
    let url = "local://notes.txt";
    spawn_pair(
        &host,
        url,
        "Which line, sky first?",
        "Which line, sky second?",
    )
    .await?;
    read_at(&from, "sky first?", "sky second?").await?;
    tokio::time::sleep(LEAD_DELAY + HOLD).await;
    spawn_pair(&host, url, "Again, sky first?", "Again, sky second?").await?;
    let (lead_at, lead, follow_at, follow) =
        read_at(&from, "Again, sky first?", "Again, sky second?").await?;
    let gap = follow_at.duration_since(lead_at);
    assert!(
        gap >= LEAD_DELAY && gap < RELEASED_BY,
        "the second fan-out's follower sent {gap:?} after its lead"
    );
    let marked = &lead["messages"][1]["content"][0]["cache_control"];
    assert_eq!(*marked, json!({"type": "ephemeral"}), "{lead}");
    assert_eq!(
        follow["messages"][1].to_string(),
        lead["messages"][1].to_string()
    );
    Ok(())
}

/// Fan-outs over two partitions at once each have a lead: neither waits on the other.
#[tokio::test(flavor = "multi_thread")]
async fn fan_outs_over_two_partitions_do_not_wait_on_each_other() -> TestResult {
    let scratch = Scratch::new("yi-family-two")?;
    let (port, from) = timed_stand_in("first?", 200)?;
    let (_session, host) = root(&scratch, openrouter_claude(), port, false)?;
    std::fs::write(scratch.join("ws/trees.txt"), "oak\nash\n")?;
    let (question, kwargs) = reader_over("local://notes.txt", "Which line, sky first?", Some(2));
    host.spawn(question, kwargs)?;
    let (question, kwargs) = reader_over("local://trees.txt", "Which line, tree first?", Some(2));
    host.spawn(question, kwargs)?;
    let (sky_at, _, tree_at, _) = read_at(&from, "sky first?", "tree first?").await?;
    let apart = tree_at.max(sky_at).duration_since(tree_at.min(sky_at));
    assert!(apart < LEAD_DELAY, "one lead waited {apart:?} on the other");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_openrouter_variant_goes_out_whole_on_the_base_models_route() -> TestResult {
    let scratch = Scratch::new("yi-variant")?;
    let (port, from) = stand_in(sse_reply(&json!({"content": "OK"}), "stop"))?;
    let mut model = yi_runtime::resolve_model("openrouter", "z-ai/glm-5.3-flash:exacto")
        .ok_or("the variant is refused")?;
    model.base_url = "http://openrouter.ai.invalid/api/v1".to_owned();
    let (session, _host) = root(&scratch, model, port, false)?;
    session.prompt("variant probe")?;
    let mut bodies = Vec::new();
    let body = body_asking(&mut bodies, &from, "variant probe").await?;
    assert_eq!(body["model"], "z-ai/glm-5.3-flash:exacto", "{body}");
    Ok(())
}
