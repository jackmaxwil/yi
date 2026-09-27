#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
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
    let root = Scratch::new("yi-compact-e2e")?;
    let mut repo = JsonlRepo::new(root.to_path_buf(), "/tmp/yi-compact-test");
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
    let provider = Arc::new(ProviderStream::new(None, None));
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

    let resumed = session_for_compaction(Arc::new(ProviderStream::new(None, None)));
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
            .is_ok_and(|replaced| replaced.is_some())
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

/// Incident: the loop's hook ran before every request and copied the whole history, read
/// the store and assembled the system prompt, only to learn compaction was not due.
#[tokio::test]
async fn a_request_that_is_not_due_assembles_nothing() -> Result<(), Box<dyn Error>> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let mut compactor = yi_runtime::Compactor::new("w".to_owned());
    compactor.settings = tight_settings();
    let asked = Arc::new(AtomicUsize::new(0));
    let (prompts, stores) = (Arc::clone(&asked), Arc::clone(&asked));
    let hook = yi_runtime::compaction::loop_hook(
        Arc::new(compactor),
        Arc::new(ProviderStream::new(None, None)),
        faux_model(2_000),
        Arc::new(move || {
            prompts.fetch_add(1, Ordering::SeqCst);
            "sys".to_owned()
        }),
        Arc::new(move || {
            stores.fetch_add(1, Ordering::SeqCst);
            None
        }),
        Arc::new(|| {}),
        Arc::new(|_| {}),
    );
    let history = [reply_with_usage("small", 10, 20)];
    assert!(hook(&history).await.is_none());
    assert_eq!(asked.load(Ordering::SeqCst), 0);
    Ok(())
}
