//! A gateway that says nothing until its first token is ready, against the model's own deadline.
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use yi_ai::request::{Retry, pump_sse_with_resend, send_with_retry};
use yi_types::model::DEFAULT_READ_TIMEOUT_SECS;

use crate::common::model;
use crate::loopback_server::{Script, serve_scripted};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Served = std::io::Result<(u16, Arc<AtomicUsize>, std::thread::JoinHandle<Vec<String>>)>;

const SSE_HEAD: &str = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n";

/// Serves `replies` requests, each read whole and then held `stall` before its first byte.
fn silent_at_first(stall: Duration, replies: usize) -> Served {
    serve_scripted(std::iter::repeat_n(
        Script::new(SSE_HEAD).stall(stall),
        replies,
    ))
}

/// Serves `replies` requests: headers at once, then `chunks` events, `gap` apart.
fn trickle(gap: Duration, chunks: usize, replies: usize) -> Served {
    let events: Vec<String> = (0..chunks)
        .map(|n| format!("data: {{\"n\":{n}}}\n\n"))
        .collect();
    serve_scripted(std::iter::repeat_n(
        Script::new(SSE_HEAD).then(gap, events),
        replies,
    ))
}

fn probe() -> yi_ai::breakpoints::Encoded {
    yi_ai::breakpoints::Encoded::provider_prefix(
        serde_json::json!({"model": "probe"}),
        "messages",
        Vec::new(),
    )
}

fn deadline(secs: u64) -> yi_types::model::Model {
    let mut model = model("m", "openai-chat", "p", "http://127.0.0.1:0", None);
    model.read_timeout_secs = Some(secs);
    model
}

/// Dies with a silent gateway spending one 60 s wait per retry: the incident ran four commands
/// and paid four stalls for one answer.
#[test]
fn a_gateway_silent_past_the_models_deadline_names_the_field_and_runs_once() -> TestResult {
    let (port, served, server) = silent_at_first(Duration::from_secs(30), 3)?;
    let url = format!("http://127.0.0.1:{port}/v1/chat/completions");
    let body = probe();
    let retry = Retry::new(deadline(1).read_timeout(), None, &|_| {});
    let failed = send_with_retry(&url, &[], &body, None, &retry)
        .err()
        .ok_or("a gateway silent past the deadline answered")?;
    assert!(failed.contains("after 1 s"), "{failed}");
    assert!(failed.contains("readTimeoutSecs"), "{failed}");
    assert!(
        failed.contains(&format!("default {DEFAULT_READ_TIMEOUT_SECS}")),
        "{failed}"
    );
    assert!(failed.contains("1 attempt ran"), "{failed}");
    assert_eq!(served.load(Ordering::SeqCst), 1, "{failed}");
    drop(server);
    Ok(())
}

/// Dies with a deadline no model could raise under it: the field is the model's, not the client's.
#[test]
fn a_gateway_silent_within_the_models_deadline_answers() -> TestResult {
    let (port, _, server) = silent_at_first(Duration::from_secs(1), 1)?;
    let url = format!("http://127.0.0.1:{port}/v1/chat/completions");
    let body = probe();
    let response = send_with_retry(
        &url,
        &[],
        &body,
        None,
        &Retry::new(deadline(5).read_timeout(), None, &|_| {}),
    )?;
    assert_eq!(response.status(), 200);
    server.join().map_err(|_| "server thread panicked")?;
    Ok(())
}

/// Dies with a stream that died after its headers being sent again: the gateway had proved
/// silent for the model's whole deadline, and the resend spent it twice.
#[test]
fn a_stream_that_stalls_after_its_headers_is_not_resent() -> TestResult {
    let (port, served, server) = trickle(Duration::from_secs(30), 1, 2)?;
    let url = format!("http://127.0.0.1:{port}/v1/chat/completions");
    let body = probe();
    let retry = Retry::new(deadline(1).read_timeout(), None, &|_| {});
    let failed = pump_sse_with_resend(
        None,
        &retry,
        || send_with_retry(&url, &[], &body, None, &retry),
        |_| Ok(true),
    )
    .err()
    .ok_or("a stream that stalled after its headers completed")?;
    assert!(failed.contains("readTimeoutSecs"), "{failed}");
    assert!(failed.contains("1 attempt ran"), "{failed}");
    assert_eq!(served.load(Ordering::SeqCst), 1, "{failed}");
    drop(server);
    Ok(())
}

/// Dies with an unset field meaning whatever the build's flag said: the catalog entry that
/// cannot afford 60 s must be able to say less, and one that says nothing keeps 60.
#[test]
fn an_unset_field_keeps_sixty_seconds() {
    assert_eq!(DEFAULT_READ_TIMEOUT_SECS, 60);
    let unset = model("m", "openai-chat", "p", "http://127.0.0.1:0", None);
    assert_eq!(unset.read_timeout(), Duration::from_secs(60));
    assert_eq!(deadline(9).read_timeout(), Duration::from_secs(9));
}

/// Holds the decision row's claim on what the field bounds: ureq applies its read timeout per
/// read, so a body slower than the deadline completes while a silent gap over it does not. A
/// total-body bound would have killed every generation past 60 s.
#[test]
fn the_deadline_bounds_the_gap_between_chunks_not_the_whole_body() -> TestResult {
    let (port, _, server) = trickle(Duration::from_millis(300), 6, 1)?;
    let url = format!("http://127.0.0.1:{port}/v1/chat/completions");
    let body = probe();
    let retry = Retry::new(deadline(1).read_timeout(), None, &|_| {});
    let mut events = 0usize;
    let pumped = pump_sse_with_resend(
        None,
        &retry,
        || send_with_retry(&url, &[], &body, None, &retry),
        |_| {
            events = events.saturating_add(1);
            Ok(true)
        },
    );
    assert!(pumped.is_ok(), "a kept-alive body died: {pumped:?}");
    assert_eq!(
        events, 6,
        "every event of a body slower than the deadline arrived"
    );
    drop(server);
    Ok(())
}

/// Dies with an adapter that hands the request a constant instead of the model's own field: an
/// author's `readTimeoutSecs` would be ignored and the catalog entry could never raise it.
#[test]
fn every_streaming_adapter_reads_the_deadline_from_the_model() -> TestResult {
    use yi_types::event::AssistantMessageEvent;
    use yi_types::message::AgentMessage;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    for api in ["anthropic", "chat", "responses"] {
        let (port, _, server) = silent_at_first(Duration::from_secs(30), 1)?;
        let mut model = deadline(1);
        model.base_url = format!("http://127.0.0.1:{port}");
        let context = yi_types::model::LlmContext {
            cache_ttl: yi_types::model::Ttl::Min5,
            cache_tail: true,
            system_prompt: String::new(),
            messages: Vec::new(),
            transient: Vec::new(),
            schema: None,
            shared_through: None,
            reuse: yi_types::model::Reuse::Loop,
            tools: None,
            tool_choice: None,
        };
        let options = yi_ai::openai::OpenAiOptions::default();
        let failed = runtime.block_on(async {
            let mut receiver = match api {
                "anthropic" => yi_ai::anthropic::stream(
                    &model,
                    &context,
                    &yi_ai::anthropic::AnthropicOptions::default(),
                    "k",
                ),
                "chat" => yi_ai::openai::stream(&model, &context, &options, "k"),
                _ => yi_ai::openai_responses::stream(&model, &context, &options, "k"),
            };
            let mut failed = None;
            while let Some(event) = receiver.recv().await {
                if let AssistantMessageEvent::Error {
                    error: AgentMessage::Assistant { error_message, .. },
                    ..
                } = event
                {
                    failed = error_message;
                }
            }
            failed
        });
        let failed = failed.ok_or("a silent gateway did not fail the stream")?;
        assert!(failed.contains("after 1 s"), "{api}: {failed}");
        drop(server);
    }
    Ok(())
}

const EVENT: &str = "data: {\"n\":0}\n\n";

fn event_reply() -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n{EVENT}",
        EVENT.len()
    )
}

/// What a test does with the second request that arrives on the kept-alive socket.
type OnReused = Box<dyn FnOnce(std::io::BufReader<std::net::TcpStream>) + Send>;

/// A gateway that answers one request, keeps its socket open, and hands the next request on that
/// socket to `on_reused`; any later connection is answered at once. Returns the port and the
/// number of connections accepted. The first send is pumped, so its agent holds a reusable socket.
fn reused_socket(
    on_reused: OnReused,
) -> std::io::Result<(u16, Arc<AtomicUsize>, std::thread::JoinHandle<()>)> {
    use std::io::{BufReader, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&accepted);
    let server = std::thread::spawn(move || {
        let Ok((first, _)) = listener.accept() else {
            return;
        };
        count.fetch_add(1, Ordering::SeqCst);
        let mut first = BufReader::new(first);
        crate::loopback::read_request(&mut first);
        let _ = first.get_mut().write_all(event_reply().as_bytes());
        crate::loopback::read_request(&mut first);
        let handler = std::thread::spawn(move || on_reused(first));
        for later in listener.incoming().flatten() {
            count.fetch_add(1, Ordering::SeqCst);
            let mut later = BufReader::new(later);
            crate::loopback::read_request(&mut later);
            let _ = later.get_mut().write_all(event_reply().as_bytes());
        }
        let _ = handler.join();
    });
    Ok((port, accepted, server))
}

/// Sends once and pumps the reply, so the agent for `deadline` holds a socket to reuse.
fn warm(url: &str, retry: &Retry<'_>) -> TestResult {
    let body = probe();
    let pumped = pump_sse_with_resend(
        None,
        retry,
        || send_with_retry(url, &[], &body, None, retry),
        |_| Ok(true),
    );
    assert!(pumped.is_ok(), "{pumped:?}");
    Ok(())
}

/// Dies with a pooled socket a NAT dropped: the request written into it waited forever, because
/// ureq clears the read deadline when a socket returns to its pool. Run under nextest, one process
/// a test: the agent pool is process-wide and three slots wide.
#[test]
fn a_silent_reused_socket_ends_at_the_deadline_without_a_second_send() -> TestResult {
    let (port, accepted, _server) = reused_socket(Box::new(|mut held| {
        let mut rest = String::new();
        let _ = std::io::BufRead::read_line(&mut held, &mut rest);
    }))?;
    let url = format!("http://127.0.0.1:{port}/v1/chat/completions");
    let retry = Retry::new(Duration::from_millis(1111), None, &|_| {});
    warm(&url, &retry)?;
    let body = probe();
    let failed = send_with_retry(&url, &[], &body, None, &retry)
        .err()
        .ok_or("a silent reused socket answered")?;
    assert!(failed.contains("readTimeoutSecs"), "{failed}");
    assert_eq!(
        accepted.load(Ordering::SeqCst),
        1,
        "the prompt was sent twice"
    );
    let next = send_with_retry(&url, &[], &body, None, &retry)?;
    assert_eq!(
        next.status(),
        200,
        "the next request dials a new connection"
    );
    Ok(())
}

/// Dies with a slow but live reused connection written to twice: the server generated the reply
/// twice and the user paid twice.
#[test]
fn a_slow_reused_socket_is_not_sent_the_prompt_again() -> TestResult {
    let (port, accepted, _server) = reused_socket(Box::new(|mut held| {
        std::thread::sleep(Duration::from_millis(1700));
        let _ = std::io::Write::write_all(held.get_mut(), event_reply().as_bytes());
    }))?;
    let url = format!("http://127.0.0.1:{port}/v1/chat/completions");
    let retry = Retry::new(Duration::from_millis(1222), None, &|_| {});
    warm(&url, &retry)?;
    let body = probe();
    let failed = send_with_retry(&url, &[], &body, None, &retry)
        .err()
        .ok_or("a reply past the deadline arrived")?;
    assert!(failed.contains("after 1 s"), "{failed}");
    assert_eq!(accepted.load(Ordering::SeqCst), 1, "{failed}");
    Ok(())
}

/// Dies with a total bound on a reused socket's stream: the deadline is one read, so a body
/// slower than it in all still completes.
#[test]
fn a_reused_socket_streams_past_the_deadline_in_total() -> TestResult {
    let (port, _, _server) = reused_socket(Box::new(|mut held| {
        let stream = &mut *held.get_mut();
        let _ = std::io::Write::write_all(
            stream,
            b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n",
        );
        for n in 0..6 {
            std::thread::sleep(Duration::from_millis(300));
            let _ =
                std::io::Write::write_all(stream, format!("data: {{\"n\":{n}}}\n\n").as_bytes());
        }
    }))?;
    let url = format!("http://127.0.0.1:{port}/v1/chat/completions");
    let retry = Retry::new(Duration::from_millis(1333), None, &|_| {});
    warm(&url, &retry)?;
    let body = probe();
    let mut events = 0usize;
    let pumped = pump_sse_with_resend(
        None,
        &retry,
        || send_with_retry(&url, &[], &body, None, &retry),
        |_| {
            events = events.saturating_add(1);
            Ok(true)
        },
    );
    assert!(pumped.is_ok(), "{pumped:?}");
    assert_eq!(events, 6);
    Ok(())
}
