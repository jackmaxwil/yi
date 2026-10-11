use serde_json::{Map, Value};
use yi_session::{EntryOrder, EntryQuery};
use yi_tools::{ToolOutput, error_output, text_output};
use yi_types::entry::Entry;
use yi_types::message::AgentMessage;
use yi_types::url::{Scheme, Url};

use super::{FetchError, Resolver};

const HITS: usize = 40;
const LINE_CHARS: usize = 200;
/// The root that names this repository's other root sessions, as `parent` names one.
const REPO: &str = "repo";
const ARGS: [&str; 5] = ["pattern", "path", "literal", "ignore_case", "offset"];

fn kind(message: &AgentMessage) -> String {
    match message {
        AgentMessage::User { .. } => "user".to_owned(),
        AgentMessage::Assistant { .. } => "assistant".to_owned(),
        AgentMessage::ToolResult { tool_name, .. } => format!("result {tool_name}"),
        AgentMessage::Custom { custom_type, .. } => custom_type.clone(),
        AgentMessage::BashExecution { .. } => "bash".to_owned(),
        AgentMessage::BranchSummary { .. } | AgentMessage::CompactionSummary { .. } => {
            "summary".to_owned()
        }
    }
}

fn searchable(entry: &Entry) -> Option<(String, String)> {
    match entry {
        Entry::Message { message, .. } => Some((kind(message), message.plain_text())),
        Entry::Compaction { summary, .. } | Entry::BranchSummary { summary, .. } => {
            Some(("summary".to_owned(), summary.clone()))
        }
        _ => None,
    }
}

impl Resolver {
    /// Every line of a transcript's messages and summaries, tool results included, that the
    /// matcher hits; `history://repo` walks this repository's other root sessions.
    fn grep_history(
        &self,
        url: &Url,
        hit: &dyn Fn(&str) -> bool,
        offset: usize,
    ) -> Result<String, FetchError> {
        let target = self.admitted(url)?;
        let backend = |message: String| FetchError::Backend {
            url: url.to_string(),
            message,
        };
        let (sessions, skipped) = match url.path() {
            REPO => {
                self.history_walled(url)?;
                let own = self
                    .session_store()
                    .map(|store| yi_session::lock_session(&store).metadata().id.clone());
                let (all, skipped) = self.transcripts().map_or((Vec::new(), 0), |t| t.corpus());
                let others = all.into_iter().filter(|(id, _)| Some(id) != own.as_ref());
                (others.collect::<Vec<_>>(), skipped)
            }
            agent => {
                let session =
                    self.transcript_of(target.path())
                        .ok_or_else(|| FetchError::NotFound {
                            url: url.to_string(),
                            what: format!("agent {agent}: no live, kept or on-disk session"),
                        })?;
                (vec![(agent.to_owned(), session)], 0)
            }
        };
        let (mut rows, mut total, mut entries, mut clipped) = (Vec::new(), 0usize, 0usize, false);
        for (agent, session) in &sessions {
            let store = yi_session::lock_session(session);
            let found = store
                .find_entries(&EntryQuery {
                    order: EntryOrder::OldestFirst,
                    ..EntryQuery::default()
                })
                .map_err(|error| backend(error.to_string()))?;
            for entry in &found {
                let Some((kind, text)) = searchable(entry) else {
                    continue;
                };
                entries = entries.saturating_add(1);
                for line in text.lines().filter(|line| hit(line)) {
                    total = total.saturating_add(1);
                    if total <= offset || rows.len() == HITS {
                        continue;
                    }
                    let kept: String = line.chars().take(LINE_CHARS).collect();
                    let cut = if kept.len() < line.len() { "…" } else { "" };
                    clipped = clipped || !cut.is_empty();
                    let id = entry.id();
                    let seq = entry.seq();
                    rows.push(format!("history://{agent}/{id} #{seq} {kind}: {kept}{cut}"));
                }
            }
        }
        let searched = format!("{entries} entries of {} sessions", sessions.len());
        if rows.is_empty() {
            rows.push(match total {
                0 => format!("[no hits in {searched}; try ignore_case, fewer words, or a synonym]"),
                _ => format!("[offset {offset} is past the last of {total} hits]"),
            });
        }
        let shown = rows.len();
        let last = offset.saturating_add(shown);
        if last < total {
            rows.push(format!(
                "[… hits {}-{last} of {total} in {searched} · history grep page {HITS} · grep again with offset={last}]",
                offset.saturating_add(1)
            ));
        }
        if clipped {
            rows.push(format!(
                "[… a hit line is cut at {LINE_CHARS} chars (LINE_CHARS) · read its address for the whole entry]"
            ));
        }
        if skipped > 0 {
            rows.push(format!(
                "[… {skipped} session files not searched: unreadable, or of an unknown repository]"
            ));
        }
        Ok(rows.join("\n"))
    }
}

pub(super) fn serve_grep(resolver: &Resolver, input: &Map<String, Value>) -> Option<ToolOutput> {
    let path = input.get("path").and_then(Value::as_str)?;
    let url = match path.parse::<Url>() {
        Ok(url) if *url.scheme() == Scheme::History => url,
        Err(error) if path.starts_with("history://") => {
            return Some(error_output(format!("{path}: {error}")));
        }
        _ => return None,
    };
    if let Some(other) = input.keys().find(|key| !ARGS.contains(&key.as_str())) {
        return Some(error_output(format!(
            "{other} is a file grep option; a history:// grep takes {}",
            ARGS.join(", ")
        )));
    }
    let matcher = match yi_tools::line_matcher(input) {
        Ok(matcher) => matcher,
        Err(output) => return Some(*output),
    };
    let offset = input
        .get("offset")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(0);
    Some(
        match resolver.grep_history(&url, &|line| matcher.is_match(line), offset) {
            Ok(text) => text_output(text),
            Err(error) => error_output(error.to_string()),
        },
    )
}
