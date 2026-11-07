use yi_types::compaction::CompactionDetails;
use yi_types::entry::Entry;
use yi_types::message::AgentMessage;

use crate::account::{Tokens, estimate_context};
use crate::cut::select_cut;
use crate::details::{FileOps, compute_file_lists, extract_file_ops, format_file_operations};
use crate::floor::{RETENTION_FLOOR_BUDGET, retain_floor};
use crate::policy::Settings;
use crate::project::project_attributed;
use crate::view::{CompiledView, compile_view, view_extra, view_from_extra};

#[derive(Debug, Clone, PartialEq)]
pub struct Preparation {
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
    let _span = yi_types::trace::span("context.prepare_compaction").arg("entries", branch.len());
    if matches!(branch.last(), Some(Entry::Compaction { .. })) {
        return None;
    }
    let (previous_summary, previous_details) = previous_compaction(branch);
    let attributed = project_attributed(branch);
    let projected: Vec<AgentMessage> = attributed
        .iter()
        .map(|entry| entry.message.clone())
        .collect();
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
    let previous_view = previous_details
        .as_ref()
        .and_then(|d| view_from_extra(&d.extra));
    let view = compile_view(
        &attributed[work_start..][..cut.first_kept_index],
        previous_view.as_ref(),
    );
    Some(Preparation {
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
    let files = format_file_operations(&read_files, &modified_files, file_ops.reads_dropped);
    let view_text = view.render();
    let text = if view_text.is_empty() {
        format!("{summary}{files}")
    } else {
        format!("{view_text}\n\n{summary}{files}")
    };
    let mut extra: serde_json::Map<_, _> = view_extra(view).into_iter().collect();
    if file_ops.reads_dropped > 0 {
        let dropped = serde_json::json!(file_ops.reads_dropped);
        extra.insert(yi_types::kernel::NESTED_READS_DROPPED.to_owned(), dropped);
    }
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

/// Invariant: a reply missing Values or Dropped lost that section, so previous rows return with
/// the oldest rows cut first. A reply that has a section is trusted, edits and removals included.
pub const VALUES_CARRIED_CAP: usize = 40;
pub const DROPPED_CARRIED_CAP: usize = 40;

fn section_rows<'a>(summary: &'a str, heading: &str) -> Vec<&'a str> {
    let mut rows = Vec::new();
    let mut inside = false;
    for line in summary.lines() {
        if line.starts_with("## ") {
            inside = line.trim_end() == heading;
        } else if line.starts_with('<') || (line.starts_with('[') && !line.starts_with("[… ")) {
            inside = false;
        } else if inside && line.starts_with("- ") && line.trim() != "- (none)" {
            rows.push(line.trim_end());
        }
    }
    rows
}

fn carried_block(
    previous: Option<&str>,
    summary: &str,
    heading: &str,
    cap: usize,
    cap_name: &str,
    what: &str,
    next_call: &str,
) -> Option<String> {
    if summary.lines().any(|line| line.trim_end() == heading) {
        return None;
    }
    let rows = previous
        .map(|text| section_rows(text, heading))
        .unwrap_or_default();
    if rows.is_empty() {
        return None;
    }
    let mut block = String::from(heading);
    if rows.len() > cap {
        block.push('\n');
        block.push_str(&yi_tools::cut_row(
            cap,
            rows.len(),
            what,
            cap_name,
            cap,
            next_call,
        ));
    }
    for row in rows.iter().skip(rows.len().saturating_sub(cap)) {
        block.push('\n');
        block.push_str(row);
    }
    Some(block)
}

pub fn keep_values(previous: Option<&str>, summary: &str) -> String {
    let values = carried_block(
        previous,
        summary,
        "## Values",
        VALUES_CARRIED_CAP,
        "VALUES_CARRIED_CAP",
        "earlier value rows carried",
        "grep(pattern=\"<value>\", path=\"history://self\")",
    );
    let dropped = carried_block(
        previous,
        summary,
        "## Dropped",
        DROPPED_CARRIED_CAP,
        "DROPPED_CARRIED_CAP",
        "earlier dropped rows carried",
        "grep(pattern=\"## Dropped\", path=\"history://self\")",
    );
    if values.is_none() && dropped.is_none() {
        return summary.to_owned();
    }
    let mut text = summary.trim_end().to_owned();
    if let Some(block) = values {
        match text.find("\n## Dropped") {
            Some(at) => {
                let (head, tail) = text.split_at(at);
                text = format!("{}\n\n{block}\n{tail}", head.trim_end());
            }
            None => {
                text.push_str("\n\n");
                text.push_str(&block);
            }
        }
    }
    if let Some(block) = dropped {
        text.push_str("\n\n");
        text.push_str(&block);
    }
    text
}
