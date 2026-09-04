use yi_types::compaction::CompactionDetails;
use yi_types::entry::Entry;
use yi_types::message::AgentMessage;

use crate::account::{Tokens, estimate_context};
use crate::audit::{DROPPED_CAP, identifiers, message_text};
use crate::cut::select_cut;
use crate::details::{FileOps, compute_file_lists, extract_file_ops, format_file_operations};
use crate::floor::{RETENTION_FLOOR_BUDGET, retain_floor};
use crate::policy::Settings;
use crate::project::{project, project_attributed};
use crate::view::{CompiledView, compile_view};

#[derive(Debug, Clone, PartialEq)]
pub struct Preparation {
    pub messages_to_summarize: Vec<AgentMessage>,
    pub turn_prefix_messages: Vec<AgentMessage>,
    pub retained_tail: Vec<AgentMessage>,
    pub is_split_turn: bool,
    pub tokens_before: Tokens,
    pub previous_summary: Option<String>,
    pub file_ops: FileOps,
    pub view: CompiledView,
}

fn previous_compaction(branch: &[Entry]) -> (Option<String>, Option<CompactionDetails>) {
    for entry in branch.iter().rev() {
        if let Entry::Compaction {
            summary, details, ..
        } = entry
        {
            let parsed = details
                .clone()
                .and_then(|value| serde_json::from_value(value).ok());
            return (Some(summary.clone()), parsed);
        }
    }
    (None, None)
}

/// Projects the branch, picks the cut, splits into summarize/prefix/kept, seeds file ops from
/// the prior compaction, pulls the retention floor out. None when there is nothing to do.
pub fn prepare_compaction(branch: &[Entry], settings: &Settings) -> Option<Preparation> {
    if matches!(branch.last(), Some(Entry::Compaction { .. })) {
        return None;
    }
    let (previous_summary, previous_details) = previous_compaction(branch);
    let projected = project(branch);
    let tokens_before = estimate_context(&projected).tokens;
    let work_start = usize::from(matches!(
        projected.first(),
        Some(AgentMessage::CompactionSummary { .. })
    ));
    let work = &projected[work_start..];
    let cut = select_cut(work, settings.keep_recent_tokens);
    let history_end = if cut.is_split_turn {
        cut.turn_start_index.unwrap_or(cut.first_kept_index)
    } else {
        cut.first_kept_index
    };
    let messages_to_summarize: Vec<AgentMessage> = work[..history_end].to_vec();
    let turn_prefix_messages: Vec<AgentMessage> = if cut.is_split_turn {
        work[cut.turn_start_index.unwrap_or(cut.first_kept_index)..cut.first_kept_index].to_vec()
    } else {
        Vec::new()
    };
    if messages_to_summarize.is_empty()
        && turn_prefix_messages.is_empty()
        && previous_summary.is_none()
    {
        return None;
    }
    let mut file_ops = extract_file_ops(&messages_to_summarize, previous_details.as_ref());
    for message in &turn_prefix_messages {
        crate::details::extract_file_ops_from_message(message, &mut file_ops);
    }
    let summarized_away: Vec<AgentMessage> = messages_to_summarize
        .iter()
        .chain(turn_prefix_messages.iter())
        .cloned()
        .collect();
    let floored = retain_floor(&summarized_away, RETENTION_FLOOR_BUDGET);
    let mut retained_tail = floored;
    retained_tail.extend_from_slice(&work[cut.first_kept_index..]);
    let attributed = project_attributed(branch);
    let attributed_work = &attributed[work_start..];
    let view_slice = &attributed_work[..history_end];
    let mut view = compile_view(view_slice, previous_summary.as_deref(), &file_ops);
    view.dropped = dropped_idents(view_slice, &view, &retained_tail);
    Some(Preparation {
        messages_to_summarize,
        turn_prefix_messages,
        retained_tail,
        is_split_turn: cut.is_split_turn,
        tokens_before,
        previous_summary,
        file_ops,
        view,
    })
}

pub fn compose_summary(
    summary: &str,
    file_ops: &FileOps,
    view: &CompiledView,
) -> (String, CompactionDetails) {
    let (read_files, modified_files) = compute_file_lists(file_ops);
    let files = format_file_operations(&read_files, &modified_files);
    let mut view = view.clone();
    view.dropped
        .retain(|(ident, _)| !summary.contains(ident.as_str()));
    if view.dropped.len() > DROPPED_CAP {
        view.dropped.truncate(DROPPED_CAP);
    }
    let dropped = view.dropped.len();
    let mut view_text = view.render();
    for pin in &view.pinned {
        if !view_text.contains(pin.as_str()) {
            view_text.push('\n');
            view_text.push_str(pin);
        }
    }
    let text = if view_text.is_empty() {
        format!("{summary}{files}")
    } else {
        format!("{view_text}\n\n{summary}{files}")
    };
    let mut extra = serde_json::Map::new();
    extra.insert("dropped".to_owned(), serde_json::json!(dropped));
    extra.insert("pinned".to_owned(), serde_json::json!(view.pinned.len()));
    (
        text,
        CompactionDetails {
            read_files,
            modified_files,
            window: None,
            extra,
        },
    )
}

fn dropped_idents(
    span: &[(String, AgentMessage)],
    view: &CompiledView,
    tail: &[AgentMessage],
) -> Vec<(String, String)> {
    let mut kept = identifiers(&view.render());
    for message in tail {
        kept.extend(identifiers(&message_text(message)));
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for (id, message) in span {
        for ident in identifiers(&message_text(message)) {
            if kept.contains(&ident) || !seen.insert(ident.clone()) {
                continue;
            }
            out.push((ident, id.clone()));
        }
    }
    out
}
