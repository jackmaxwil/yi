use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::sync::Arc;

use yi_ai::faux::{faux_assistant_message, faux_text};
use yi_context::{Settings, Tokens};
use yi_loop::ExecutionMode;
use yi_runtime::{AgentSession, ProviderStream, SessionConfig};
use yi_session::{CreateOptions, JsonlRepo, SessionRepo};
use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, StopReason, UserContent};
use yi_types::model::{Effort, Model, ModelCost};

pub(crate) fn faux_model(context_window: u64) -> Model {
    let zero = || serde_json::Number::from(0u64);
    Model {
        id: "faux-1".to_owned(),
        name: "Faux".to_owned(),
        api: "faux".to_owned(),
        provider: "faux".to_owned(),
        base_url: "http://localhost:0".to_owned(),
        reasoning: false,
        input: vec!["text".to_owned()],
        cost: ModelCost {
            input: zero(),
            output: zero(),
            cache_read: zero(),
            cache_write: zero(),
            tiers: None,
        },
        context_window,
        max_tokens: 16_384,
        compat: None,
        thinking_level_map: None,
        headers: None,
    }
}

fn reply_with_usage(text: &str, input: i64, total: i64) -> AgentMessage {
    let mut message = faux_assistant_message(vec![faux_text(text)], StopReason::Stop);
    if let AgentMessage::Assistant { usage, .. } = &mut message {
        usage.input = input;
        usage.output = total.saturating_sub(input);
        usage.total_tokens = total;
    }
    message
}

fn tight_settings() -> Settings {
    Settings {
        enabled: true,
        reserve_tokens: Tokens(1_000),
        keep_recent_tokens: Tokens(10),
    }
}

fn session_for_compaction(provider: Arc<ProviderStream>) -> AgentSession {
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(2_000),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );
    session.enable_compaction_with(tight_settings());
    session
}

fn compaction_entries(store: &yi_session::SharedSession) -> Vec<Entry> {
    yi_session::lock_session(store)
        .find_entries(&yi_session::EntryQuery {
            order: yi_session::EntryOrder::OldestFirst,
            ..yi_session::EntryQuery::default()
        })
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| matches!(entry, Entry::Compaction { .. }))
        .collect()
}

#[tokio::test]
async fn auto_compaction_fires_at_the_message_boundary_and_persists() -> Result<(), Box<dyn Error>>
{
    let root = Scratch::new("yi-compact-e2e")?;
    let mut repo = JsonlRepo::new(root.to_path_buf(), "/tmp/yi-compact-test");
    let store = repo.create(CreateOptions {
        id: Some("compact-one".to_owned()),
        ..CreateOptions::default()
    })?;

    let provider = Arc::new(ProviderStream::new(None));
    provider.queue_faux(vec![
        reply_with_usage(&format!("big reply {}", "x".repeat(400)), 100, 5_000),
        faux_assistant_message(
            vec![faux_text("## Goal\nSummarized history for the test")],
            StopReason::Stop,
        ),
        reply_with_usage("second answer", 50, 300),
    ]);
    let session = session_for_compaction(provider);
    session.attach_store(Arc::clone(&store))?;

    session.prompt("first requirement: keep the guardrails green")?;
    session.wait_idle().await;
    assert!(compaction_entries(&store).is_empty());

    session.prompt("second ask with enough characters to keep recent")?;
    session.wait_idle().await;
    assert_eq!(session.store_error(), None);

    let compactions = compaction_entries(&store);
    assert_eq!(compactions.len(), 1);
    let Entry::Compaction {
        summary,
        retained_tail,
        tokens_before,
        details,
        ..
    } = &compactions[0]
    else {
        return Err("expected compaction entry".into());
    };
    assert!(summary.contains("Summarized history for the test"));
    assert!(*tokens_before > 0);
    assert!(
        retained_tail.iter().any(|message| matches!(
            message,
            AgentMessage::User { content: UserContent::Text(text), .. }
                if text.contains("first requirement")
        )),
        "retention floor must keep the first user message verbatim"
    );
    let details = details.clone().ok_or("expected details")?;
    let parsed: yi_types::compaction::CompactionDetails = serde_json::from_value(details)?;
    let window = parsed.window.ok_or("expected window ids")?;
    assert_eq!(window.number, 1);
    assert!(window.previous.is_some());

    let in_memory = session.messages();
    assert!(matches!(
        in_memory.first(),
        Some(AgentMessage::CompactionSummary { .. })
    ));

    drop(session);
    drop(store);
    let reopened = repo.open("compact-one")?;
    let resumed = {
        let provider = Arc::new(ProviderStream::new(None));
        let mut session = session_for_compaction(provider);
        session.enable_compaction_with(Settings {
            enabled: false,
            ..tight_settings()
        });
        session
    };
    resumed.attach_store(reopened)?;
    let loaded = resumed.messages();
    assert!(
        matches!(
            loaded.first(),
            Some(AgentMessage::CompactionSummary { summary, .. })
                if summary.contains("Summarized history for the test")
        ),
        "projection after reopen must start from the compaction summary"
    );
    Ok(())
}

#[tokio::test]
async fn compact_now_applies_immediately_when_idle() -> Result<(), Box<dyn Error>> {
    let provider = Arc::new(ProviderStream::new(None));
    provider.queue_faux(vec![
        reply_with_usage(&format!("long body {}", "y".repeat(400)), 100, 5_000),
        faux_assistant_message(vec![faux_text("## Goal\nIdle summary")], StopReason::Stop),
    ]);
    let session = session_for_compaction(provider);
    let compacted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let compacted = Arc::clone(&compacted);
        session.set_on_compacted(Arc::new(move || {
            compacted.store(true, std::sync::atomic::Ordering::SeqCst);
        }));
    }
    session.prompt("please do the thing with sufficient text here")?;
    session.wait_idle().await;

    assert!(
        session
            .compact_now()
            .await
            .is_ok_and(|outcome| outcome.applied())
    );
    let messages = session.messages();
    assert!(matches!(
        messages.first(),
        Some(AgentMessage::CompactionSummary { summary, .. }) if summary.contains("Idle summary")
    ));
    assert!(
        compacted.load(std::sync::atomic::Ordering::SeqCst),
        "an applied compaction must fire the post-compaction hook (kernel sync)"
    );
    Ok(())
}

/// The summarizer ran but the entry never reached disk: the live history must stay what a
/// resume loads, the model must read why at the next request, and a disk that stays broken
/// must not buy another summarizer call at every boundary.
#[tokio::test]
async fn a_compaction_that_fails_to_write_is_not_applied() -> Result<(), Box<dyn Error>> {
    let root = Scratch::new("yi-compact-unsaved")?;
    let mut repo = JsonlRepo::new(root.to_path_buf(), "/tmp/yi-compact-test");
    let store = repo.create(CreateOptions {
        id: Some("unsaved".to_owned()),
        ..CreateOptions::default()
    })?;
    let provider = Arc::new(ProviderStream::new(None));
    provider.queue_faux(vec![
        reply_with_usage(&format!("long body {}", "y".repeat(400)), 100, 5_000),
        faux_assistant_message(
            vec![faux_text("## Goal\nUnsaved summary")],
            StopReason::Stop,
        ),
        reply_with_usage("after the notice", 50, 300),
    ]);
    let session = session_for_compaction(provider);
    session.attach_store(Arc::clone(&store))?;
    session.prompt("please do the thing with sufficient text here")?;
    session.wait_idle().await;

    // A path swap, not a chmod: CI runs as root, and a directory refuses the append (EISDIR).
    let path = yi_session::lock_session(&store)
        .file_path()
        .cloned()
        .ok_or("file-backed store")?;
    let aside = path.with_extension("aside");
    std::fs::rename(&path, &aside)?;
    std::fs::create_dir(&path)?;
    let applied = session
        .compact_now()
        .await
        .is_ok_and(|outcome| outcome.applied());
    let live = session.messages();
    let error = session.store_error().unwrap_or_default();
    session.prompt("next ask")?;
    session.wait_idle().await;
    std::fs::remove_dir(&path)?;
    std::fs::rename(&aside, &path)?;

    let resumed = session_for_compaction(Arc::new(ProviderStream::new(None)));
    resumed.attach_store(repo.open("unsaved")?)?;
    assert_eq!(
        live,
        resumed.messages(),
        "live history must be what a resume loads"
    );
    assert!(!applied, "an unsaved compaction must not report applied");
    assert!(
        error.contains("Failed to append session"),
        "store_error: {error:?}"
    );

    let messages = session.messages();
    let text_of = |message: &AgentMessage| match message {
        AgentMessage::User {
            content: UserContent::Text(text),
            ..
        }
        | AgentMessage::Custom {
            content: UserContent::Text(text),
            ..
        } => text.clone(),
        AgentMessage::Assistant { content, .. } => {
            serde_json::to_string(content).unwrap_or_default()
        }
        _ => String::new(),
    };
    let texts: Vec<String> = messages.iter().map(text_of).collect();
    let (answer, asked) = texts.split_last().ok_or("no reply")?;
    assert!(
        answer.contains("after the notice"),
        "the queued reply must answer the turn, not feed a second summarizer call: {texts:#?}"
    );
    assert!(
        asked
            .iter()
            .any(|text| text.starts_with("[compaction not saved: ")
                && text.contains("Failed to append session")
                && text.ends_with("history left uncompacted, /compact retries]")),
        "the next request must carry the notice: {texts:#?}"
    );
    Ok(())
}

/// Both recall tests need the same shape: a probe turn whose long tool body hides `needle`,
/// then two live asks that trip the boundary and compact that turn away.
struct Recalled {
    store: yi_session::SharedSession,
    session: AgentSession,
    needle_id: String,
    body: String,
    root: Scratch,
}

async fn seed_and_compact(tag: &str, needle: &str) -> Result<Recalled, Box<dyn Error>> {
    let root = Scratch::new(&format!("yi-compact-{tag}"))?;
    let mut repo = JsonlRepo::new(root.to_path_buf(), format!("/tmp/yi-compact-{tag}"));
    let store = repo.create(CreateOptions {
        id: Some(format!("compact-{tag}")),
        ..CreateOptions::default()
    })?;
    // The keep-recent window drops this turn and the 160-char brief line caps before the
    // needle, so only the entry id survives in the window.
    let body = format!("{} {needle} {}", "preamble ".repeat(40), "tail ".repeat(80));
    let needle_id = {
        let mut guard = yi_session::lock_session(&store);
        guard.append_message(
            "main",
            AgentMessage::user_input(UserContent::Text("early ask: run the probe".to_owned()), 0),
        )?;
        guard.append_message(
            "main",
            faux_assistant_message(
                vec![yi_types::message::Content::ToolCall {
                    id: "call-1".to_owned(),
                    name: "probe".to_owned(),
                    arguments: serde_json::Map::new(),
                    thought_signature: None,
                    namespace: None,
                }],
                StopReason::ToolUse,
            ),
        )?;
        guard.append_message(
            "main",
            AgentMessage::ToolResult {
                tool_call_id: "call-1".to_owned(),
                tool_name: "probe".to_owned(),
                content: vec![yi_types::message::Content::Text {
                    text: body.clone(),
                    text_signature: None,
                }],
                details: None,
                usage: None,
                added_tool_names: None,
                is_error: false,
                timestamp: 0,
            },
        )?
    };

    let provider = Arc::new(ProviderStream::new(None));
    provider.queue_faux(vec![
        reply_with_usage(&format!("big reply {}", "x".repeat(400)), 100, 5_000),
        faux_assistant_message(
            vec![faux_text("## Goal\nSummarized history for the recall test")],
            StopReason::Stop,
        ),
        reply_with_usage("second answer", 50, 300),
    ]);
    let session = session_for_compaction(provider);
    session.attach_store(Arc::clone(&store))?;
    session.prompt("first live ask with enough text to matter")?;
    session.wait_idle().await;
    session.prompt("second live ask trips the compaction boundary")?;
    session.wait_idle().await;
    assert_eq!(session.store_error(), None);
    Ok(Recalled {
        store,
        session,
        needle_id,
        body,
        root,
    })
}

/// The recall half of compaction: a turn the keep-recent window drops stays
/// discoverable through `SessionStore::grep` and fetchable through
/// `history://`, and the compact view cites its entry id.
#[tokio::test]
async fn a_compacted_away_turn_stays_greppable_and_fetchable() -> Result<(), Box<dyn Error>> {
    let needle = "NEEDLE-ZEBRA-4182";
    let seeded = seed_and_compact("recall", needle).await?;
    assert_eq!(compaction_entries(&seeded.store).len(), 1);

    let live_text = live_text(&seeded.session)?;
    assert!(
        !live_text.contains(needle),
        "the compacted-away needle must leave the live window: {live_text}"
    );
    assert!(
        live_text.contains(&format!("(#{})", seeded.needle_id)),
        "the compact view must cite the dropped turn by entry id: {live_text}"
    );

    let hits = yi_session::lock_session(&seeded.store).grep(needle, 8);
    assert!(
        hits.iter().any(|hit| hit.entry_id == seeded.needle_id),
        "store grep must find the compacted-away needle: {hits:?}"
    );

    let fetched = fetch_entry(&seeded)?;
    assert!(
        fetched.contains(&seeded.body),
        "history:// must serve the full compacted-away blob: {fetched}"
    );
    Ok(())
}

/// Compact still emits the extractive view and `history://` still fetches the pointed-to entry.
#[tokio::test]
async fn compact_view_cites_a_tool_entry_that_history_can_fetch() -> Result<(), Box<dyn Error>> {
    let ident = "crates/fold_probe/mod.rs";
    let seeded = seed_and_compact("view", ident).await?;
    let live_text = live_text(&seeded.session)?;
    assert!(
        live_text.contains("<yi_compact_view>")
            && live_text.contains(&format!("(#{})", seeded.needle_id)),
        "compact must emit the view with entry pointers: {live_text}"
    );
    let fetched = fetch_entry(&seeded)?;
    assert!(
        fetched.contains(ident),
        "history:// must fetch the pointed-to entry: {fetched}"
    );
    Ok(())
}

fn live_text(session: &AgentSession) -> Result<String, Box<dyn Error>> {
    Ok(session
        .messages()
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()?
        .join("\n"))
}

fn fetch_entry(seeded: &Recalled) -> Result<String, Box<dyn Error>> {
    let resolver =
        yi_runtime::fetch::Resolver::new(seeded.root.to_path_buf(), yi_runtime::Wall::default())
            .with_session_handle("main", seeded.session.store_handle());
    let url: yi_types::url::Url = format!("history://main/{}", seeded.needle_id).parse()?;
    Ok(resolver.fetch(&url)?.text)
}

/// #950 F1: the tail an idle `/compact` keeps holds the reply whose usage counted the whole
/// history before it. Read as the count, it made the next prompt compact again: a second summary
/// request, summarizing a summary.
#[tokio::test]
async fn the_prompt_after_an_idle_compaction_does_not_compact_again() -> Result<(), Box<dyn Error>>
{
    let provider = Arc::new(ProviderStream::new(None));
    let summary = |text: &str| faux_assistant_message(vec![faux_text(text)], StopReason::Stop);
    provider.queue_faux(vec![
        reply_with_usage(&"answer ".repeat(20), 1_400, 1_500),
        summary("## Goal\nFIRST"),
        summary("ok"),
        summary("## Goal\nSECOND"),
    ]);
    let session = session_for_compaction(Arc::clone(&provider));
    session.prompt("ask")?;
    session.wait_idle().await;
    assert!(
        session
            .compact_now()
            .await
            .is_ok_and(|outcome| outcome.applied()),
        "the idle compaction applies"
    );
    session.prompt("next ask")?;
    session.wait_idle().await;
    let left = provider
        .faux
        .lock()
        .map_err(|_| "faux lock")?
        .pending_response_count();
    assert_eq!(
        left, 1,
        "one compaction: the next prompt's request took the reply, no second summary"
    );
    let messages = session.messages();
    assert!(
        matches!(messages.first(), Some(AgentMessage::CompactionSummary { summary, .. })
            if summary.ends_with("FIRST")),
        "the idle compaction's summary still leads: {messages:#?}"
    );
    Ok(())
}

#[tokio::test]
async fn compaction_below_threshold_is_a_no_op() -> Result<(), Box<dyn Error>> {
    let provider = Arc::new(ProviderStream::new(None));
    provider.queue_faux(vec![
        reply_with_usage("tiny", 10, 50),
        reply_with_usage("also tiny", 10, 60),
    ]);
    let session = session_for_compaction(provider);
    session.prompt("one")?;
    session.wait_idle().await;
    session.prompt("two")?;
    session.wait_idle().await;
    assert!(
        !session
            .messages()
            .iter()
            .any(|message| matches!(message, AgentMessage::CompactionSummary { .. }))
    );
    Ok(())
}

/// Dies with the summarizer running unannounced: the turn sat on "Waiting…" for the whole
/// summary, with nothing to say the context was being condensed.
#[tokio::test]
async fn a_compaction_announces_itself_and_closes() -> Result<(), Box<dyn Error>> {
    let root = Scratch::new("yi-compact-wait")?;
    let mut repo = JsonlRepo::new(root.to_path_buf(), "/tmp/yi-compact-wait");
    let store = repo.create(CreateOptions {
        id: Some("compact-wait".to_owned()),
        ..CreateOptions::default()
    })?;
    let provider = Arc::new(ProviderStream::new(None));
    provider.queue_faux(vec![
        reply_with_usage(&format!("big reply {}", "x".repeat(400)), 100, 5_000),
        faux_assistant_message(vec![faux_text("## Goal\nSummary")], StopReason::Stop),
        reply_with_usage("second answer", 50, 300),
    ]);
    let session = session_for_compaction(provider);
    session.attach_store(Arc::clone(&store))?;
    session.prompt("first requirement: keep the guardrails green")?;
    session.wait_idle().await;
    let mut events = session.subscribe();
    session.prompt("second ask with enough characters to keep recent")?;
    session.wait_idle().await;
    let mut waits = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let yi_types::event::AgentEvent::Wait { wait } = event {
            waits.push(wait);
        }
    }
    assert!(
        matches!(
            waits.as_slice(),
            [Some(yi_types::event::Wait::Compaction { tokens }), None] if *tokens > 0
        ),
        "{waits:?}"
    );
    Ok(())
}

/// Incident: the loop's hook ran before every request, copied the history, read the store and
/// rendered the system prompt and tools, only to learn compaction was not due.
#[tokio::test]
async fn a_request_that_is_not_due_assembles_nothing() -> Result<(), Box<dyn Error>> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let mut compactor = yi_runtime::Compactor::new("w".to_owned());
    compactor.settings = tight_settings();
    let asked = Arc::new(AtomicUsize::new(0));
    let (requests, stores) = (Arc::clone(&asked), Arc::clone(&asked));
    let hook = yi_runtime::compaction::loop_hook(
        Arc::new(compactor),
        Arc::new(ProviderStream::new(None)),
        Arc::new(move |effort| {
            requests.fetch_add(1, Ordering::SeqCst);
            yi_runtime::compaction::LoopRequest::new("sys".to_owned(), &[], effort)
        }),
        Arc::new(move || {
            stores.fetch_add(1, Ordering::SeqCst);
            None
        }),
        yi_runtime::compaction::CompactReports {
            waiting: Arc::new(|_| {}),
            compacted: Arc::new(|| {}),
            failed: Arc::new(|_| {}),
            elided: Arc::new(|_| {}),
        },
    );
    let history = [reply_with_usage("small", 10, 20)];
    assert!(
        hook(&history, &faux_model(2_000), Effort::Off)
            .await
            .is_none()
    );
    assert_eq!(asked.load(Ordering::SeqCst), 0);
    Ok(())
}

/// Dies with a host-written message counted into the index: every `user://` address the
/// summarizer's key and a produced summary name is fetched and must serve the user's own words.
#[tokio::test]
async fn a_summarys_user_addresses_resolve_to_the_users_own_words() -> Result<(), Box<dyn Error>> {
    let root = Scratch::new("yi-compact-cites")?;
    let mut repo = JsonlRepo::new(root.to_path_buf(), "/tmp/yi-compact-cites");
    let store = repo.create(CreateOptions {
        id: Some("compact-cites".to_owned()),
        ..CreateOptions::default()
    })?;
    let asks = [
        "first requirement: keep the guardrails green",
        "second ask with enough characters to keep recent",
    ];
    let provider = Arc::new(ProviderStream::new(None));
    provider.queue_faux(vec![
        reply_with_usage(&format!("big reply {}", "x".repeat(400)), 100, 5_000),
        faux_assistant_message(
            vec![faux_text(
                "## Goal\n- user://2\n\n## Constraints & Preferences\n- user://1: \"keep the guardrails green\"",
            )],
            StopReason::Stop,
        ),
        reply_with_usage("second answer", 50, 300),
    ]);
    let session = session_for_compaction(provider);
    session.attach_store(Arc::clone(&store))?;
    for ask in asks {
        session.prompt_message(yi_runtime::session::user_input(ask))?;
        session.wait_idle().await;
        let host = AgentMessage::host_user(UserContent::Text("[host] a reminder".to_owned()), 0);
        yi_session::lock_session(&store).append_message("main", host)?;
    }

    let compactions = compaction_entries(&store);
    let Some(Entry::Compaction { summary, .. }) = compactions.first() else {
        return Err("expected a compaction entry".into());
    };
    let resolver =
        yi_runtime::fetch::Resolver::new(root.to_path_buf(), yi_runtime::Wall::default())
            .with_session("main", Arc::clone(&store));
    let cited: Vec<&str> = summary
        .split_whitespace()
        .filter_map(|word| word.strip_suffix(':').or(Some(word)))
        .filter(|word| word.starts_with("user://"))
        .collect();
    assert_eq!(cited, ["user://2", "user://1"], "{summary}");
    for (address, ask) in cited.iter().zip(asks.iter().rev()) {
        let served = resolver.fetch(&address.parse()?)?;
        assert!(
            served.text.contains(ask),
            "{address} served {:?}",
            served.text
        );
    }
    let window = compaction_window(&store)?;
    let key = yi_context::user_key(&window, &yi_runtime::fetch::user_inputs(&store)?);
    let keyed: Vec<(&str, &str)> = key
        .lines()
        .filter_map(|row| row.split_once(": "))
        .filter(|(address, _)| address.starts_with("user://"))
        .collect();
    assert_eq!(keyed.len(), asks.len(), "{key}");
    for (address, quote) in keyed {
        let served = resolver.fetch(&address.parse()?)?;
        assert!(
            served.text.contains(quote.trim_matches('"')),
            "{address} quoted {quote} but served {:?}",
            served.text
        );
    }
    Ok(())
}

fn compaction_window(
    store: &yi_session::SharedSession,
) -> Result<Vec<AgentMessage>, Box<dyn Error>> {
    let entries = yi_session::lock_session(store).find_entries_on_branch(
        "main",
        &yi_session::EntryQuery {
            order: yi_session::EntryOrder::OldestFirst,
            ..yi_session::EntryQuery::default()
        },
        &yi_session::BranchBounds::default(),
    )?;
    Ok(entries
        .into_iter()
        .filter_map(|entry| match entry {
            Entry::Message { message, .. } => Some(message),
            _ => None,
        })
        .collect())
}

/// Answers each request with the next of `replies`, an OpenAI-style event stream, and hands
/// back every request body it read, in order. It stands in for OpenRouter as the proxy.
fn openrouter_stand_in(
    replies: Vec<String>,
) -> std::io::Result<(u16, std::thread::JoinHandle<Vec<String>>)> {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let handle = std::thread::spawn(move || {
        let mut bodies = Vec::new();
        for reply in replies {
            let Ok((stream, _)) = listener.accept() else {
                break;
            };
            let mut reader = BufReader::new(stream);
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                    break;
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; length];
            let _ = reader.read_exact(&mut body);
            // A measured reply counts its prompt as the provider would: the whole body, bytes/3.
            let reply = reply.replace("\"MEASURED\"", &body.len().div_ceil(3).to_string());
            bodies.push(String::from_utf8_lossy(&body).into_owned());
            let _ = write!(
                reader.into_inner(),
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                reply.len()
            );
        }
        bodies
    });
    Ok((port, handle))
}

/// One streamed reply: `delta` then a finish chunk that carries usage, so nothing settles late.
pub(crate) fn sse_reply(delta: &serde_json::Value, finish: &str) -> String {
    sse_reply_after(delta, finish, 10)
}

/// `sse_reply` for a request whose prompt was `prompt_tokens` long.
fn sse_reply_after(delta: &serde_json::Value, finish: &str, prompt_tokens: u64) -> String {
    let chunks = [
        serde_json::json!({"choices": [{"index": 0, "delta": delta}]}),
        serde_json::json!({"choices": [{"index": 0, "delta": {}, "finish_reason": finish}],
            "usage": {"prompt_tokens": prompt_tokens, "completion_tokens": 5,
                "total_tokens": prompt_tokens + 5}}),
    ];
    let mut text: String = chunks
        .iter()
        .map(|chunk| format!("data: {chunk}\n\n"))
        .collect();
    text.push_str("data: [DONE]\n\n");
    text
}

/// A text reply whose usage the stand-in fills in from the request it answers.
fn sse_reply_measured(content: &str) -> String {
    let chunks = [
        serde_json::json!({"choices": [{"index": 0, "delta": {"content": content}}]}),
        serde_json::json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": "MEASURED", "completion_tokens": content.len().div_ceil(3)}}),
    ];
    let mut text: String = chunks
        .iter()
        .map(|chunk| format!("data: {chunk}\n\n"))
        .collect();
    text.push_str("data: [DONE]\n\n");
    text
}

/// One Anthropic Messages event stream holding `block`, a text or a `tool_use` block.
fn anthropic_sse(block: &serde_json::Value, stop: &str) -> String {
    let (start, delta) = match block["type"].as_str() {
        Some("text") => (
            serde_json::json!({"type": "text", "text": ""}),
            serde_json::json!({"type": "text_delta", "text": block["text"]}),
        ),
        _ => (
            serde_json::json!({"type": "tool_use", "id": block["id"], "name": block["name"],
                "input": {}}),
            serde_json::json!({"type": "input_json_delta", "partial_json": "{}"}),
        ),
    };
    [
        serde_json::json!({"type": "message_start", "message": {"id": "msg_1",
            "model": "claude-haiku-4-5", "usage": {"input_tokens": 10, "output_tokens": 1}}}),
        serde_json::json!({"type": "content_block_start", "index": 0, "content_block": start}),
        serde_json::json!({"type": "content_block_delta", "index": 0, "delta": delta}),
        serde_json::json!({"type": "content_block_stop", "index": 0}),
        serde_json::json!({"type": "message_delta", "delta": {"stop_reason": stop},
            "usage": {"output_tokens": 5}}),
        serde_json::json!({"type": "message_stop"}),
    ]
    .iter()
    .map(|event| {
        format!(
            "event: {}\ndata: {event}\n\n",
            event["type"].as_str().unwrap_or("")
        )
    })
    .collect()
}

/// A tool whose result is long enough to make the next boundary compact.
struct LongResult;

impl yi_loop::AgentTool for LongResult {
    fn definition(&self) -> yi_types::model::ToolDef {
        yi_types::model::ToolDef {
            name: "probe".to_owned(),
            description: "Reads the probe.".to_owned(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            freeform: None,
        }
    }

    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        _args: serde_json::Map<String, serde_json::Value>,
        _signal: &'a yi_loop::interrupt::InterruptSignal,
    ) -> yi_loop::tool::ToolFuture<'a> {
        Box::pin(async {
            yi_loop::tool::ToolOutcome {
                result: yi_types::event::ToolResult {
                    content: vec![yi_types::message::Content::Text {
                        text: "probe line\n".repeat(400),
                        text_signature: None,
                    }],
                    details: serde_json::json!({}),
                    usage: None,
                    added_tool_names: None,
                    terminate: None,
                },
                is_error: false,
            }
        })
    }
}

/// A Claude route on OpenRouter with reasoning, a tool table, and a proxy to the stand-in. A cut
/// needs a reply inside `keep_recent`: a single message over it keeps everything.
fn openrouter_session(port: u16, keep_recent: u64) -> Result<AgentSession, Box<dyn Error>> {
    let settings = Settings {
        keep_recent_tokens: Tokens(keep_recent),
        ..tight_settings()
    };
    openrouter_session_with(port, 2_000, settings)
}

fn openrouter_session_with(
    port: u16,
    window: u64,
    settings: Settings,
) -> Result<AgentSession, Box<dyn Error>> {
    let mut model = faux_model(window);
    model.id = "anthropic/claude-haiku-4.5".to_owned();
    model.api = "openai-completions".to_owned();
    model.provider = "openrouter".to_owned();
    model.base_url = "http://openrouter.ai.invalid/api/v1".to_owned();
    model.reasoning = true;
    let provider = ProviderStream::new(None)
        .with_auth(
            "openrouter",
            yi_runtime::auth::Resolved {
                secret: yi_runtime::auth::Secret::new("sk-test".to_owned()),
                kind: yi_runtime::auth::AuthKind::ApiKey,
                org: None,
                expires: None,
                headers: Vec::new(),
            },
        )
        .with_proxy(yi_ai::request::ProxyConfig::from_values(
            Some(&format!("http://127.0.0.1:{port}")),
            None,
            None,
        )?);
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model,
            thinking_level: Some(yi_types::model::Effort::Medium),
            tool_execution: ExecutionMode::Sequential,
        },
        Arc::new(provider),
    );
    session.set_tools(vec![Arc::new(LongResult)]);
    session.enable_compaction_with(settings);
    Ok(session)
}

/// Indexes of the body messages that carry a breakpoint.
fn marked(body: &serde_json::Value) -> Vec<usize> {
    body["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter(|(_, message)| {
            message["content"]
                .as_array()
                .is_some_and(|parts| parts.iter().any(|part| part["cache_control"].is_object()))
        })
        .map(|(index, _)| index)
        .collect()
}

/// The body's messages through `last`, with every breakpoint taken off: a mark moves no byte
/// of the prompt it sits on. Nothing else is touched, so a string beside a part list differs.
fn unmarked_through(body: &serde_json::Value, last: usize) -> Option<Vec<serde_json::Value>> {
    let mut head = body["messages"].as_array()?.get(..=last)?.to_vec();
    for message in &mut head {
        for part in message["content"].as_array_mut().into_iter().flatten() {
            part.as_object_mut()
                .map(|part| part.remove("cache_control"));
        }
    }
    Some(head)
}

/// #746: the compaction is the loop request before it through that tail, marked only on the
/// system and that tail. OpenRouter merges adjacent user messages, so each shape counts.
fn assert_warm(loop_body: &str, compaction_body: &str) -> Result<(), Box<dyn Error>> {
    let mut before: serde_json::Value = serde_json::from_str(loop_body)?;
    let mut compaction: serde_json::Value = serde_json::from_str(compaction_body)?;
    let tail = *marked(&before)
        .last()
        .ok_or("the loop request marks nothing")?;
    assert_eq!(
        marked(&compaction),
        [0, tail],
        "the compaction must mark the system and the loop's tail, and nothing after: {compaction}"
    );
    assert_eq!(
        unmarked_through(&compaction, tail),
        unmarked_through(&before, tail),
        "the compaction must repeat the loop's messages through its tail"
    );
    let (Some(rest), Some(loop_rest)) = (compaction.as_object_mut(), before.as_object_mut()) else {
        return Err("a body is not an object".into());
    };
    rest.remove("messages");
    loop_rest.remove("messages");
    assert_eq!(
        rest, loop_rest,
        "tools, reasoning and tool choice must be the loop's"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_in_loop_compaction_reads_the_loop_requests_prefix() -> Result<(), Box<dyn Error>> {
    let call = serde_json::json!({"tool_calls": [{"index": 0, "id": "call_1", "type": "function",
        "function": {"name": "probe", "arguments": "{}"}}]});
    let (port, served) = openrouter_stand_in(vec![
        sse_reply(
            &serde_json::json!({"content": "answer ".repeat(460)}),
            "stop",
        ),
        sse_reply(&call, "tool_calls"),
        sse_reply(&serde_json::json!({"content": "## Goal\nProbe"}), "stop"),
        sse_reply(&serde_json::json!({"content": "done"}), "stop"),
    ])?;
    let session = openrouter_session(port, 1_500)?;
    session.prompt("say something")?;
    session.wait_idle().await;
    session.prompt("read the probe")?;
    session.wait_idle().await;
    assert!(
        session
            .messages()
            .iter()
            .any(|message| matches!(message, AgentMessage::CompactionSummary { .. })),
        "the probe's result must compact at the next boundary"
    );
    let bodies = served.join().map_err(|_| "the stand-in panicked")?;
    let [_, before, compaction, _] = bodies.as_slice() else {
        return Err(format!("expected four requests, got {}", bodies.len()).into());
    };
    assert_warm(before, compaction)
}

/// #947: a first request longer than the reserve made compaction due only once the body past
/// it filled window − reserve, so the history overflowed first. It is due while the summary
/// request still fits, and that request reads the loop's cache. The stand-in counts each prompt
/// from its whole body, system prompt and tools included.
#[tokio::test(flavor = "multi_thread")]
async fn compaction_is_due_while_the_warm_summary_still_fits() -> Result<(), Box<dyn Error>> {
    const WINDOW: usize = 6_000;
    let reply = sse_reply_measured(&format!("## Goal\n{}", "answer ".repeat(300)));
    let (port, served) = openrouter_stand_in(vec![reply; 5])?;
    let settings = Settings {
        enabled: true,
        reserve_tokens: Tokens(2_000),
        keep_recent_tokens: Tokens(10),
    };
    let session = openrouter_session_with(port, WINDOW as u64, settings)?;
    for ask in ["ask ".repeat(1_500).as_str(), "go on", "go on", "go on"] {
        session.prompt(ask)?;
        session.wait_idle().await;
    }
    assert!(
        session
            .messages()
            .iter()
            .any(|message| matches!(message, AgentMessage::CompactionSummary { .. })),
        "the fourth prompt leaves room for the summary request only if it compacts first"
    );
    let bodies = served.join().map_err(|_| "the stand-in panicked")?;
    let [_, _, before, compaction, _] = bodies.as_slice() else {
        return Err(format!("expected five requests, got {}", bodies.len()).into());
    };
    for body in &bodies {
        assert!(
            body.len().div_ceil(3) < WINDOW,
            "no request overflows the window: {} tokens",
            body.len().div_ceil(3)
        );
    }
    assert_warm(before, compaction)
}

/// #947: what follows the last reported usage counts at bytes/3, as the rescue charges it. At
/// chars/4 this history sits under window − reserve (110 + 750 of 1,000); at bytes/3 it is over.
#[tokio::test]
async fn text_after_the_last_usage_counts_at_bytes_per_three() -> Result<(), Box<dyn Error>> {
    let mut compactor = yi_runtime::Compactor::new("w".to_owned());
    compactor.settings = tight_settings();
    let messages = vec![
        AgentMessage::user_input(UserContent::Text("ask".to_owned()), 0),
        reply_with_usage("ok", 100, 110),
        AgentMessage::user_input(UserContent::Text("x".repeat(3_000)), 0),
    ];
    let provider = ProviderStream::new(None);
    provider.queue_faux(vec![faux_assistant_message(
        vec![faux_text("## Goal\nSummary")],
        StopReason::Stop,
    )]);
    let replaced = compactor
        .maybe_compact(
            &messages,
            &faux_model(2_000),
            &yi_runtime::compaction::LoopRequest::new("sys".to_owned(), &[], Effort::Off),
            &provider,
            None,
            &yi_loop::interrupt::InterruptSignal::default(),
        )
        .await?;
    assert!(replaced.is_some(), "1,110 of a 1,000-token budget is due");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_idle_compaction_reads_the_last_requests_prefix() -> Result<(), Box<dyn Error>> {
    let (port, served) = openrouter_stand_in(vec![
        sse_reply(
            &serde_json::json!({"content": "answer ".repeat(100)}),
            "stop",
        ),
        sse_reply(&serde_json::json!({"content": "## Goal\nProbe"}), "stop"),
    ])?;
    let session = openrouter_session(port, 10)?;
    session.prompt("say something")?;
    session.wait_idle().await;
    assert!(
        session
            .compact_now()
            .await
            .is_ok_and(|outcome| outcome.applied()),
        "a scheduled compaction applies when idle"
    );
    let bodies = served.join().map_err(|_| "the stand-in panicked")?;
    let [first, compaction] = bodies.as_slice() else {
        return Err(format!("expected two requests, got {}", bodies.len()).into());
    };
    assert_warm(first, compaction)
}

/// A reply that is a tool call alone carries no summary: the trimmed retry goes cold, with no
/// tools to call, so it answers in text and the history is never replaced by a blank summary.
#[tokio::test(flavor = "multi_thread")]
async fn a_call_instead_of_a_summary_retries_without_tools() -> Result<(), Box<dyn Error>> {
    let call = serde_json::json!({"tool_calls": [{"index": 0, "id": "call_1", "type": "function",
        "function": {"name": "probe", "arguments": "{}"}}]});
    let (port, served) = openrouter_stand_in(vec![
        sse_reply(
            &serde_json::json!({"content": "answer ".repeat(100)}),
            "stop",
        ),
        sse_reply(&call, "tool_calls"),
        sse_reply(&serde_json::json!({"content": "## Goal\nProbe"}), "stop"),
    ])?;
    let session = openrouter_session(port, 10)?;
    session.prompt("say something")?;
    session.wait_idle().await;
    assert!(
        session
            .compact_now()
            .await
            .is_ok_and(|outcome| outcome.applied()),
        "the retry's summary applies"
    );
    let bodies = served.join().map_err(|_| "the stand-in panicked")?;
    let [_, warm, retry] = bodies.as_slice() else {
        return Err(format!("expected three requests, got {}", bodies.len()).into());
    };
    let (warm, retry): (serde_json::Value, serde_json::Value) =
        (serde_json::from_str(warm)?, serde_json::from_str(retry)?);
    assert!(
        warm["tools"].is_array(),
        "the first request is the warm one: {warm}"
    );
    assert!(
        retry.get("tools").is_none(),
        "the retry offers no tool: {retry}"
    );
    assert_eq!(marked(&retry), [0], "a cold retry marks the system alone");
    let summary = session
        .messages()
        .into_iter()
        .find_map(|message| match message {
            AgentMessage::CompactionSummary { summary, .. } => Some(summary),
            _ => None,
        });
    assert!(
        summary
            .as_deref()
            .is_some_and(|text| text.contains("## Goal\nProbe")),
        "{summary:?}"
    );
    Ok(())
}

/// #948: Anthropic refuses `tool_use` and `tool_result` blocks in a request that defines no
/// tools. A cold attempt sends none, so its history must hold no tool block either.
#[tokio::test(flavor = "multi_thread")]
async fn a_cold_attempt_on_anthropic_sends_no_tool_block_without_tools()
-> Result<(), Box<dyn Error>> {
    let call = serde_json::json!({"type": "tool_use", "id": "toolu_1", "name": "probe"});
    let summary = serde_json::json!({"type": "text", "text": "## Goal\nProbe"});
    let (port, served) = openrouter_stand_in(vec![
        anthropic_sse(&call, "tool_use"),
        anthropic_sse(&summary, "end_turn"),
    ])?;
    let mut model = faux_model(200_000);
    model.id = "claude-haiku-4-5".to_owned();
    model.api = "anthropic-messages".to_owned();
    model.provider = "anthropic".to_owned();
    model.base_url = "http://api.anthropic.invalid".to_owned();
    let provider = ProviderStream::new(None)
        .with_auth(
            "anthropic",
            yi_runtime::auth::Resolved {
                secret: yi_runtime::auth::Secret::new("sk-ant-test".to_owned()),
                kind: yi_runtime::auth::AuthKind::ApiKey,
                org: None,
                expires: None,
                headers: Vec::new(),
            },
        )
        .with_proxy(yi_ai::request::ProxyConfig::from_values(
            Some(&format!("http://127.0.0.1:{port}")),
            None,
            None,
        )?);
    let mut compactor = yi_runtime::Compactor::new("w".to_owned());
    compactor.settings = tight_settings();
    compactor.schedule();
    let [step, result] = tool_step("toolu_1");
    let messages = vec![
        AgentMessage::user_input(UserContent::Text("read the probe".to_owned()), 0),
        step,
        result,
        reply_with_usage("done", 900, 950),
    ];
    let tools: Vec<Arc<dyn yi_loop::AgentTool>> = vec![Arc::new(LongResult)];
    let replaced = compactor
        .maybe_compact(
            &messages,
            &model,
            &yi_runtime::compaction::LoopRequest::new("sys".to_owned(), &tools, Effort::Off),
            &provider,
            None,
            &yi_loop::interrupt::InterruptSignal::default(),
        )
        .await?;
    assert!(replaced.is_some(), "the cold attempt's summary applies");
    let bodies = served.join().map_err(|_| "the stand-in panicked")?;
    let [warm, cold] = bodies.as_slice() else {
        return Err(format!("expected two requests, got {}", bodies.len()).into());
    };
    for body in [warm, cold] {
        let body: serde_json::Value = serde_json::from_str(body)?;
        let blocks = body["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|message| message["content"].as_array().into_iter().flatten());
        let tool_blocks = blocks
            .filter(|block| matches!(block["type"].as_str(), Some("tool_use" | "tool_result")))
            .count();
        assert!(
            tool_blocks == 0 || body["tools"].is_array(),
            "{tool_blocks} tool blocks and no tools: {body}"
        );
    }
    let cold: serde_json::Value = serde_json::from_str(cold)?;
    let sent = cold["messages"].to_string();
    assert!(
        cold.get("tools").is_none() && sent.contains("probe()") && sent.contains("result toolu_1"),
        "the cold attempt is text, and still carries the call and its result: {cold}"
    );
    Ok(())
}

fn failed_reply(error: &str) -> AgentMessage {
    let mut message = faux_assistant_message(Vec::new(), StopReason::Error);
    if let AgentMessage::Assistant { error_message, .. } = &mut message {
        *error_message = Some(error.to_owned());
    }
    message
}

/// #871: a summarizer that fails twice (an error, then a blank reply) leaves the history as it
/// was, returns the reason at once, keeps `/compact`'s instructions, and queues one notice per
/// failure; after a failure in the loop no later boundary of that run asks again (#946 F1).
#[tokio::test]
async fn two_failed_summaries_leave_the_history_uncompacted() -> Result<(), Box<dyn Error>> {
    let root = Scratch::new("yi-compact-unsummarized")?;
    let mut repo = JsonlRepo::new(root.to_path_buf(), "/tmp/yi-compact-test");
    let store = repo.create(CreateOptions {
        id: Some("unsummarized".to_owned()),
        ..CreateOptions::default()
    })?;
    let provider = Arc::new(ProviderStream::new(None));
    let blank = || faux_assistant_message(vec![faux_text(" \n")], StopReason::Stop);
    // A call to a tool the session lacks: a short error result, and a boundary still due.
    let call = || {
        let mut message = reply_with_usage("", 100, 1_500);
        if let AgentMessage::Assistant {
            content,
            stop_reason,
            ..
        } = &mut message
        {
            *content = vec![yi_ai::faux::faux_tool_call(
                "c",
                "probe",
                serde_json::Map::new(),
            )];
            *stop_reason = StopReason::ToolUse;
        }
        message
    };
    // Due (past window − reserve) but not over the window: no last resort.
    provider.queue_faux(vec![
        reply_with_usage(&format!("long body {}", "y".repeat(400)), 100, 1_500),
        failed_reply("upstream 503"),
        blank(),
        blank(),
        failed_reply("upstream 503"),
        blank(),
        failed_reply("upstream 529"),
        call(),
        call(),
        reply_with_usage("after the notices", 50, 300),
    ]);
    let session = session_for_compaction(Arc::clone(&provider));
    session.attach_store(Arc::clone(&store))?;
    session.prompt("please do the thing with sufficient text here")?;
    session.wait_idle().await;
    let before = session.messages();

    let compactor = session.compactor().ok_or("compaction is on")?;
    compactor.schedule_with_instructions(Some("keep the probe".to_owned()));
    let first = session.compact_now().await;
    let second = session.compact_now().await;
    assert!(
        matches!(&first, Err(error) if error.to_string()
            == "compaction failed: the summarizer returned no text; history left uncompacted, the next prompt retries"),
        "the reason comes back at once: {first:?}"
    );
    assert!(
        matches!(&second, Err(error) if error.to_string().contains("upstream 503")),
        "{second:?}"
    );
    assert_eq!(session.messages(), before, "the idle history stays whole");
    assert_eq!(
        compactor.pending_directive().as_deref(),
        Some("keep the probe"),
        "a failed /compact keeps its instructions for the retry"
    );
    session.prompt("next ask")?;
    session.wait_idle().await;

    assert!(compaction_entries(&store).is_empty(), "nothing was saved");
    let after = session.messages();
    assert_eq!(after.get(..before.len()), Some(before.as_slice()));
    let texts: Vec<String> = after[before.len()..]
        .iter()
        .map(|message| match message {
            AgentMessage::User {
                content: UserContent::Text(text),
                ..
            }
            | AgentMessage::Custom {
                content: UserContent::Text(text),
                ..
            } => text.clone(),
            AgentMessage::Assistant { content, .. } => {
                let text = yi_types::message::join_text(content, "");
                if text.is_empty() {
                    "call".to_owned()
                } else {
                    text
                }
            }
            AgentMessage::ToolResult { .. } => "result".to_owned(),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(
        texts,
        [
            "next ask",
            "[compaction failed: upstream 503; history left uncompacted, the next prompt retries]",
            "[compaction failed: upstream 529; history left uncompacted, the next prompt retries]",
            "call",
            "result",
            "call",
            "result",
            "after the notices",
        ],
        "the second idle notice replaced the first; the run's own failure cost two requests \
         and one notice, and its later boundaries asked for no summary"
    );
    Ok(())
}

/// #946 F2: when both summaries fail and the history no longer fits the window, the oldest
/// turns leave the view so the next request fits; the ledger keeps every entry it had.
#[tokio::test(flavor = "multi_thread")]
async fn an_over_window_history_sheds_its_oldest_turns_when_no_summary_comes()
-> Result<(), Box<dyn Error>> {
    let root = Scratch::new("yi-compact-elide")?;
    let mut repo = JsonlRepo::new(root.to_path_buf(), "/tmp/yi-compact-elide");
    let store = repo.create(CreateOptions {
        id: Some("elide".to_owned()),
        ..CreateOptions::default()
    })?;
    let answer = "answer ".repeat(1_200);
    let blank = sse_reply(&serde_json::json!({"content": " "}), "stop");
    // The first reply's usage is the window's prefix; the second's is past the whole window.
    let (port, served) = openrouter_stand_in(vec![
        sse_reply(&serde_json::json!({"content": "hi"}), "stop"),
        sse_reply_after(&serde_json::json!({"content": answer}), "stop", 2_200),
        blank.clone(),
        blank,
        sse_reply(&serde_json::json!({"content": "done"}), "stop"),
    ])?;
    let session = openrouter_session(port, 10)?;
    session.attach_store(Arc::clone(&store))?;
    for ask in ["say something", "say more"] {
        session.prompt(ask)?;
        session.wait_idle().await;
    }
    let entries = |store: &yi_session::SharedSession| {
        yi_session::lock_session(store).find_entries(&yi_session::EntryQuery::default())
    };
    let ledger_before = entries(&store)?;
    session.prompt("next ask")?;
    session.wait_idle().await;

    let bodies = served.join().map_err(|_| "the stand-in panicked")?;
    let [.., next] = bodies.as_slice() else {
        return Err("no request".into());
    };
    let [_, _, first, second, _] = bodies.as_slice() else {
        return Err(format!("expected five requests, got {}", bodies.len()).into());
    };
    // Past the window the warm request cannot go out: both attempts are cold, the second shorter.
    let (first, second): (serde_json::Value, serde_json::Value) =
        (serde_json::from_str(first)?, serde_json::from_str(second)?);
    assert!(
        first.get("tools").is_none() && second.get("tools").is_none(),
        "two cold attempts: {first}"
    );
    assert!(second["messages"].to_string().len() < first["messages"].to_string().len());
    let body: serde_json::Value = serde_json::from_str(next)?;
    let sent = body["messages"].to_string();
    assert!(
        sent.len() / 4 < 2_000 && !sent.contains("answer answer"),
        "the request after the failed summaries fits the 2,000-token window: {sent}"
    );
    assert!(
        sent.contains("[compaction elided 3 earlier messages from the model's view: summary failed; the transcript keeps them]") && sent.contains("next ask"),
        "{sent}"
    );
    // One text in three places: the summary's line, the marker where they sat, the notice.
    assert_eq!(
        sent.matches("[compaction elided 3 earlier messages from the model's view: summary failed; the transcript keeps them]").count(),
        3,
        "the user and the model are told, on the same request: {sent}"
    );
    let ledger = entries(&store)?;
    assert!(
        ledger_before.iter().all(|entry| ledger.contains(entry)),
        "the ledger keeps every entry"
    );
    assert!(
        compaction_entries(&store)
            .iter()
            .any(|entry| matches!(entry,
            Entry::Compaction { summary, .. } if summary.starts_with("<yi_compact_view>")
                && summary.contains("[compaction elided 3 earlier messages from the model's view: summary failed; the transcript keeps them]"))),
        "the elided range is a compaction entry that keeps D115's recall view"
    );
    Ok(())
}

/// #946 N3: past the window in a one-prompt tool loop, both cold attempts start at a cut point:
/// the quarter and the half by message count both land on a tool result here, which no
/// provider takes without its call.
#[tokio::test(flavor = "multi_thread")]
async fn cold_attempts_in_a_tool_loop_never_open_on_a_tool_result() -> Result<(), Box<dyn Error>> {
    let call = |id: usize| {
        serde_json::json!({"tool_calls": [{"index": 0, "id": format!("call_{id}"),
            "type": "function", "function": {"name": "gone", "arguments": format!("{{\"n\":{id}}}")}}]})
    };
    let blank = sse_reply(&serde_json::json!({"content": " "}), "stop");
    let mut replies = vec![sse_reply(&serde_json::json!({"content": "hi"}), "stop")];
    replies.extend((1..7).map(|id| sse_reply(&call(id), "tool_calls")));
    replies.push(sse_reply_after(&call(7), "tool_calls", 2_200));
    replies.extend([blank.clone(), blank]);
    replies.push(sse_reply(&serde_json::json!({"content": "done"}), "stop"));
    let (port, served) = openrouter_stand_in(replies)?;
    let session = openrouter_session(port, 30)?;
    for ask in ["say something", "call the missing tool seven times"] {
        session.prompt(ask)?;
        session.wait_idle().await;
    }
    let bodies = served.join().map_err(|_| "the stand-in panicked")?;
    let [.., first, second, _] = bodies.as_slice() else {
        return Err("too few requests".into());
    };
    assert_eq!(
        bodies.len(),
        11,
        "seven calls, two cold attempts, the rescued request"
    );
    for body in [first, second] {
        let body: serde_json::Value = serde_json::from_str(body)?;
        assert!(body.get("tools").is_none(), "a cold attempt: {body}");
        let mut called = std::collections::HashSet::new();
        for message in body["messages"].as_array().into_iter().flatten() {
            for call in message["tool_calls"].as_array().into_iter().flatten() {
                called.insert(call["id"].to_string());
            }
            if message["role"] == "tool" {
                assert!(
                    called.contains(&message["tool_call_id"].to_string()),
                    "an orphaned tool result: {body}"
                );
            }
        }
    }
    Ok(())
}

/// #946 Q1, Q2: the session records what opened the work in flight. A rescue in a second run
/// keeps that run's prompt, and one in a third run keeps the follow-up it took mid-run rather
/// than the prompt it began with.
#[tokio::test(flavor = "multi_thread")]
async fn a_rescue_keeps_the_prompt_or_follow_up_the_run_is_working_on() -> Result<(), Box<dyn Error>>
{
    let pad = "p".repeat(400);
    let call = |id: usize| {
        let arguments = serde_json::json!({"n": id, "pad": pad}).to_string();
        serde_json::json!({"tool_calls": [{"index": 0, "id": format!("call_{id}"),
            "type": "function", "function": {"name": "gone", "arguments": arguments}}]})
    };
    let text = |content: &str| sse_reply(&serde_json::json!({"content": content}), "stop");
    let blank = text(" ");
    let calls = |base: usize| {
        let mut replies: Vec<String> = (1..7)
            .map(|id| sse_reply(&call(base + id), "tool_calls"))
            .collect();
        replies.push(sse_reply_after(&call(base + 7), "tool_calls", 2_200));
        replies.extend([blank.clone(), blank.clone(), text("done")]);
        replies
    };
    let mut replies = vec![text("ok")];
    replies.extend(calls(0));
    replies.push(text("ok"));
    replies.extend(calls(10));
    let (port, served) = openrouter_stand_in(replies)?;
    let session = openrouter_session(port, 30)?;
    session.prompt("FIRST: say something")?;
    session.wait_idle().await;
    session.prompt("SECOND TASK: call the missing tool")?;
    session.wait_idle().await;
    session.prompt("THIRD: say ok")?;
    session.follow_up("THE FOLLOW-UP: call it again");
    session.wait_idle().await;
    let bodies = served.join().map_err(|_| "the stand-in panicked")?;
    assert_eq!(bodies.len(), 22, "one reply, two rescued runs");
    let (second, third) = (&bodies[10], &bodies[21]);
    assert!(
        second.contains("SECOND TASK") && second.contains("elided"),
        "the second run's rescue keeps its prompt: {second}"
    );
    assert!(
        third.contains("THE FOLLOW-UP") && third.contains("elided"),
        "the third run's rescue keeps the follow-up it works on: {third}"
    );
    Ok(())
}

/// Runs one compaction whose two summaries come back blank, on a 2,000-token window with a
/// 1,000-token reserve and no tools; `opening` is the message the run began with.
async fn rescue(
    messages: &[AgentMessage],
    system: &str,
    opening: Option<&AgentMessage>,
) -> Result<Option<yi_runtime::compaction::Replacement>, yi_runtime::compaction::CompactError> {
    let mut compactor = yi_runtime::Compactor::new("w".to_owned());
    compactor.settings = Settings {
        enabled: true,
        reserve_tokens: Tokens(1_000),
        keep_recent_tokens: Tokens(500),
    };
    if let Some(opening) = opening {
        compactor.open_run(opening);
    }
    let provider = ProviderStream::new(None);
    let blank = || faux_assistant_message(vec![faux_text(" ")], StopReason::Stop);
    provider.queue_faux(vec![blank(), blank()]);
    compactor
        .maybe_compact(
            messages,
            &faux_model(2_000),
            &yi_runtime::compaction::LoopRequest::new(system.to_owned(), &[], Effort::Off),
            &provider,
            None,
            &yi_loop::interrupt::InterruptSignal::default(),
        )
        .await
}

/// What the next request sends, charged as the rescue charges it: bytes/3.
fn sent_tokens(messages: &[AgentMessage], system: &str) -> u64 {
    let body: u64 = messages
        .iter()
        .map(|message| (yi_context::estimate_message(message).0 * 4).div_ceil(3))
        .sum();
    body + u64::try_from(system.len()).unwrap_or(u64::MAX).div_ceil(3)
}

fn text_of(message: Option<&AgentMessage>) -> Option<&str> {
    match message {
        Some(
            AgentMessage::User {
                content: UserContent::Text(text),
                ..
            }
            | AgentMessage::Custom {
                content: UserContent::Text(text),
                ..
            },
        ) => Some(text),
        _ => None,
    }
}

/// One tool step: a call with some reasoning text, the reply whose usage says the request
/// filled the window, and its result.
fn tool_step(id: &str) -> [AgentMessage; 2] {
    let mut call = reply_with_usage("", 1_900, 2_100);
    if let AgentMessage::Assistant {
        content,
        stop_reason,
        ..
    } = &mut call
    {
        *content = vec![
            faux_text(&format!("step {id}: {}", "reading on. ".repeat(80))),
            yi_ai::faux::faux_tool_call(id, "probe", serde_json::Map::new()),
        ];
        *stop_reason = StopReason::ToolUse;
    }
    let result = AgentMessage::ToolResult {
        tool_call_id: id.to_owned(),
        tool_name: "probe".to_owned(),
        content: vec![faux_text(&format!("result {id} {}", "x".repeat(40)))],
        details: None,
        usage: None,
        added_tool_names: None,
        is_error: false,
        timestamp: 0,
    };
    [call, result]
}

fn every_result_has_its_call(messages: &[AgentMessage]) -> bool {
    let mut called = std::collections::HashSet::new();
    messages.iter().all(|message| match message {
        AgentMessage::Assistant { content, .. } => {
            for part in content {
                if let yi_types::message::Content::ToolCall { id, .. } = part {
                    called.insert(id.clone());
                }
            }
            true
        }
        AgentMessage::ToolResult { tool_call_id, .. } => called.contains(tool_call_id),
        _ => true,
    })
}

/// #946 R1, R2, R6: one prompt then more tool steps than fit. The prompt stays, a marker after
/// it counts what went, every kept result keeps its call, and the request fits with the system
/// prompt counted; one more step would not have fit.
#[tokio::test]
async fn a_rescue_keeps_the_prompt_of_a_long_turn_and_whole_tool_steps()
-> Result<(), Box<dyn Error>> {
    let system = "s".repeat(1_200);
    let prompt =
        AgentMessage::user_input(UserContent::Text("THE TASK: fix the parser".to_owned()), 0);
    let mut messages = vec![prompt.clone()];
    for step in 0..12 {
        messages.extend(tool_step(&format!("c{step}")));
    }
    let replaced = rescue(&messages, &system, Some(&prompt))
        .await?
        .ok_or("no rescue")?;
    let kept = &replaced.messages;
    let gone = replaced.elision.ok_or("no elision")?.messages;
    assert_eq!(text_of(kept.get(1)), Some("THE TASK: fix the parser"));
    assert_eq!(
        text_of(kept.get(2)),
        Some(format!("[compaction elided {gone} earlier messages from the model's view: summary failed; the transcript keeps them]").as_str())
    );
    assert!(matches!(kept.get(3), Some(AgentMessage::Assistant { .. })));
    assert!(every_result_has_its_call(kept), "{kept:#?}");
    let sent = sent_tokens(kept, &system);
    let step = sent_tokens(&tool_step("c0"), "");
    assert!(
        sent <= 1_000,
        "the rescue request fits window − reserve: {sent}"
    );
    assert!(
        sent + step > 900,
        "one more step would have fit: {sent} + {step}"
    );
    Ok(())
}

/// #946 R2, N2: after an earlier compaction, the rescue carries the
/// earlier summary, keeps the first ask and the run's opening, and drops no more than needed.
#[tokio::test]
async fn a_rescue_after_a_compaction_carries_its_summary_and_drops_no_more_than_needed()
-> Result<(), Box<dyn Error>> {
    let system = "s".repeat(1_200);
    let ask = |text: &str| AgentMessage::user_input(UserContent::Text(text.to_owned()), 0);
    let long = || faux_assistant_message(vec![faux_text(&"said. ".repeat(160))], StopReason::Stop);
    let third = ask("third ask");
    let messages = vec![
        AgentMessage::CompactionSummary {
            summary:
                "<yi_compact_view>\nold view\n</yi_compact_view>\n\n## Goal\nThe earlier window"
                    .to_owned(),
            tokens_before: 5_000,
            timestamp: 0,
        },
        ask("first ask"),
        long(),
        AgentMessage::host_note(
            yi_runtime::compaction::COMPACTION_NOTICE,
            "[a notice]".to_owned(),
            0,
        ),
        ask("second ask"),
        long(),
        long(),
        third.clone(),
        reply_with_usage("done", 1_900, 2_100),
    ];
    let replaced = rescue(&messages, &system, Some(&third))
        .await?
        .ok_or("no rescue")?;
    let kept = &replaced.messages;
    let gone = replaced.elision.ok_or("no elision")?.messages;
    let Some(AgentMessage::CompactionSummary { summary, .. }) = kept.first() else {
        return Err("no summary".into());
    };
    assert!(
        summary.contains("## Goal\nThe earlier window")
            && !summary.contains("old view")
            && summary.contains(&format!("[compaction elided {gone} earlier messages")),
        "the earlier summary is carried, its old view rendered anew: {summary}"
    );
    assert_eq!(text_of(kept.get(1)), Some("first ask"));
    assert!(text_of(kept.get(2)).is_some_and(|text| text.contains("summary failed")));
    let sent = sent_tokens(kept, &system);
    let next = sent_tokens(&messages[6..7], "");
    assert!(sent <= 1_000, "{sent}");
    assert!(
        sent + next > 900,
        "the newest dropped message had to go: {sent} + {next}"
    );
    assert!(
        kept.contains(&third),
        "the run's opening is kept: {kept:#?}"
    );
    Ok(())
}

/// #946 N1: a child has no typed message. Its brief, the first message, survives a second
/// rescue; a queued notice or an earlier marker never stands in for it.
#[tokio::test]
async fn a_childs_second_rescue_keeps_its_brief() -> Result<(), Box<dyn Error>> {
    let brief = AgentMessage::host_user(UserContent::Text("BRIEF: map the parser".to_owned()), 0);
    let note = |text: &str| {
        AgentMessage::host_note(
            yi_runtime::compaction::COMPACTION_NOTICE,
            text.to_owned(),
            0,
        )
    };
    let mut messages = vec![
        AgentMessage::CompactionSummary {
            summary: "[compaction elided 4 earlier messages from the model's view: summary failed; the transcript keeps them]".to_owned(),
            tokens_before: 5_000,
            timestamp: 0,
        },
        brief.clone(),
        note("[compaction elided 4 earlier messages from the model's view: summary failed; the transcript keeps them]"),
    ];
    for step in 0..2 {
        messages.extend(tool_step(&format!("a{step}")));
    }
    messages.push(note(
        "[compaction elided 4 earlier messages from the model's view]",
    ));
    for step in 0..12 {
        messages.extend(tool_step(&format!("b{step}")));
    }
    let replaced = rescue(&messages, "sys", Some(&brief))
        .await?
        .ok_or("no rescue")?;
    assert_eq!(
        text_of(replaced.messages.get(1)),
        Some("BRIEF: map the parser")
    );
    assert!(every_result_has_its_call(&replaced.messages));
    Ok(())
}

/// #946 N1: a run a host message woke keeps that message, beside the session's first ask.
#[tokio::test]
async fn a_host_woken_run_keeps_its_waking_message() -> Result<(), Box<dyn Error>> {
    let wake = AgentMessage::host_user(UserContent::Text("SCHEDULED WAKE: check CI".to_owned()), 0);
    let mut messages = vec![
        AgentMessage::user_input(UserContent::Text("first ask".to_owned()), 0),
        faux_assistant_message(vec![faux_text("ok")], StopReason::Stop),
        wake.clone(),
    ];
    for step in 0..12 {
        messages.extend(tool_step(&format!("c{step}")));
    }
    let replaced = rescue(&messages, "sys", Some(&wake))
        .await?
        .ok_or("no rescue")?;
    let texts: Vec<Option<&str>> = replaced
        .messages
        .iter()
        .take(3)
        .map(Some)
        .map(text_of)
        .collect();
    assert_eq!(
        texts[1..],
        [Some("first ask"), Some("SCHEDULED WAKE: check CI")]
    );
    Ok(())
}

/// #946 N2: when only the earlier summary could go, nothing is elided and the failure stands:
/// no `0 earlier messages`, no blank notice.
#[tokio::test]
async fn a_rescue_with_nothing_to_drop_but_the_summary_elides_nothing() -> Result<(), Box<dyn Error>>
{
    let ask = |text: &str| AgentMessage::user_input(UserContent::Text(text.to_owned()), 0);
    let said = "said. ".repeat(170);
    let messages = vec![
        AgentMessage::CompactionSummary {
            summary: "x".repeat(400),
            tokens_before: 9_000,
            timestamp: 0,
        },
        ask("first ask"),
        faux_assistant_message(vec![faux_text(&said)], StopReason::Stop),
        ask("second ask"),
        reply_with_usage(&said, 1_900, 2_100),
    ];
    let outcome = rescue(&messages, "sys", None).await;
    assert!(
        matches!(
            &outcome,
            Err(yi_runtime::compaction::CompactError::NoSummary(_))
        ),
        "{:?}",
        outcome
            .as_ref()
            .map(|replaced| replaced.as_ref().map(|replaced| replaced.elision))
    );
    Ok(())
}

/// #863: the compaction inside the loop drops the late mode fragment and delivers it again;
/// the loop's very next request must carry it, not the one after (nor an extra turn).
#[tokio::test(flavor = "multi_thread")]
async fn an_in_loop_compaction_redelivers_late_slots_on_the_next_request()
-> Result<(), Box<dyn Error>> {
    let dir = Scratch::new("yi-compact-late-slot")?;
    let call = serde_json::json!({"tool_calls": [{"index": 0, "id": "call_1", "type": "function",
        "function": {"name": "probe", "arguments": "{}"}}]});
    let (port, served) = openrouter_stand_in(vec![
        sse_reply(
            &serde_json::json!({"content": "answer ".repeat(460)}),
            "stop",
        ),
        sse_reply(&call, "tool_calls"),
        sse_reply(&serde_json::json!({"content": "## Goal\nProbe"}), "stop"),
        sse_reply(&serde_json::json!({"content": "done"}), "stop"),
    ])?;
    let session = openrouter_session(port, 1_500)?;
    session.install_extensions(yi_runtime::ext::install(yi_runtime::ext::ExtOptions {
        cwd: dir.to_path_buf(),
        home: dir.to_path_buf(),
        mode: yi_runtime::PermissionMode::Auto,
        user_system: String::new(),
        schema_instruction: None,
        context_window: 128_000,
        global_skills: Vec::new(),
    }));
    let broker = yi_runtime::permission::PermissionBroker::new(
        yi_runtime::PermissionMode::Auto,
        dir.to_path_buf(),
        Vec::new(),
        None,
        tokio::sync::broadcast::channel(8).0,
    );
    session.prompt("say something")?;
    session.wait_idle().await;
    broker.set_mode_and_fragment(yi_runtime::PermissionMode::Ask, &session);
    session.prompt("read the probe")?;
    session.wait_idle().await;
    assert!(
        session
            .messages()
            .iter()
            .any(|message| matches!(message, AgentMessage::CompactionSummary { .. })),
        "the probe's result must compact inside the loop"
    );
    let bodies = served.join().map_err(|_| "the stand-in panicked")?;
    let [_, first, _, next] = bodies.as_slice() else {
        return Err(format!("expected four requests, got {}", bodies.len()).into());
    };
    let carries = |body: &str| -> Result<bool, Box<dyn Error>> {
        let body: serde_json::Value = serde_json::from_str(body)?;
        Ok(body["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|message| {
                message["role"] == "user" && message.to_string().contains("Permission mode: ask")
            }))
    };
    assert!(carries(first)?, "the flip rides the run's first request");
    assert!(
        carries(next)?,
        "the request right after the compaction carries the mode again: {next}"
    );
    Ok(())
}

/// #872: a mid-run effort change, then a compaction. The compaction reads the prefix the loop
/// request before it wrote, so it must send that request's reasoning, not the run's first.
#[tokio::test(flavor = "multi_thread")]
async fn an_in_loop_compaction_sends_the_effort_of_the_request_before_it()
-> Result<(), Box<dyn Error>> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let call = serde_json::json!({"tool_calls": [{"index": 0, "id": "call_1", "type": "function",
        "function": {"name": "probe", "arguments": "{}"}}]});
    let (port, served) = openrouter_stand_in(vec![
        sse_reply(&call, "tool_calls"),
        sse_reply(&call, "tool_calls"),
        sse_reply(&serde_json::json!({"content": "## Goal\nProbe"}), "stop"),
        sse_reply(&serde_json::json!({"content": "done"}), "stop"),
    ])?;
    let session = openrouter_session(port, 10)?;
    let mut model = session.model();
    // Only the scheduled compaction may fire: the probe's result stays far under this window.
    model.context_window = 100_000;
    let provider = Arc::clone(session.provider_arc());
    let compactor = Arc::new({
        let mut compactor = yi_runtime::Compactor::new("w".to_owned());
        compactor.settings = Settings {
            keep_recent_tokens: Tokens(1_500),
            ..tight_settings()
        };
        compactor
    });
    let tools: Vec<Arc<dyn yi_loop::AgentTool>> = vec![Arc::new(LongResult)];
    let mut config = yi_loop::LoopConfig::new(model.clone());
    config.effort = Effort::Medium;
    config.convert_to_llm = Box::new(yi_context::convert_to_llm);
    let (system, table) = ("sys".to_owned(), tools.clone());
    config.maybe_compact = Some(yi_runtime::compaction::loop_hook(
        Arc::clone(&compactor),
        Arc::clone(&provider),
        Arc::new(move |effort| {
            yi_runtime::compaction::LoopRequest::new(system.clone(), &table, effort)
        }),
        Arc::new(|| None),
        yi_runtime::compaction::CompactReports {
            waiting: Arc::new(|_| {}),
            compacted: Arc::new(|| {}),
            failed: Arc::new(|_| {}),
            elided: Arc::new(|_| {}),
        },
    ));
    // Turn 1 raises the effort for turn 2; turn 2 asks for a compaction and lowers it again,
    // so the compaction sits between a High request and a Low one.
    let turns = Arc::new(AtomicUsize::new(0));
    let scheduler = Arc::clone(&compactor);
    config.prepare_next_turn = Some(Box::new(move |_| {
        let thinking = match turns.fetch_add(1, Ordering::SeqCst) {
            0 => Effort::High,
            _ => {
                scheduler.schedule();
                Effort::Low
            }
        };
        Some(yi_loop::NextTurn {
            model: None,
            thinking: Some(thinking),
        })
    }));
    // An earlier exchange: the cut needs a turn before the one in flight to summarize.
    let mut context = yi_loop::LoopContext {
        system_prompt: "sys".to_owned(),
        messages: vec![
            AgentMessage::user_input(UserContent::Text("say something".to_owned()), 0),
            faux_assistant_message(vec![faux_text(&"answer ".repeat(460))], StopReason::Stop),
        ],
        tools,
    };
    let signal = yi_loop::interrupt::InterruptSignal::default();
    yi_loop::run_loop(
        &mut context,
        vec![AgentMessage::user_input(
            UserContent::Text("read the probe twice".to_owned()),
            0,
        )],
        &config,
        &signal,
        &mut |_| {},
        provider.as_ref(),
    )
    .await;
    assert!(
        matches!(
            context.messages.first(),
            Some(AgentMessage::CompactionSummary { .. })
        ),
        "the scheduled compaction ran inside the loop"
    );
    let bodies = served.join().map_err(|_| "the stand-in panicked")?;
    let [_, before, compaction, _] = bodies.as_slice() else {
        return Err(format!("expected four requests, got {}", bodies.len()).into());
    };
    assert_warm(before, compaction)
}

/// Host nudges are per-window prompts: a compacted window's `todo_nudge` or `repeat_break` must
/// not ride into the next window's retained tail.
#[test]
fn compaction_drops_the_host_nudges_it_retained() -> Result<(), Box<dyn Error>> {
    // A todo nudge as a real session wrote it on 2026-09-15; it holds host text only.
    let recorded = r#"{"kind":"entry","lane":"main","type":"message","id":"01a0a2e3-e3f2-71a2-9936-bdb725d47ac3","message":{"role":"custom","customType":"todo_nudge","content":"3 changes have landed with no todo list. `init` the list naming what remains, batched with your next call.","display":false,"timestamp":1789439239154},"parentId":"01a0a2e3-e3f1-7e9b-b28b-2d62fc96509f","seq":25,"timestamp":1789439239154}"#;
    let Entry::Message { message: nudge, .. } = serde_json::from_str::<Entry>(recorded)? else {
        return Err("the recorded line is not a message entry".into());
    };
    // The other four under their producers' own type names; "discovery" has no named const.
    let note = |kind: &str| AgentMessage::host_note(kind, "a host nudge".to_owned(), 0);
    let user = AgentMessage::user_input(UserContent::Text("keep me".to_owned()), 0);
    let reply = faux_assistant_message(vec![faux_text("read")], StopReason::Stop);
    let kept = yi_context::drop_internal(&[
        nudge,
        note(yi_loop::REPEAT_BREAK_CUSTOM_TYPE),
        note(yi_loop::LENGTH_REDRIVE_CUSTOM_TYPE),
        note(yi_runtime::spend::SPEND_ALERT_TYPE),
        note("discovery"),
        user.clone(),
        reply.clone(),
    ]);
    assert_eq!(kept, [user, reply.clone()], "only the user's turn survives");
    // A nudge queued after the last reply has not been read: a repeat break or a spend alert is
    // raised once, so dropping it here would lose it for good.
    let unread = note(yi_loop::REPEAT_BREAK_CUSTOM_TYPE);
    let tail = [reply.clone(), unread.clone()];
    assert_eq!(yi_context::drop_internal(&tail), tail);
    Ok(())
}
