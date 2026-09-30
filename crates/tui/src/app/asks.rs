use serde_json::Value;
use yi_types::subagent::ChildFlag;

use super::App;
use crate::cell::{Cell, TaskStatus};

#[derive(Debug, Clone)]
pub(crate) struct ReplyTarget {
    pub(crate) child_id: String,
    pub(crate) question: String,
    pub(crate) title: String,
}

impl App {
    pub(crate) fn reply_target(&self) -> Option<ReplyTarget> {
        let id = self.focused.as_ref()?;
        let state = self.tasks.get(id)?;
        let (TaskStatus::Running, Some(ChildFlag::NeedsYou { note })) =
            (state.cell.status, &state.cell.flag)
        else {
            return None;
        };
        let (question, _) = note.strip_prefix("asks ")?.split_once(':')?;
        Some(ReplyTarget {
            child_id: id.clone(),
            question: question.to_owned(),
            title: format!(" reply to {}: {note} ", state.cell.description),
        })
    }

    /// Invariant: a draft answers the question open when it started, whatever opens later.
    pub(crate) fn bind_reply(&mut self) {
        if self.composer.is_empty() {
            self.reply_bound = self.reply_target();
        }
    }

    pub(crate) fn reply_title(&self) -> Option<String> {
        self.reply_bound.as_ref().map(|bound| bound.title.clone())
    }
}

/// A compaction note reads as the host line it always was; any other shown custom message is a
/// callout sourced by its type or its mail envelope.
pub(crate) fn custom_cell(
    custom_type: &str,
    content: &yi_types::message::UserContent,
    details: Option<&Value>,
) -> Cell {
    let text = crate::transcript::user_text(content);
    if custom_type == yi_runtime::compaction::COMPACTION_NOTICE {
        return Cell::Notice { text };
    }
    Cell::Advisory {
        source: mail_source(custom_type, details),
        text,
    }
}

pub(crate) fn mail_source(custom_type: &str, details: Option<&Value>) -> String {
    let field = |key: &str| details.and_then(|details| details.get(key)?.as_str());
    match (custom_type, field("from"), field("to")) {
        ("agent_message" | "human_answer", Some(from), Some(to)) => {
            let kind = field("kind")
                .filter(|kind| *kind != "inform")
                .unwrap_or("mail");
            let from = if field("answeredBy") == Some("human") {
                "you"
            } else {
                from
            };
            format!("{kind} {from} → {to}")
        }
        _ => custom_type.to_owned(),
    }
}
