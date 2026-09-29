//! A family's requests share one warm prefix (C3): every member sends the root session's id
//! as its affinity key, a reader marks its partition's end when a sibling sent it or a fan-out
//! shares it, and a child's stable prefix is written for five minutes, not the root's hour.
//!
//! | test | tier | claim | mechanism | contrast |
//! |---|---|---|---|---|
//! | `siblings_carry_the_root_id_and_the_second_marks_the_shared_partition` | T1 | Two readers over one partition, spawned by the production wiring on an OpenRouter Claude route, both send `session_id` = the root id; the second one's partition message is the first one's, byte for byte, plus one `cache_control`. | `ProviderStream::for_child` keeps the family key; `Breakpoints` honours `shared_through`. | The key is `None` on every request and `shared_through` is read by no encoder: each sibling may land on another upstream and writes nothing a sibling reads. |
//! | `a_fan_out_marks_the_partition_on_its_first_reader_too` | T1 | Two readers spawned as one fan-out (`readers: 2`, what `rlm.run` sends when gathered calls share a partition) both mark the partition, the first's byte for byte the second's. | `reader::share` gives `Some(0)` to every reader of a fan-out. | Only a repeat got `Some(0)`: nobody wrote the entry, and the second sibling paid the write. |
//! | `a_child_of_an_hour_long_root_writes_five_minute_marks` | T1 | Under a root that holds its stable prefix for an hour, the root's request carries `"ttl":"1h"` and its reader child's carries none. | `for_child` clears the long cache. | The child shares the root's `ProviderStream` and writes its own prompt at the 1h price. |

use crate::compaction_faux::{faux_model, sse_reply};
use crate::scratch::Scratch;

use std::error::Error;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::{Arc, mpsc};

use serde_json::{Map, Value, json};
use yi_runtime::{AgentSession, ProviderStream, SessionConfig, SubagentHost};
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

fn route(id: &str, api: &str, provider: &str, base_url: &str) -> Model {
    let mut model = faux_model(200_000);
    (model.id, model.api) = (id.to_owned(), api.to_owned());
    (model.provider, model.base_url) = (provider.to_owned(), base_url.to_owned());
    model
}

/// A root session wired the way `yi` wires one, its requests proxied to the stand-in.
fn root(
    scratch: &Scratch,
    model: Model,
    port: u16,
    long_cache: bool,
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
            broker: None,
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
    let mut kwargs = json!({"role": "reader", "tools": [], "partition": ["local://notes.txt"]});
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
fn stand_in(reply: String) -> std::io::Result<(u16, mpsc::Receiver<Value>)> {
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
async fn body_asking(
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

/// Every request body with the instant it was read. The request that asks `lead` is answered
/// after `delay` with `status` (a stream on 200, an error body otherwise); the rest at once.
fn timed_stand_in(
    lead: &'static str,
    delay: std::time::Duration,
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
                let (status, kind, reply) = if leading && status != 200 {
                    std::thread::sleep(delay);
                    let error = r#"{"error":{"message":"bad request","code":400}}"#.to_owned();
                    (status, "application/json", error)
                } else {
                    if leading {
                        std::thread::sleep(delay);
                    }
                    (
                        200,
                        "text/event-stream",
                        sse_reply(&json!({"content": "sky"}), "stop"),
                    )
                };
                let _ = write!(
                    reader.into_inner(),
                    "HTTP/1.1 {status} X\r\ncontent-type: {kind}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                    reply.len()
                );
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

async fn fan_out_pair(
    status: u16,
) -> Result<(std::time::Instant, Value, std::time::Instant, Value), Box<dyn Error>> {
    let scratch = Scratch::new("yi-family-stagger")?;
    let (port, from) = timed_stand_in("sky, first?", LEAD_DELAY, status)?;
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
    read_at(&from, "sky, first?", "sky, second?").await
}

/// A fan-out's second reader sends only once the first one's response has begun, since an
/// entry is read only after the response writing it begins (design §9.4, D314).
#[tokio::test(flavor = "multi_thread")]
async fn a_fan_out_follower_sends_after_the_leads_response_begins() -> TestResult {
    let (lead_at, lead, follow_at, follow) = fan_out_pair(200).await?;
    let gap = follow_at.duration_since(lead_at);
    assert!(
        gap >= LEAD_DELAY,
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
