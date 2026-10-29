//! What a provider's wire does to a turn: the connection a finished stream hands back, the bytes
//! that end it, and the turn that survives a cut. Every server here is a loopback listener that
//! writes raw bytes, because ureq's pool and its decoder are only visible from the far side.

use std::error::Error;
use std::io::{BufReader, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use serde_json::json;
use yi_ai::openai::OpenAiOptions;
use yi_ai::openai_responses::EventMapper;
use yi_types::event::AssistantMessageEvent;
use yi_types::message::{AgentMessage, Content, StopReason};
use yi_types::model::{LlmContext, Model};

use crate::loopback::read_request;

type TestResult = Result<(), Box<dyn Error>>;

fn model(api: &str, provider: &str, port: u16) -> Result<Model, Box<dyn Error>> {
    Ok(serde_json::from_value(json!({
        "id": "probe",
        "name": "probe",
        "api": api,
        "provider": provider,
        "baseUrl": format!("http://127.0.0.1:{port}"),
        "reasoning": false,
        "input": ["text"],
        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
        "contextWindow": 1000,
        "maxTokens": 64
    }))?)
}

fn context() -> LlmContext {
    LlmContext {
        cache_ttl: yi_types::model::Ttl::Min5,
        system_prompt: String::new(),
        messages: vec![AgentMessage::user_input(
            yi_types::message::UserContent::Text("hi".to_owned()),
            0,
        )],
        transient: Vec::new(),
        schema: None,
        shared_through: None,
        reuse: yi_types::model::Reuse::Loop,
        tools: None,
        tool_choice: None,
    }
}

/// One HTTP/1.1 response whose body is `body`, framed so ureq hands the socket back to its pool the
/// moment the body is exhausted: what a stream that stops early must still do.
fn framed(body: &str) -> Vec<u8> {
    let head = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n",
        body.len()
    );
    format!("{head}{body}").into_bytes()
}

fn keep_alive(reply: Vec<u8>) -> std::io::Result<(u16, Count, std::thread::JoinHandle<()>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let counts = Count::default();
    let returned = counts.clone();
    let handle = std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            counts.accepts.fetch_add(1, Ordering::SeqCst);
            let reply = reply.clone();
            let counts = counts.clone();
            std::thread::spawn(move || {
                let Ok(cloned) = stream.try_clone() else {
                    return;
                };
                let mut reader = BufReader::new(cloned);
                let mut writer = stream;
                loop {
                    if read_request(&mut reader).is_empty() {
                        return;
                    }
                    counts.requests.fetch_add(1, Ordering::SeqCst);
                    if writer.write_all(&reply).is_err() || writer.flush().is_err() {
                        return;
                    }
                }
            });
        }
    });
    Ok((port, returned, handle))
}

/// What a server saw: TCP connections opened, and requests answered. A pool question is about the
/// first, a retry question about the second.
#[derive(Clone, Default)]
struct Count {
    accepts: Arc<AtomicUsize>,
    requests: Arc<AtomicUsize>,
}

impl Count {
    fn accepts(&self) -> usize {
        self.accepts.load(Ordering::SeqCst)
    }

    fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

/// Answers one request with `bytes` and closes: a body that ends before its framing said it would.
fn once(bytes: Vec<u8>) -> std::io::Result<(u16, std::thread::JoinHandle<()>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let handle = std::thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        let Ok(cloned) = stream.try_clone() else {
            return;
        };
        let mut reader = BufReader::new(cloned);
        let _ = read_request(&mut reader);
        let mut writer = stream;
        let _ = writer.write_all(&bytes);
        let _ = writer.flush();
    });
    Ok((port, handle))
}

/// The two facts a driver reads off a turn: what it ended as, and the text it carried.
#[derive(Debug)]
struct TestEvent {
    error: Option<String>,
    stop: Option<StopReason>,
    text: String,
}

impl TestEvent {
    fn of(event: AssistantMessageEvent) -> Self {
        let message = match event {
            AssistantMessageEvent::Done { message, .. }
            | AssistantMessageEvent::Error { error: message, .. } => Some(message),
            _ => None,
        };
        let (error, stop, text) = match message {
            Some(AgentMessage::Assistant {
                content,
                stop_reason,
                error_message,
                ..
            }) => (
                error_message,
                Some(stop_reason),
                yi_types::message::join_text(&content, " "),
            ),
            _ => (None, None, String::new()),
        };
        Self { error, stop, text }
    }
}

fn drain(
    open: impl FnOnce() -> tokio::sync::mpsc::Receiver<AssistantMessageEvent>,
) -> Vec<TestEvent> {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return Vec::new();
    };
    runtime.block_on(async {
        let mut receiver = open();
        let mut events = Vec::new();
        while let Some(event) = receiver.recv().await {
            events.push(TestEvent::of(event));
        }
        events
    })
}

fn responses_turn(text: &str, api: &str, provider: &str, body: String) -> Vec<TestEvent> {
    let Ok((port, server)) = once(framed(&body)) else {
        return Vec::new();
    };
    let Ok(model) = model(api, provider, port) else {
        return Vec::new();
    };
    let opened = if api == "openai-responses" {
        drain(|| {
            yi_ai::openai_responses::stream(&model, &context(), &OpenAiOptions::default(), "k")
        })
    } else {
        drain(|| yi_ai::openai::stream(&model, &context(), &OpenAiOptions::default(), "k"))
    };
    let _ = server.join();
    let _ = text;
    opened
}

fn responses_body() -> String {
    let mut body = [
        "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\n",
        "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n",
        "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\",\"usage\":{\"input_tokens\":7,\"output_tokens\":3}}}\n\n",
    ]
    .concat();
    // A terminator the callback never reads: it stops at `response.completed`, and the padding keeps
    // those bytes past the pump's first 8 KiB read, which is exactly why they must be drained.
    body.push_str(&format!(": {}\n\n", "keep-alive ".repeat(1_200)));
    body
}

/// Incident: `response.completed` ends the stream before the body's terminator, so the reader was
/// dropped where ureq hands it back to its pool, and every turn dialed a new connection.
#[test]
fn two_sequential_responses_turns_reuse_one_connection() -> TestResult {
    let (port, counts, server) = keep_alive(framed(&responses_body()))?;
    let model = model("openai-responses", "openai", port)?;
    for _ in 0..2 {
        let opened = drain(|| {
            yi_ai::openai_responses::stream(&model, &context(), &OpenAiOptions::default(), "k")
        });
        assert!(
            opened.iter().any(|event| event.text.contains("hello")),
            "{opened:?}"
        );
    }
    drop(server);
    assert_eq!(counts.accepts(), 1, "a connection per turn");
    Ok(())
}

/// Eight streams finished together and eight more reuse them: ureq's default keeps one idle
/// connection per host, so the second batch dialed seven more.
#[test]
fn eight_concurrent_streams_then_eight_more_reuse_eight() -> TestResult {
    let (port, counts, server) = keep_alive(framed(&responses_body()))?;
    let model = Arc::new(model("openai-responses", "openai", port)?);
    for _ in 0..2 {
        let mut threads = Vec::new();
        for _ in 0..8 {
            let model = Arc::clone(&model);
            threads.push(std::thread::spawn(move || {
                let opened = drain(|| {
                    yi_ai::openai_responses::stream(
                        &model,
                        &context(),
                        &OpenAiOptions::default(),
                        "k",
                    )
                });
                opened.len()
            }));
        }
        for thread in threads {
            let _ = thread.join();
        }
    }
    drop(server);
    let accepted = counts.accepts();
    assert_eq!(
        accepted, 8,
        "eight streams and a second batch: {accepted} accepts"
    );
    Ok(())
}

/// Incident: a chunk boundary inside a multi-byte character, or an invalid byte, was skipped and the
/// read loop spun on a buffer that could never decode: the turn hung to its 60 s read timeout.
#[test]
fn an_invalid_byte_ends_the_stream_naming_its_offset() -> TestResult {
    let mut body = b"data: {\"a\":1}\n\n".to_vec();
    body.extend_from_slice(&[0xff, 0xfe]);
    body.extend_from_slice(b"data: {\"b\":2}\n\n");
    let head = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n",
        body.len()
    );
    let mut reply = head.into_bytes();
    reply.extend_from_slice(&body);
    let (port, server) = once(reply)?;
    let model = model("openai-completions", "openrouter", port)?;
    let opened =
        drain(|| yi_ai::openai::stream(&model, &context(), &OpenAiOptions::default(), "k"));
    let _ = server.join();
    let error = opened.iter().find_map(|event| event.error.clone());
    let error = error.ok_or_else(|| format!("no error ended the turn: {opened:?}"))?;
    assert!(error.contains("invalid UTF-8"), "{error}");
    assert!(error.contains("offset 15"), "{error}");
    Ok(())
}

/// Incident: an unparsable data line was dropped, so a body that stopped being SSE read as a turn
/// with no text; Anthropic has failed its turn on one since its mapper existed.
#[test]
fn an_unparsable_data_line_ends_the_chat_turn() {
    let opened = responses_turn(
        "",
        "openai-completions",
        "openrouter",
        "data: {\"choices\": nope\n\n".to_owned(),
    );
    let error = opened.iter().find_map(|event| event.error.clone());
    assert!(
        error
            .as_deref()
            .is_some_and(|text| text.contains("Could not parse chat SSE event")),
        "{opened:?}"
    );
}

/// The Responses wire obeys the same rule: a line it cannot parse is not an event it may skip.
#[test]
fn an_unparsable_data_line_ends_the_responses_turn() {
    let opened = responses_turn(
        "",
        "openai-responses",
        "openai",
        "event: response.created\ndata: nope\n\n".to_owned(),
    );
    let error = opened.iter().find_map(|event| event.error.clone());
    assert!(
        error
            .as_deref()
            .is_some_and(|text| text.contains("Could not parse Responses SSE event")),
        "{opened:?}"
    );
}

const ANTHROPIC_CUT: &str = concat!(
    "event: message_start\n",
    "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"probe\",\"usage\":{\"input_tokens\":11,\"output_tokens\":1}}}\n\n",
    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"half an answer\"}}\n\n"
);

const CHAT_CUT: &str = concat!(
    "data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"content\":\"half an answer\"}}]}\n\n",
    "data: {\"id\":\"c1\",\"choices\":[{\"delta\":{},\"finish_reason\":null}],\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":1}}\n\n"
);

const RESPONSES_CUT: &str = concat!(
    "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\n",
    "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"half an answer\"}\n\n"
);

/// The body is framed longer than the bytes that arrive, so the socket closes mid-body: a recording
/// whose stream was cut. Incident: Anthropic and Responses failed a fresh, empty mapper here, and
/// the text and usage the turn had already been billed for were thrown away.
fn cut_turn(body: &str, api: &str, provider: &str) -> Vec<TestEvent> {
    let head = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n",
        body.len().saturating_add(4096)
    );
    let Ok((port, server)) = once(format!("{head}{body}").into_bytes()) else {
        return Vec::new();
    };
    let Ok(model) = model(api, provider, port) else {
        return Vec::new();
    };
    let opened = match api {
        "anthropic-messages" => {
            drain(|| yi_ai::anthropic::stream(&model, &context(), &Default::default(), "k"))
        }
        "openai-responses" => drain(|| {
            yi_ai::openai_responses::stream(&model, &context(), &OpenAiOptions::default(), "k")
        }),
        _ => drain(|| yi_ai::openai::stream(&model, &context(), &OpenAiOptions::default(), "k")),
    };
    let _ = server.join();
    opened
}

#[test]
fn a_stream_cut_after_text_keeps_the_text_on_every_adapter() {
    for (body, api, provider) in [
        (ANTHROPIC_CUT, "anthropic-messages", "anthropic"),
        (CHAT_CUT, "openai-completions", "openrouter"),
        (RESPONSES_CUT, "openai-responses", "openai"),
    ] {
        let opened = cut_turn(body, api, provider);
        let kept = opened
            .iter()
            .find(|event| event.stop == Some(StopReason::Error))
            .map(|event| event.text.clone());
        assert_eq!(
            kept.as_deref(),
            Some("half an answer"),
            "{api}: the cut turn lost its text: {opened:?}"
        );
    }
}

/// The mapper's own contract: two reasoning items are two blocks, so a replay sends two.
#[test]
fn two_reasoning_items_replay_as_two() -> TestResult {
    let model = model("openai-responses", "openai", 1)?;
    let mut mapper = EventMapper::new(&model);
    for (id, encrypted) in [("rs_1", "one"), ("rs_2", "two")] {
        let item = json!({"type":"reasoning","id":id,"encrypted_content":encrypted});
        mapper.push(&json!({"type":"response.output_item.added","item":item}));
        mapper.push(&json!({"type":"response.output_item.done","item":item}));
    }
    let mut events = mapper.push(
        &json!({"type":"response.completed","response":{"id":"resp_1","status":"completed"}}),
    );
    events.extend(mapper.finish());
    let message = events
        .into_iter()
        .find_map(|event| match event {
            AssistantMessageEvent::Done { message, .. }
            | AssistantMessageEvent::Error { error: message, .. } => Some(message),
            _ => None,
        })
        .ok_or("no terminal event")?;
    let AgentMessage::Assistant { content, .. } = &message else {
        return Err("not an assistant message".into());
    };
    let blocks = content
        .iter()
        .filter(|block| matches!(block, Content::Thinking { .. }))
        .count();
    assert_eq!(blocks, 2, "one block per reasoning item: {content:?}");
    Ok(())
}

/// Incident: a refusal rides `response.refusal.*` and the turn ended as an empty Stop, which the
/// loop read as an answer with no text.
#[test]
fn a_responses_refusal_ends_the_turn_as_a_named_error() {
    let Ok(model) = model("openai-responses", "openai", 1) else {
        return;
    };
    let mut mapper = EventMapper::new(&model);
    mapper.push(&json!({"type":"response.refusal.delta","delta":"I can't help"}));
    mapper.push(&json!({"type":"response.refusal.done","refusal":"I can't help with that."}));
    let mut events = mapper.push(
        &json!({"type":"response.completed","response":{"id":"resp_1","status":"completed"}}),
    );
    events.extend(mapper.finish());
    let refused = events.into_iter().find_map(|event| match event {
        AssistantMessageEvent::Error {
            error: AgentMessage::Assistant { error_message, .. },
            ..
        } => error_message,
        _ => None,
    });
    assert_eq!(
        refused.as_deref(),
        Some("The model refused: I can't help with that."),
        "a refusal is the provider's answer, named"
    );
}

/// A cut during a backoff ends the wait: the request the loop no longer wants is never sent.
#[test]
fn an_interrupt_during_a_backoff_sends_no_further_request() -> TestResult {
    let reply = b"HTTP/1.1 429 Too Many Requests\r\nretry-after: 30\r\ncontent-length: 2\r\n\r\nno"
        .to_vec();
    let (port, counts, server) = keep_alive(reply)?;
    let fired = Arc::new(AtomicBool::new(false));
    let body =
        yi_ai::breakpoints::Encoded::provider_prefix(json!({"model": "p"}), "messages", Vec::new());
    let cutting = Arc::clone(&fired);
    let error = yi_ai::request::send_with_retry(
        &format!("http://127.0.0.1:{port}/chat/completions"),
        &[],
        &body,
        None,
        Some(cutting.as_ref()),
        &|_| cutting.store(true, Ordering::SeqCst),
    )
    .err()
    .ok_or("a 429 was taken as a success")?;
    assert!(error.contains("cut during"), "{error}");
    assert_eq!(counts.requests(), 1, "a request after the cut");
    drop(server);
    Ok(())
}

/// Incident: a `retry-after: 61` was slept through and the same bytes sent again; past the cap the
/// window is the provider's and the turn ends naming the wait.
#[test]
fn a_wait_past_the_cap_ends_the_turn_and_one_inside_it_retries() -> TestResult {
    let too_long =
        b"HTTP/1.1 429 Too Many Requests\r\nretry-after: 61\r\ncontent-length: 2\r\n\r\nno"
            .to_vec();
    let (port, counts, server) = keep_alive(too_long)?;
    let body =
        yi_ai::breakpoints::Encoded::provider_prefix(json!({"model": "p"}), "messages", Vec::new());
    let error = yi_ai::request::send_with_retry(
        &format!("http://127.0.0.1:{port}/chat/completions"),
        &[],
        &body,
        None,
        None,
        &|_| {},
    )
    .err()
    .ok_or("a 429 was taken as a success")?;
    assert!(error.contains("61"), "the wait is named: {error}");
    assert!(error.contains("retry cap"), "{error}");
    assert_eq!(counts.requests(), 1, "a request after a refused wait");
    drop(server);

    let inside =
        b"HTTP/1.1 429 Too Many Requests\r\nretry-after: 0.2\r\ncontent-length: 2\r\n\r\nno"
            .to_vec();
    let (port, counts, server) = keep_alive(inside)?;
    let error = yi_ai::request::send_with_retry(
        &format!("http://127.0.0.1:{port}/chat/completions"),
        &[],
        &body,
        None,
        None,
        &|_| {},
    )
    .err()
    .ok_or("a 429 was taken as a success")?;
    assert!(error.starts_with("HTTP 429"), "{error}");
    assert!(
        counts.requests() >= 2,
        "a wait inside the cap is honored and retried"
    );
    drop(server);
    Ok(())
}

/// Overflow bodies are the provider refusing the request, not the wire failing: neither lifts on a
/// resend, and both classed as transport http 400.
#[test]
fn overflow_bodies_class_as_overflow() {
    use yi_types::telemetry::ErrorClass;
    let openai = r#"HTTP 400: {"error":{"message":"This model's maximum context length is 128000 tokens.","code":"context_length_exceeded"}}"#;
    let anthropic = "HTTP 400: {\"type\":\"error\",\"error\":{\"type\":\"invalid_request_error\",\"message\":\"prompt is too long: 208310 tokens > 200000 maximum\"}}";
    for body in [openai, anthropic] {
        let class = ErrorClass::from_provider_text(body);
        assert_eq!(class.to_string(), "provider:overflow", "{body}");
        assert!(!class.is_transport(), "{body}");
    }
}

/// A 400 refuses the same bytes again: the loop spends no second request and no steer on it.
#[test]
fn a_400_body_ends_the_turn_at_once() -> TestResult {
    let refused = b"HTTP/1.1 400 Bad Request\r\ncontent-type: application/json\r\ncontent-length: 74\r\n\r\n{\"error\":{\"message\":\"Invalid schema for function 'done'\",\"type\":\"invalid\"}}".to_vec();
    let (port, counts, server) = keep_alive(refused)?;
    let body =
        yi_ai::breakpoints::Encoded::provider_prefix(json!({"model": "p"}), "messages", Vec::new());
    let error = yi_ai::request::send_with_retry(
        &format!("http://127.0.0.1:{port}/chat/completions"),
        &[],
        &body,
        None,
        None,
        &|_| {},
    )
    .err()
    .ok_or("a 400 was taken as a success")?;
    assert!(error.starts_with("HTTP 400"), "{error}");
    assert_eq!(counts.requests(), 1, "a 400 was sent again");
    assert!(
        !yi_types::telemetry::ErrorClass::from_provider_text(&error).is_transport(),
        "a 400 is not the wire"
    );
    drop(server);
    Ok(())
}
