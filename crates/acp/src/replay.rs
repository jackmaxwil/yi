//! Resume replay in frames a reader can hold, and rows naming request parts the agent ignores.

use serde_json::Value;
use yi_runtime::session_store::{EntryOrder, EntryQuery, SharedSession, lock_session};
use yi_types::entry::Entry;
use yi_types::subagent::ChildId;

use crate::forward::session_name;
use crate::update::{
    IdMap, ReplayFrame, extension, replay_update, replay_updates, update_notification,
};
use crate::{AcpState, INTERNAL_ERROR};

pub const REPLAY_CHUNK: usize = 512;
/// Incident: 512 entries of 64 KiB tool output made one 33 MB frame, past the console's 32 MiB
/// reader cap, so a resume of that session dropped the link and re-dialed into the same frame.
pub const REPLAY_BYTES: usize = 8 * 1024 * 1024;

/// Splits at [`REPLAY_CHUNK`] entries or [`REPLAY_BYTES`] serialized, whichever comes first. An
/// entry is never split, and an empty tail is one empty frame, which still carries the leaf.
pub fn frames(entries: &[Entry]) -> Vec<&[Entry]> {
    let mut frames = Vec::new();
    let (mut start, mut bytes) = (0_usize, 0_usize);
    for (index, entry) in entries.iter().enumerate() {
        let size = serde_json::to_vec(entry).map_or(0, |bytes| bytes.len());
        let over = bytes.saturating_add(size) > REPLAY_BYTES;
        if index > start && (index.saturating_sub(start) == REPLAY_CHUNK || over) {
            frames.push(entries.get(start..index).unwrap_or_default());
            (start, bytes) = (index, 0);
        }
        bytes = bytes.saturating_add(size);
    }
    if start < entries.len() || frames.is_empty() {
        frames.push(entries.get(start..).unwrap_or_default());
    }
    frames
}

/// The model reads text and resource links; every other block gets a row saying it was cut.
pub(crate) fn prompt_text(params: &Value) -> String {
    let blocks: &[Value] = params
        .get("prompt")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice);
    let mut text = String::new();
    let mut dropped = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        let field = |key| block.get(key).and_then(Value::as_str);
        match (field("type"), field("text"), field("uri")) {
            (Some("text"), Some(part), _) => text.push_str(part),
            (Some("resource_link"), _, Some(uri)) => {
                if !text.is_empty() && !text.ends_with(char::is_whitespace) {
                    text.push(' ');
                }
                text.push_str(uri);
            }
            (kind, _, _) => dropped.push((index.saturating_add(1), kind.unwrap_or("untyped"))),
        }
    }
    for (position, kind) in dropped {
        let total = blocks.len();
        text.push_str(&format!(
            "\n[…] prompt block {position} of {total} ({kind}) was dropped: this agent reads \
             text and resource_link blocks; send the file as a resource_link"
        ));
    }
    text
}

impl AcpState {
    /// A session request's `mcpServers` are not attached; the session's first notice says so.
    pub(crate) fn emit_servers_notice(&self, session_id: &str, params: &Value) {
        let asked = params
            .get("mcpServers")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        if asked == 0 {
            return;
        }
        let text = format!(
            "[…] 0 of {asked} mcpServers attached: this agent does not take MCP servers over \
             ACP; list them in .mcp.json or ~/.yi/mcp.json"
        );
        let update = extension("_yi/notice", [("text", Value::String(text))]);
        (self.sink)(&update_notification(session_id, update));
    }

    pub(crate) fn emit_replay(
        &self,
        session_id: &str,
        from: u64,
        child: Option<&ChildId>,
    ) -> Result<(), (i64, String)> {
        let Some(handle) = self.sessions.get(session_id) else {
            return Ok(());
        };
        let Some(store) = handle.session.store() else {
            return Ok(());
        };
        let context_window = handle.session.model().context_window;
        self.emit_replay_from(session_id, &store, from, context_window, child)
            .map(|_| ())
    }

    pub(crate) fn emit_replay_from(
        &self,
        session_id: &str,
        store: &SharedSession,
        from: u64,
        context_window: u64,
        child: Option<&ChildId>,
    ) -> Result<u64, (i64, String)> {
        let mut span = yi_types::trace::span("acp.emit_replay");
        let (entries, leaf, goal, stats) = {
            let session = lock_session(store);
            let entries = session
                .find_entries(&EntryQuery {
                    order: EntryOrder::OldestFirst,
                    ..EntryQuery::default()
                })
                .map_err(|error| (INTERNAL_ERROR, error.to_string()))?;
            let leaf = session.leaf_id("main").ok().flatten();
            (entries, leaf, session.goal(), session.stats())
        };
        let stats = child.is_none().then_some(&stats);
        let todos = yi_runtime::todo::latest_record(store).map(|record| record.list);
        let name = session_name(store);
        let total = u64::try_from(entries.len()).unwrap_or(u64::MAX);
        let skip = usize::try_from(from.min(total)).unwrap_or(usize::MAX);
        let tail = entries.get(skip..).unwrap_or_default();
        let mut sent = from.min(total);
        span.set("entries", tail.len());
        let mut chunks = frames(tail).into_iter().peekable();
        while let Some(chunk) = chunks.next() {
            let chunk_from = sent;
            sent = sent.saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
            let last = chunks.peek().is_none();
            let frame = ReplayFrame {
                entries: chunk,
                from: chunk_from,
                replayed_to: sent,
                leaf: if last { leaf.as_deref() } else { None },
                name: name.as_deref(),
                goal: goal.as_ref(),
                todos: todos.as_ref(),
                context_window,
                child,
                stats: stats.filter(|_| last),
            };
            (self.sink)(&update_notification(session_id, replay_update(&frame)));
        }
        Ok(total)
    }

    /// Design §17.2: the stored branch replayed as `session/update`s. `from` skips entries the
    /// client holds (valid only if it saw nothing since); returns the next `replayedTo`.
    pub(crate) fn replay(
        &self,
        session_id: &str,
        store: &SharedSession,
        from: u64,
        context_window: u64,
    ) -> Result<u64, (i64, String)> {
        let mut span = yi_types::trace::span("acp.replay_updates");
        let entries = lock_session(store)
            .find_entries(&EntryQuery {
                order: EntryOrder::OldestFirst,
                ..EntryQuery::default()
            })
            .map_err(|error| (INTERNAL_ERROR, error.to_string()))?;
        let total = u64::try_from(entries.len()).unwrap_or(u64::MAX);
        let skip = usize::try_from(from.min(total)).unwrap_or(usize::MAX);
        let tail = entries.get(skip..).unwrap_or_default();
        let mut ids = IdMap::new(context_window);
        let mut frames = 0_u64;
        for updated in replay_updates(tail, &mut ids) {
            frames = frames.saturating_add(1);
            (self.sink)(&update_notification(session_id, updated));
        }
        span.set("entries", tail.len());
        span.set("frames", frames);
        Ok(total)
    }
}
