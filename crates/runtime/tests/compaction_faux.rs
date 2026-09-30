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

    assert!(session.compact_now().await);
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
    let applied = session.compact_now().await;
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

// A server-observed prefill latches for the whole window, so an unreported
// usage recorded as zero pins the prefix at zero permanently and the first-only
// guard drops every later real observation.
#[tokio::test]
async fn an_unknown_usage_never_pins_the_window_prefill() -> Result<(), Box<dyn Error>> {
    async fn compacts_after(first: yi_types::message::Usage) -> bool {
        let mut compactor = yi_runtime::Compactor::new("win-0".to_owned());
        compactor.settings = tight_settings();
        compactor.on_usage(&first);
        let mut reported = yi_types::message::Usage::zero();
        reported.input = 1_500;
        compactor.on_usage(&reported);
        let messages = vec![
            AgentMessage::host_user(UserContent::Text("ask ".repeat(30)), 0),
            reply_with_usage(&"answer ".repeat(30), 1_500, 1_600),
            AgentMessage::host_user(UserContent::Text("follow up".to_owned()), 0),
        ];
        let provider = Arc::new(ProviderStream::new(None));
        let signal = yi_loop::interrupt::InterruptSignal::default();
        compactor
            .maybe_compact(
                &messages,
                &faux_model(2_000),
                &yi_runtime::compaction::LoopRequest::new("sys".to_owned(), &[], Effort::Off),
                &provider,
                None,
                &signal,
            )
            .await
            .is_ok_and(|replaced| replaced.is_some())
    }

    let mut tiny = yi_types::message::Usage::zero();
    tiny.input = 1;
    assert!(
        compacts_after(tiny).await,
        "control: a reported 1-token prefix latches and charges 1599 against the 1000 budget"
    );
    assert!(
        !compacts_after(yi_types::message::Usage::unknown()).await,
        "an unknown usage must not forge a zero prefix: the reported 1500 leaves 100 charged"
    );
    let mut refused = yi_ai::request::empty_assistant(&faux_model(2_000));
    let _ = yi_ai::request::fail_message(&mut refused, "HTTP 402: insufficient credits");
    let AgentMessage::Assistant { usage, .. } = refused else {
        return Err("not an assistant message".into());
    };
    assert!(
        !compacts_after(usage).await,
        "a refusal's known zero sent no prompt and must not pin the prefix either"
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
        faux_model(2_000),
        Arc::new(move || {
            requests.fetch_add(1, Ordering::SeqCst);
            yi_runtime::compaction::LoopRequest::new("sys".to_owned(), &[], Effort::Off)
        }),
        Arc::new(move || {
            stores.fetch_add(1, Ordering::SeqCst);
            None
        }),
        yi_runtime::compaction::CompactReports {
            waiting: Arc::new(|_| {}),
            compacted: Arc::new(|| {}),
            unsaved: Arc::new(|_| {}),
        },
    );
    let history = [reply_with_usage("small", 10, 20)];
    assert!(hook(&history).await.is_none());
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
    let chunks = [
        serde_json::json!({"choices": [{"index": 0, "delta": delta}]}),
        serde_json::json!({"choices": [{"index": 0, "delta": {}, "finish_reason": finish}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}}),
    ];
    let mut text: String = chunks
        .iter()
        .map(|chunk| format!("data: {chunk}\n\n"))
        .collect();
    text.push_str("data: [DONE]\n\n");
    text
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
    let mut model = faux_model(2_000);
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
    session.enable_compaction_with(Settings {
        keep_recent_tokens: Tokens(keep_recent),
        ..tight_settings()
    });
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
        session.compact_now().await,
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
    assert!(session.compact_now().await, "the retry's summary applies");
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
