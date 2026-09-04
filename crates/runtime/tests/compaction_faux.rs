use std::error::Error;
use std::sync::Arc;

use yi_ai::faux::{faux_assistant_message, faux_text};
use yi_context::{Settings, Tokens};
use yi_loop::ExecutionMode;
use yi_runtime::{AgentSession, ProviderStream, SessionConfig};
use yi_session::{CreateOptions, JsonlRepo, SessionRepo};
use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, StopReason, UserContent};
use yi_types::model::{Model, ModelCost};

fn faux_model(context_window: u64) -> Model {
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
    let root = std::env::temp_dir().join(format!("yi-compact-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut repo = JsonlRepo::new(root.clone(), "/tmp/yi-compact-test");
    let store = repo.create(CreateOptions {
        id: Some("compact-one".to_owned()),
        ..CreateOptions::default()
    })?;

    let provider = Arc::new(ProviderStream::new(None, None));
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
        let provider = Arc::new(ProviderStream::new(None, None));
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
    std::fs::remove_dir_all(&root)?;
    Ok(())
}

#[tokio::test]
async fn compact_now_applies_immediately_when_idle() -> Result<(), Box<dyn Error>> {
    let provider = Arc::new(ProviderStream::new(None, None));
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

/// The recall half of compaction: a turn the keep-recent window drops stays
/// discoverable through `SessionStore::grep` and fetchable through
/// `history://`, and the compact view cites its entry id.
#[tokio::test]
async fn a_compacted_away_turn_stays_greppable_and_fetchable() -> Result<(), Box<dyn Error>> {
    let root = std::env::temp_dir().join(format!("yi-compact-recall-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut repo = JsonlRepo::new(root.clone(), "/tmp/yi-compact-recall");
    let store = repo.create(CreateOptions {
        id: Some("compact-recall".to_owned()),
        ..CreateOptions::default()
    })?;

    // Seed an early turn whose tool result carries a unique needle deep in a
    // long body: the keep-recent window drops it, and the 160-char brief line
    // caps before the needle, so only the entry id survives in the window.
    let needle = "NEEDLE-ZEBRA-4182";
    let long_body = format!("{} {needle} {}", "preamble ".repeat(40), "tail ".repeat(80));
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
                    text: long_body.clone(),
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

    let provider = Arc::new(ProviderStream::new(None, None));
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
    assert_eq!(compaction_entries(&store).len(), 1);

    let live = session.messages();
    let live_text = live
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()?
        .join("\n");
    assert!(
        !live_text.contains(needle),
        "the compacted-away needle must leave the live window: {live_text}"
    );
    assert!(
        live_text.contains(&format!("(#{needle_id})")),
        "the compact view must cite the dropped turn by entry id: {live_text}"
    );

    let hits = yi_session::lock_session(&store).grep(needle, 8);
    assert!(
        hits.iter().any(|hit| hit.entry_id == needle_id),
        "store grep must find the compacted-away needle: {hits:?}"
    );

    let resolver = yi_runtime::fetch::Resolver::new(root.clone(), yi_runtime::Wall::default())
        .with_session_handle("main", session.store_handle());
    let url: yi_types::url::Url = format!("history://main/{needle_id}").parse()?;
    let fetched = resolver.fetch(&url)?;
    assert!(
        fetched.text.contains(&long_body),
        "history:// must serve the full compacted-away blob: {}",
        fetched.text
    );

    drop(session);
    std::fs::remove_dir_all(&root)?;
    Ok(())
}

#[tokio::test]
async fn compaction_below_threshold_is_a_no_op() -> Result<(), Box<dyn Error>> {
    let provider = Arc::new(ProviderStream::new(None, None));
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
        let provider = Arc::new(ProviderStream::new(None, None));
        let signal = yi_loop::interrupt::InterruptSignal::default();
        compactor
            .maybe_compact(
                &messages,
                &faux_model(2_000),
                "sys",
                &provider,
                None,
                &signal,
            )
            .await
            .is_some()
    }

    assert!(
        compacts_after(yi_types::message::Usage::zero()).await,
        "control: a reported zero prefix charges all 1600 tokens against the 1000 budget"
    );
    assert!(
        !compacts_after(yi_types::message::Usage::unknown()).await,
        "an unknown usage must not forge a zero prefix: the reported 1500 leaves 100 charged"
    );
    Ok(())
}
