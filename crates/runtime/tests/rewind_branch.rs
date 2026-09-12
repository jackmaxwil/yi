//! E1: a rewind used to drop the abandoned attempt on the floor. It now leaves
//! a `BranchSummary` behind, written by the summarizer role and replayed by the
//! same projection every other entry goes through.

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

use std::error::Error;
use std::sync::Arc;

use yi_ai::faux::{faux_assistant_message, faux_text};
use yi_loop::ExecutionMode;
use yi_runtime::{AgentSession, ProviderStream, SessionConfig};
use yi_session::{CreateOptions, JsonlRepo, SessionRepo};
use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, StopReason};
use yi_types::model::{Model, ModelCost};

type TestResult = Result<(), Box<dyn Error>>;

fn faux_model() -> Model {
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
        context_window: 128_000,
        max_tokens: 16_384,
        compat: None,
        thinking_level_map: None,
        headers: None,
    }
}

struct Seeded {
    session: AgentSession,
    store: yi_session::SharedSession,
    provider: Arc<ProviderStream>,
    _root: Scratch,
}

/// Two turns on one lane; the caller rewinds onto the first assistant reply, so
/// the second turn is the abandoned attempt.
async fn seeded(id: &str) -> Result<Seeded, Box<dyn Error>> {
    let root = Scratch::new(&format!("yi-rewind-{id}"))?;
    let mut repo = JsonlRepo::new(root.to_path_buf(), "/tmp/yi-rewind-test");
    let store = repo.create(CreateOptions {
        id: Some(id.to_owned()),
        ..CreateOptions::default()
    })?;
    let provider = Arc::new(ProviderStream::new(None, None));
    provider.queue_faux(vec![
        faux_assistant_message(vec![faux_text("kept reply")], StopReason::Stop),
        faux_assistant_message(
            vec![faux_text("tried the ABANDONED-PATH and it deadlocked")],
            StopReason::Stop,
        ),
    ]);
    let session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        Arc::clone(&provider),
    );
    session.attach_store(Arc::clone(&store))?;
    session.prompt("first ask")?;
    session.wait_idle().await;
    session.prompt("second ask")?;
    session.wait_idle().await;
    Ok(Seeded {
        session,
        store,
        provider,
        _root: root,
    })
}

fn entries(store: &yi_session::SharedSession) -> Vec<Entry> {
    yi_session::lock_session(store)
        .find_entries_on_branch(
            "main",
            &yi_session::EntryQuery {
                order: yi_session::EntryOrder::OldestFirst,
                ..yi_session::EntryQuery::default()
            },
            &yi_session::BranchBounds::default(),
        )
        .unwrap_or_default()
}

/// The last entry of the first turn: rewinding here keeps turn one and abandons
/// turn two.
fn first_reply_id(store: &yi_session::SharedSession) -> Option<String> {
    entries(store)
        .into_iter()
        .find(|entry| {
            matches!(
                entry,
                Entry::Message {
                    message: AgentMessage::Assistant { .. },
                    ..
                }
            )
        })
        .map(|entry| entry.id().to_owned())
}

#[tokio::test]
async fn a_rewind_writes_a_branch_summary_the_projection_replays() -> TestResult {
    let seeded = seeded("rewind-one").await?;
    let before = entries(&seeded.store);
    let old_leaf = before.last().ok_or("seed wrote nothing")?.id().to_owned();
    let target = first_reply_id(&seeded.store).ok_or("no assistant entry to rewind onto")?;

    let rewound = yi_runtime::rewind_to(&seeded.session, &target)?;
    let stub = rewound
        .abandoned
        .ok_or("a rewind past a whole turn must hand back the abandoned span")?;
    assert_eq!(stub.from_id, old_leaf, "the stub names the leaf it left");
    assert!(
        stub.messages.iter().any(|message| matches!(
            message,
            AgentMessage::Assistant { .. } | AgentMessage::User { .. }
        )),
        "the abandoned span must carry the orphaned turn's messages"
    );

    seeded.provider.queue_faux(vec![faux_assistant_message(
        vec![faux_text(
            "Tried ABANDONED-PATH; it deadlocked. Do not retry it.",
        )],
        StopReason::Stop,
    )]);
    yi_runtime::summarize_branch(&seeded.session, stub).await;

    let after = entries(&seeded.store);
    let (from_id, summary) = after
        .iter()
        .find_map(|entry| match entry {
            Entry::BranchSummary {
                from_id, summary, ..
            } => Some((from_id.clone(), summary.clone())),
            _ => None,
        })
        .ok_or("the rewind must leave a BranchSummary entry on the lane")?;
    assert_eq!(
        from_id, old_leaf,
        "the entry must name the leaf the branch left, or a reader cannot place it"
    );
    assert!(summary.contains("ABANDONED-PATH"), "{summary}");

    let projected = yi_context::project(&after);
    assert!(
        projected.iter().any(|message| matches!(
            message,
            AgentMessage::BranchSummary { summary, .. } if summary.contains("ABANDONED-PATH")
        )),
        "the same projection every other entry uses must replay it: {projected:?}"
    );
    assert!(
        seeded
            .session
            .messages()
            .iter()
            .any(|message| matches!(message, AgentMessage::BranchSummary { .. })),
        "an idle session must also see it without a reload"
    );
    Ok(())
}

#[tokio::test]
async fn a_failed_summarizer_never_fails_the_rewind() -> TestResult {
    let seeded = seeded("rewind-two").await?;
    let target = first_reply_id(&seeded.store).ok_or("no assistant entry to rewind onto")?;

    let rewound = yi_runtime::rewind_to(&seeded.session, &target)?;
    assert_eq!(
        rewound.leaf.as_deref(),
        Some(target.as_str()),
        "the lane moves whatever the summarizer later does"
    );
    let stub = rewound.abandoned.ok_or("expected an abandoned span")?;

    // Nothing queued: the faux provider errors, which is the failure the entry
    // must not be forged from.
    yi_runtime::summarize_branch(&seeded.session, stub).await;
    assert!(
        !entries(&seeded.store)
            .iter()
            .any(|entry| matches!(entry, Entry::BranchSummary { .. })),
        "a failed summarizer must write no entry rather than an empty one"
    );
    assert_eq!(
        yi_session::lock_session(&seeded.store).leaf_id("main")?,
        Some(target),
        "the rewind itself stands"
    );
    Ok(())
}

#[tokio::test]
async fn an_empty_abandoned_span_writes_nothing() -> TestResult {
    let seeded = seeded("rewind-three").await?;
    let leaf = entries(&seeded.store)
        .last()
        .ok_or("seed wrote nothing")?
        .id()
        .to_owned();

    let rewound = yi_runtime::rewind_to(&seeded.session, &leaf)?;
    assert!(
        rewound.abandoned.is_none(),
        "rewinding onto the leaf abandons nothing, so no summarizer call is earned"
    );
    Ok(())
}
