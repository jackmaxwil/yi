use serde_json::{Map, Value};
use yi_session::{EntryOrder, EntryQuery};
use yi_tools::{ToolContext, ToolOutput, error_output, text_output};
use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, Content};
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

/// Incident: a pattern a parent passed to its own call matched as an `assistant` line, beside
/// the output it was after; a call's arguments are searched, but labelled as the call.
fn searchable(entry: &Entry) -> Vec<(String, String)> {
    match entry {
        Entry::Message {
            message: AgentMessage::Assistant { content, .. },
            ..
        } => content
            .iter()
            .filter_map(|block| match block {
                Content::Text { text, .. } => Some(("assistant".to_owned(), text.clone())),
                Content::ToolCall {
                    name, arguments, ..
                } => Some((
                    format!("call {name}"),
                    Value::Object(arguments.clone()).to_string(),
                )),
                _ => None,
            })
            .collect(),
        Entry::Message { message, .. } => vec![(kind(message), message.plain_text())],
        Entry::Compaction { summary, .. } | Entry::BranchSummary { summary, .. } => {
            vec![("summary".to_owned(), summary.clone())]
        }
        _ => Vec::new(),
    }
}

/// `tail/N` is the last N entries, `since/S` everything after sequence S (D165): the one window
/// grammar `read` and `grep` both serve.
pub(super) fn window(selector: &str) -> Option<(&str, u64)> {
    let (kind, raw) = selector.split_once('/')?;
    let number = raw.parse::<u64>().ok()?;
    matches!(kind, "tail" | "since").then_some((kind, number))
}

pub(super) fn in_window<'a>(
    entries: &'a [yi_types::entry::Entry],
    (kind, number): (&str, u64),
) -> Vec<&'a yi_types::entry::Entry> {
    match kind {
        "tail" => {
            let keep = usize::try_from(number).unwrap_or(usize::MAX);
            entries
                .iter()
                .skip(entries.len().saturating_sub(keep))
                .collect()
        }
        _ => entries
            .iter()
            .filter(|entry| entry.seq() > number)
            .collect(),
    }
}

/// The entries a root's selector names, by `read`'s own grammar; `None` when it names none,
/// which is refused as `read` refuses it rather than answered as zero hits.
fn within<'a>(found: &'a [Entry], selector: &str) -> Option<Vec<&'a Entry>> {
    if selector.is_empty() {
        return Some(found.iter().collect());
    }
    if let Some(window) = window(selector) {
        return Some(in_window(found, window));
    }
    let entry: Vec<&Entry> = found
        .iter()
        .filter(|entry| entry.id() == selector)
        .collect();
    (!entry.is_empty()).then_some(entry)
}

const NO_CORPUS: &str = "a session root: a spawned child searches no past sessions, so ask \
                         the parent, whose grep of history://repo does";

#[derive(Default)]
struct Hits {
    rows: Vec<String>,
    total: usize,
    entries: usize,
    sessions: usize,
    clipped: bool,
    skipped: usize,
}

impl Hits {
    fn scan(
        &mut self,
        agent: &str,
        store: &yi_session::SessionStore,
        window: &str,
        hit: &dyn Fn(&str) -> bool,
        offset: usize,
    ) -> Result<bool, String> {
        let found = store
            .find_entries(&EntryQuery {
                order: EntryOrder::OldestFirst,
                ..EntryQuery::default()
            })
            .map_err(|error| error.to_string())?;
        let Some(chosen) = within(&found, window) else {
            return Ok(false);
        };
        self.sessions = self.sessions.saturating_add(1);
        for entry in chosen {
            let parts = searchable(entry);
            self.entries = self.entries.saturating_add(usize::from(!parts.is_empty()));
            let lines = parts
                .iter()
                .flat_map(|(kind, text)| text.lines().map(move |line| (kind, line)));
            for (kind, line) in lines.filter(|(_, line)| hit(line)) {
                self.total = self.total.saturating_add(1);
                if self.total <= offset || self.rows.len() == HITS {
                    continue;
                }
                let kept: String = line.chars().take(LINE_CHARS).collect();
                let cut = if kept.len() < line.len() { "…" } else { "" };
                self.clipped = self.clipped || !cut.is_empty();
                let (id, seq) = (entry.id(), entry.seq());
                self.rows
                    .push(format!("history://{agent}/{id} #{seq} {kind}: {kept}{cut}"));
            }
        }
        Ok(true)
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
        let mut hits = Hits::default();
        match url.path() {
            REPO => {
                self.history_walled(url)?;
                let listing = self
                    .transcripts()
                    .and_then(|desk| desk.corpus())
                    .ok_or_else(|| super::unsupported(url, NO_CORPUS))?;
                let own = self.session_store().map(|store| {
                    let metadata = yi_session::lock_session(&store).metadata().clone();
                    (metadata.id, metadata.parent_session_id.is_some())
                });
                // Invariant: a spawned child never walks the corpus; its parent searches it.
                if own.as_ref().is_some_and(|(_, spawned)| *spawned) {
                    return Err(super::unsupported(url, NO_CORPUS));
                }
                let own = own.map(|(id, _)| id);
                hits.skipped = listing.skipped;
                // Invariant: one parsed session is resident at a time, however large the corpus.
                for file in &listing.files {
                    let Ok(store) = yi_session::load_session(file) else {
                        hits.skipped = hits.skipped.saturating_add(1);
                        continue;
                    };
                    let id = store.metadata().id.clone();
                    if Some(&id) != own.as_ref() {
                        hits.scan(&id, &store, "", hit, offset).map_err(backend)?;
                    }
                }
            }
            asked => {
                let (session, window) =
                    self.history_root(target.path())
                        .ok_or_else(|| FetchError::NotFound {
                            url: url.to_string(),
                            what: format!("agent {asked}: no live, kept or on-disk session"),
                        })?;
                self.spellings_walled(url, &session, window)?;
                let window = window.unwrap_or_default();
                let agent = asked.strip_suffix(window).unwrap_or(asked);
                let agent = agent.strip_suffix('/').unwrap_or(agent);
                let store = yi_session::lock_session(&session);
                if !hits
                    .scan(agent, &store, window, hit, offset)
                    .map_err(backend)?
                {
                    return Err(FetchError::NotFound {
                        url: url.to_string(),
                        what: format!("entry {window}"),
                    });
                }
            }
        }
        let Hits {
            mut rows,
            total,
            entries,
            sessions,
            clipped,
            skipped,
        } = hits;
        let searched = format!("{entries} entries of {sessions} sessions");
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
                "[… {skipped} not searched: session directories whose checkout is gone, and unreadable session files]"
            ));
        }
        Ok(rows.join("\n"))
    }
}

pub(super) fn serve_grep(
    resolver: &Resolver,
    input: &Map<String, Value>,
    _context: &ToolContext,
) -> Option<ToolOutput> {
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
