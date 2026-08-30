use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, Content, UserContent};
use yi_types::model::LlmContext;

use crate::session::{AgentSession, Status};

/// E1: the summarizer reads an abandoned attempt as material, never as work to
/// continue — the P6 framing compaction uses.
const BRANCH_SUMMARY_PROMPT: &str = "The transcript below is an abandoned attempt: the user rewound past it. In at most five sentences, terse and factual, record what was tried, what was learned (findings, errors, dead ends), and any decision that still stands. Do not continue the work and do not give advice.";

/// The entries a rewind orphaned, kept so the branch can be summarized after
/// the lane has already moved.
pub struct BranchStub {
    /// The leaf the lane left.
    pub from_id: String,
    pub messages: Vec<AgentMessage>,
}

pub struct Rewound {
    pub leaf: Option<String>,
    /// Handed back for the composer: landing on a user turn means editing it.
    pub unsent: Option<String>,
    /// `None` when the rewind abandoned nothing (landing on the leaf itself).
    pub abandoned: Option<BranchStub>,
}

/// Selecting a user message rewinds to its *parent*: the point of picking your
/// own message is to stand where you were before you sent it.
pub fn rewind_to(session: &AgentSession, entry_id: &str) -> Result<Rewound, String> {
    let store = session.store().ok_or("no session store")?;
    let entry = yi_session::lock_session(&store)
        .entry(entry_id)
        .ok_or_else(|| format!("no entry {entry_id}"))?;
    let (leaf, unsent) = match &entry {
        Entry::Message {
            message: AgentMessage::User { content, .. },
            parent_id,
            ..
        } => (parent_id.clone(), Some(user_text(content))),
        Entry::Custom {
            custom_type,
            data,
            parent_id,
            ..
        } => (
            parent_id.clone(),
            custom_text(custom_type, data.as_ref()).into(),
        ),
        other => (Some(other.id().to_owned()), None),
    };
    // Collected before the move: afterwards the orphaned span is off the lane
    // and the branch walk can no longer reach it.
    let abandoned = abandoned_span(&store, leaf.as_deref());
    yi_session::lock_session(&store)
        .move_lane("main", leaf.as_deref())
        .map_err(|error| error.to_string())?;
    session
        .attach_store(store)
        .map_err(|error| error.to_string())?;
    Ok(Rewound {
        leaf,
        unsent,
        abandoned,
    })
}

fn abandoned_span(store: &yi_session::SharedSession, new_leaf: Option<&str>) -> Option<BranchStub> {
    let entries = yi_session::lock_session(store)
        .find_entries_on_branch(
            "main",
            &yi_session::EntryQuery {
                order: yi_session::EntryOrder::OldestFirst,
                ..yi_session::EntryQuery::default()
            },
            &yi_session::BranchBounds::default(),
        )
        .ok()?;
    let from_id = entries.last()?.id().to_owned();
    let start = match new_leaf {
        Some(leaf) => entries
            .iter()
            .position(|entry| entry.id() == leaf)?
            .checked_add(1)?,
        None => 0,
    };
    let messages = yi_context::project(entries.get(start..)?);
    (!messages.is_empty()).then_some(BranchStub { from_id, messages })
}

/// Best-effort by design: a rewind that already moved the lane must not be
/// undone by a summarizer that failed, so nothing here returns an error.
pub async fn summarize_branch(session: &AgentSession, stub: BranchStub) {
    let model = session
        .compactor()
        .and_then(|compactor| compactor.summarizer.clone())
        .unwrap_or_else(|| session.model());
    let context = LlmContext {
        system_prompt: BRANCH_SUMMARY_PROMPT.to_owned(),
        messages: vec![AgentMessage::User {
            content: UserContent::Text(yi_context::serialize_conversation(&stub.messages)),
            timestamp: 0,
        }],
        tools: None,
    };
    let signal = yi_loop::interrupt::InterruptSignal::default();
    let Ok(summary) =
        crate::compaction::complete_text(session.provider_arc(), &model, &context, &signal).await
    else {
        return;
    };
    let summary = summary.trim().to_owned();
    if summary.is_empty() {
        return;
    }
    let Some(store) = session.store() else {
        return;
    };
    if yi_session::lock_session(&store)
        .append_branch_summary("main", stub.from_id, summary)
        .is_err()
    {
        return;
    }
    // Mid-turn the in-memory history belongs to the running turn, so the view
    // waits for the next attach; idle, re-projecting is that same attach.
    if session.status() == Status::Idle {
        let _ = session.attach_store(store);
    }
}

fn user_text(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|block| match block {
                Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn custom_text(custom_type: &str, data: Option<&serde_json::Value>) -> String {
    data.and_then(|data| data.get("text"))
        .and_then(serde_json::Value::as_str)
        .map_or_else(|| format!("[{custom_type}]"), str::to_owned)
}
