//! Incident: replaying one Opus session, a watermark that cut old tool results every turn lost
//! $316-466 in cache rewrites; a cut here is made only when the reads it saves pay for one.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};

use yi_types::entry::{CustomRecord, Entry};
use yi_types::image::attach_image_call;
use yi_types::message::{AgentMessage, Content, has_image};
use yi_types::model::{CACHE_DIAGNOSTIC, Model, Ttl};
use yi_types::reclaim::{AnnotationRecord, ReclaimRecord, Reclaimed};

pub(crate) const KEEP_TURNS: u64 = 5;
/// Below this many tokens a cut's rewrite and its notice cost more than the reads it saves.
const MIN_DROPPED: u64 = 2_000;
/// Cache reads a rewrite counts as on a route with no catalog cache prices.
const REWRITE_READS: u64 = 10;
/// A session's age says nothing about requests left: savings count over at most this many.
const MAX_HORIZON_TURNS: u64 = 40;
const MIN_RESULT_CHARS: usize = 1_200;
/// The newest image results no cut takes: the model is still working from them.
const KEEP_IMAGES: usize = 3;
const PLACEHOLDER_TOKENS: u64 = 40;
/// Results that carry the user's own words or live plan state, which a cut would hide.
pub(crate) const NEVER: [&str; 3] = ["ask_user", "plan", "todo"];
const FIVE_MINUTES_MS: u64 = 5 * 60 * 1000;
const HOUR_MS: u64 = 60 * 60 * 1000;

type Args = serde_json::Map<String, serde_json::Value>;
type Key = (String, String);

/// Every result cut so far on this branch, by call id and text hash, and the model's pins and
/// discards; a resume folds the same.
#[derive(Default)]
pub(crate) struct Cut {
    marks: BTreeMap<Key, Mark>,
    pins: BTreeMap<Key, ()>,
    discards: BTreeMap<Key, ()>,
    sections: Sections,
}

/// Invariant: keyed by the text's hash, never the call id, which leaked calls reuse. The paths
/// each read, edit or write result's `[path#TAG]` headers name, parsed once per text.
#[derive(Default)]
pub(crate) struct Sections(Mutex<BTreeMap<u64, Vec<String>>>);

impl Sections {
    pub(crate) fn paths(&self, content: &[Content]) -> Vec<String> {
        self.memo(content, |text| {
            text.lines()
                .filter_map(yi_tools::hashline::tokenizer::try_parse_header)
                .filter(|header| header.file_hash.is_some())
                .map(|header| header.path)
                .collect()
        })
    }

    /// The lock is held across `parse`, so readers of one new text at once parse it once.
    fn memo(&self, content: &[Content], parse: impl FnOnce(&str) -> Vec<String>) -> Vec<String> {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        texts(content).for_each(|text| text.hash(&mut hasher));
        let mut memo = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let parsed = || parse(&yi_types::message::join_text(content, "\n"));
        memo.entry(hasher.finish()).or_insert_with(parsed).clone()
    }
}

struct Mark {
    entry: Option<String>,
    turn: u64,
}

impl Cut {
    pub(crate) fn fold(entries: &[Entry]) -> Self {
        let mut cut = Self::default();
        for entry in entries {
            if let Entry::Custom {
                custom_type,
                data: Some(data),
                ..
            } = entry
                && custom_type == ReclaimRecord::TYPE
                && let Ok(record) = serde_json::from_value::<ReclaimRecord>(data.clone())
            {
                cut.add(&record);
            }
        }
        annotations(entries).for_each(|record| cut.note(&record));
        cut
    }

    fn add(&mut self, record: &ReclaimRecord) {
        for item in &record.items {
            let mark = Mark {
                entry: item.entry_id.clone(),
                turn: record.turn,
            };
            (self.marks).insert((item.tool_call_id.clone(), item.hash.clone()), mark);
        }
    }

    pub(crate) fn note(&mut self, record: &AnnotationRecord) {
        let Some(target) = &record.target else {
            return;
        };
        let key = (target.tool_call_id.clone(), target.hash.clone());
        // The model's last mark on a result wins: a discard lets go of an earlier pin.
        match record.kind.as_str() {
            "pin" => drop((self.discards.remove(&key), self.pins.insert(key, ()))),
            "discard" => drop((self.pins.remove(&key), self.discards.insert(key, ()))),
            _ => {}
        }
    }

    /// Invariant: a cut image whose picture can no longer be had goes out whole, so it is no cut
    /// to the price, the view or annotate either.
    fn get(&self, message: &AgentMessage) -> Option<&Mark> {
        find(&self.marks, message).filter(|_| reattach(message).is_ok())
    }

    pub(crate) fn cut_at(&self, message: &AgentMessage) -> Option<u64> {
        self.get(message).map(|mark| mark.turn)
    }
}

/// A message's entry in `map`; hashing every stored result on every request is the cost, so
/// the call id is probed first.
fn find<'a, V>(map: &'a BTreeMap<Key, V>, message: &AgentMessage) -> Option<&'a V> {
    let AgentMessage::ToolResult {
        tool_call_id,
        content,
        ..
    } = message
    else {
        return None;
    };
    let start = (tool_call_id.clone(), String::new());
    let mut same = map
        .range(start..)
        .take_while(|((id, _), _)| id == tool_call_id);
    let first = same.next()?;
    let text = hash(content);
    std::iter::once(first)
        .chain(same)
        .find(|((_, at), _)| *at == text)
        .map(|(_, value)| value)
}

type StoreFn = dyn Fn() -> Option<yi_session::SharedSession> + Send + Sync;

/// The view stage a run's requests pass through: it may record a new cut, then renders every
/// cut result as its placeholder, so the bytes of a cut are the same on every later request.
pub(crate) struct Overlay {
    cut: Arc<Mutex<Cut>>,
    store: Box<StoreFn>,
    model: Model,
}

impl Overlay {
    pub(crate) fn new(cut: Arc<Mutex<Cut>>, store: Box<StoreFn>, model: Model) -> Self {
        Self { cut, store, model }
    }

    pub(crate) fn view(&self, messages: Vec<AgentMessage>) -> Vec<AgentMessage> {
        let _span = yi_types::trace::span("reclaim.view");
        let mut cut = self.cut.lock().unwrap_or_else(PoisonError::into_inner);
        let now = yi_session::now_ms();
        if let Some(mut record) = decide(&messages, &cut, &self.model, now) {
            if let Some(store) = (self.store)() {
                let mut store = yi_session::lock_session(&store);
                name_entries(&store, &mut record.items);
                // An unrecorded cut still holds for this process; a resume shows the result whole.
                let _recorded_or_not = store.append_custom_record(&record);
            }
            cut.add(&record);
        }
        apply(messages, &cut)
    }
}

/// Renders every recorded cut and decides nothing: a request sent beside the loop repeats the
/// bytes the loop last sent instead of spending a cut of its own (D338).
pub(crate) fn convert(cut: Option<Arc<Mutex<Cut>>>) -> crate::compaction::Convert {
    match cut {
        Some(cut) => Arc::new(move |messages: &[AgentMessage]| {
            let converted = yi_context::convert_to_llm(messages);
            apply(
                converted,
                &cut.lock().unwrap_or_else(PoisonError::into_inner),
            )
        }),
        None => Arc::new(yi_context::convert_to_llm),
    }
}

fn name_entries(store: &yi_session::SessionStore, items: &mut [Reclaimed]) {
    let bounds = yi_session::BranchBounds::default();
    let query = yi_session::EntryQuery::default();
    for entry in store
        .find_entries_on_branch("main", &query, &bounds)
        .unwrap_or_default()
    {
        if let Entry::Message {
            id,
            message:
                AgentMessage::ToolResult {
                    tool_call_id,
                    content,
                    ..
                },
            ..
        } = entry
        {
            let text = hash(&content);
            let named = items
                .iter_mut()
                .find(|item| item.tool_call_id == tool_call_id && item.hash == text);
            named
                .into_iter()
                .for_each(|item| item.entry_id = Some(id.clone()));
        }
    }
}

fn texts(content: &[Content]) -> impl Iterator<Item = &str> {
    content.iter().filter_map(|part| match part {
        Content::Text { text, .. } => Some(text.as_str()),
        _ => None,
    })
}

fn chars(content: &[Content]) -> usize {
    texts(content).map(|text| text.chars().count()).sum()
}

/// An image result's files to attach again, its `attachmentMedia` rows: `Ok(None)` with no image,
/// `Err` unless every file still has the size and mtime the kernel recorded.
fn reattach(message: &AgentMessage) -> Result<Option<Vec<String>>, ()> {
    let AgentMessage::ToolResult {
        content, details, ..
    } = message
    else {
        return Ok(None);
    };
    let images = content
        .iter()
        .filter(|part| matches!(part, Content::Image { .. }))
        .count();
    if images == 0 {
        return Ok(None);
    }
    let media = (details.as_ref())
        .and_then(|details| details.get("attachmentMedia")?.as_array())
        .ok_or(())?;
    let paths: Vec<String> = media
        .iter()
        .filter_map(|row| {
            let path = row.get("path")?.as_str()?;
            let recorded_bytes = row.get("bytes")?.as_u64()?;
            let recorded_modified_ns = row.get("modified_ns")?.as_u64()?;
            // Invariant: a kernel-named path is only asked whether it has the exact size and
            // nanosecond mtime the kernel claimed, which confirms a guess and reveals nothing else.
            let metadata = std::fs::metadata(path).ok()?;
            let modified = metadata.modified().ok()?;
            let since_epoch = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
            let modified_ns = u64::try_from(since_epoch.as_nanos()).ok()?;
            (metadata.is_file()
                && metadata.len() == recorded_bytes
                && modified_ns == recorded_modified_ns)
                .then(|| path.to_owned())
        })
        .collect();
    (paths.len() == images).then_some(Some(paths)).ok_or(())
}

pub(crate) fn newest_images<M: std::borrow::Borrow<AgentMessage>>(
    messages: &[M],
) -> BTreeSet<usize> {
    let shown: Vec<usize> = (messages.iter().enumerate())
        .filter(|(_, message)| match message.borrow() {
            AgentMessage::ToolResult { content, .. } => has_image(content),
            _ => false,
        })
        .map(|(index, _)| index)
        .collect();
    let keep = shown.len().saturating_sub(KEEP_IMAGES);
    (shown.get(keep..).unwrap_or_default())
        .iter()
        .copied()
        .collect()
}

pub(crate) fn hash(content: &[Content]) -> String {
    let text = yi_types::message::join_text(content, "");
    crate::ext::content_hash(&text).chars().take(16).collect()
}

/// The call each tool result answers, by index: looked up in the nearest assistant message
/// before it, since a call id repeats across messages.
pub(crate) fn calls<M: std::borrow::Borrow<AgentMessage>>(
    messages: &[M],
) -> Vec<Option<(&str, &Args)>> {
    let mut asked: Vec<(&str, &str, &Args)> = Vec::new();
    messages
        .iter()
        .map(|message| match message.borrow() {
            AgentMessage::Assistant { content, .. } => {
                asked = content
                    .iter()
                    .filter_map(|part| match part {
                        Content::ToolCall {
                            id,
                            name,
                            arguments,
                            ..
                        } => Some((id.as_str(), name.as_str(), arguments)),
                        _ => None,
                    })
                    .collect();
                None
            }
            AgentMessage::ToolResult { tool_call_id, .. } => (asked.iter())
                .find(|(id, _, _)| id == tool_call_id)
                .map(|(_, name, args)| (*name, *args)),
            _ => None,
        })
        .collect()
}

/// Every annotation on the branch, in the order the model wrote them.
pub(crate) fn annotations(entries: &[Entry]) -> impl Iterator<Item = AnnotationRecord> + '_ {
    entries.iter().filter_map(|entry| match entry {
        Entry::Custom {
            custom_type,
            data: Some(data),
            ..
        } if custom_type == AnnotationRecord::TYPE => serde_json::from_value(data.clone()).ok(),
        _ => None,
    })
}

/// The newest read of each file the session edited, by the `[path#TAG]` headers the read, edit
/// and write results print: an edit needs its file's tags, a file only read is cut like any result.
pub(crate) fn newest_reads<M: std::borrow::Borrow<AgentMessage>>(
    calls: &[Option<(&str, &Args)>],
    messages: &[M],
    sections: &Sections,
) -> BTreeSet<usize> {
    let result = |index: usize| match messages.get(index).map(|message| message.borrow()) {
        Some(AgentMessage::ToolResult {
            content, details, ..
        }) => Some((content, details)),
        _ => None,
    };
    let shown = |index: usize| -> Vec<String> {
        (result(index)).map_or_else(Vec::new, |(content, _)| sections.paths(content))
    };
    let (mut edited, mut latest) = (Vec::new(), BTreeMap::new());
    for (index, call) in calls.iter().enumerate() {
        match call.map(|(name, _)| name) {
            Some("read") => latest.extend(shown(index).into_iter().map(|path| (path, index))),
            Some("edit" | "write") => edited.extend(shown(index)),
            Some("ipython") => {
                let mut ops = yi_context::FileOps::default();
                if let Some(message) = messages.get(index).map(|message| message.borrow()) {
                    yi_context::details::extract_file_ops_kept(message, &mut ops, |row| row.ok);
                }
                edited.extend(ops.edited.into_iter().chain(ops.written));
            }
            _ => {}
        }
    }
    // ponytail: no cwd here, so `/abs/src/lib.rs` and `src/lib.rs` are one file by suffix; a
    // same-named file elsewhere keeps its read too, which costs tokens, never an edit's tags.
    let same = |read: &str| {
        let read = std::path::Path::new(read);
        (edited.iter())
            .any(|path: &String| read.ends_with(path) || std::path::Path::new(path).ends_with(read))
    };
    (latest.into_iter())
        .filter_map(|(path, index)| same(&path).then_some(index))
        .collect()
}

/// Why no cut ever takes this result, whatever its age or the model's discard.
pub(crate) fn never_cut(
    message: &AgentMessage,
    newest_read: bool,
    newest_image: bool,
) -> Option<&'static str> {
    let AgentMessage::ToolResult {
        tool_name, content, ..
    } = message
    else {
        return Some("it is not a tool result");
    };
    if NEVER.contains(&tool_name.as_str()) {
        Some(
            "plan, todo and ask_user results carry live state or the user's words and are never cut",
        )
    } else if has_image(content) && newest_image {
        Some(
            "one of the three newest image results is never cut, since the model still works from it",
        )
    } else if reattach(message).is_err() {
        Some(
            "an image result is never cut unless every image's source file keeps the size and modification time it was attached with",
        )
    } else if newest_read {
        Some(
            "the newest read of a file this session edited is never cut, since an edit needs its line tags",
        )
    } else {
        None
    }
}

/// A new cut, when one pays; pure over the messages, the cut, the prices, the clock and the image files.
pub(crate) fn decide(
    messages: &[AgentMessage],
    cut: &Cut,
    model: &Model,
    now: u64,
) -> Option<ReclaimRecord> {
    let calls = calls(messages);
    let newest = newest_reads(&calls, messages, &cut.sections);
    let images = newest_images(messages);
    let marks: Vec<bool> = (messages.iter())
        .map(|message| cut.get(message).is_some())
        .collect();
    let (mut turn, mut turns) = (0_u64, Vec::with_capacity(messages.len()));
    for message in messages {
        if matches!(message, AgentMessage::Assistant { .. }) {
            turn = turn.saturating_add(1);
        }
        turns.push(turn);
    }
    let eligible = |index: usize| -> bool {
        let Some(message) = messages.get(index) else {
            return false;
        };
        let AgentMessage::ToolResult { content, .. } = message else {
            return false;
        };
        let aged = turns
            .get(index)
            .is_some_and(|at| turn.saturating_sub(*at) >= crate::levers::get().reclaim_keep_turns);
        let marked = |map| find(map, message).is_some();
        let sized = chars(content) >= MIN_RESULT_CHARS || has_image(content);
        let floors = (aged && sized) || marked(&cut.discards);
        if !floors {
            return false;
        }
        let kept = marks.get(index).copied().unwrap_or(false)
            || marked(&cut.pins)
            || never_cut(message, newest.contains(&index), images.contains(&index)).is_some();
        !kept
    };
    // A result already cut costs its placeholder, so a new cut's rewrite is the view's own size.
    let sizes: Vec<u64> = (messages.iter().zip(&marks))
        .map(|(message, cut)| {
            if *cut {
                PLACEHOLDER_TOKENS
            } else {
                yi_context::estimate_message(message).0
            }
        })
        .collect();
    let mut picked: Vec<usize> = (0..messages.len())
        .filter(|index| eligible(*index))
        .collect();
    // The oldest pick sets how much is rewritten: drop picks from the front until the rest pays.
    while let Some(&first) = picked.first() {
        let dropped = (picked.iter())
            .filter_map(|index| sizes.get(*index))
            .map(|size| size.saturating_sub(PLACEHOLDER_TOKENS))
            .fold(0, u64::saturating_add);
        let rewritten = sizes
            .get(first..)
            .unwrap_or_default()
            .iter()
            .fold(0_u64, |sum, size| sum.saturating_add(*size));
        if let Some(reason) = reason(messages, model, now, (dropped, rewritten, turn)) {
            let items = picked
                .iter()
                .filter_map(|index| match messages.get(*index)? {
                    AgentMessage::ToolResult {
                        tool_call_id,
                        content,
                        ..
                    } => Some(Reclaimed {
                        tool_call_id: tool_call_id.clone(),
                        hash: hash(content),
                        entry_id: None,
                    }),
                    _ => None,
                });
            return Some(ReclaimRecord {
                items: items.collect(),
                reason: reason.to_owned(),
                turn,
                extra: serde_json::Map::new(),
            });
        }
        picked.remove(0);
    }
    None
}

/// Cache reads a rewrite of the prefix costs: the route's own write over read price when the
/// catalog prices both, else [`REWRITE_READS`].
fn rewrite_reads(model: &Model) -> u64 {
    match (
        model.cost.cache_read.as_f64(),
        model.cost.cache_write.as_f64(),
    ) {
        (Some(read), Some(write)) if read > 0.0 && write > read => {
            (write / read).ceil().clamp(1.0, 1_000.0) as u64
        }
        _ => REWRITE_READS,
    }
}

/// Why a cut pays now, if it does: the cache lapsed, or the tokens it saves over as many more
/// requests as were sent so far (at most forty) outweigh a rewrite of everything after the cut.
fn reason(
    messages: &[AgentMessage],
    model: &Model,
    now: u64,
    (dropped, rewritten, turns): (u64, u64, u64),
) -> Option<&'static str> {
    // A synthesized reply (an abort, an error) has no usage; one stored before stamping no time.
    let replies = messages.iter().rev().filter_map(|message| match message {
        AgentMessage::Assistant {
            usage,
            timestamp,
            diagnostics,
            ..
        } if *timestamp > 0 && usage.input.saturating_add(usage.cache_read) > 0 => {
            Some((usage, *timestamp, diagnostics))
        }
        _ => None,
    });
    let replies: Vec<_> = replies.collect();
    let priced = model
        .cost
        .cache_read
        .as_f64()
        .is_some_and(|rate| rate > 0.0);
    let reads_seen = replies.iter().any(|(usage, _, _)| usage.cache_read > 0);
    let caches = priced || reads_seen;
    let lapsed = replies.first().is_some_and(|(_, at, diagnostics)| {
        let hour = (diagnostics.iter().flatten())
            .find(|note| note.diagnostic_type == CACHE_DIAGNOSTIC)
            .and_then(|note| note.details.as_ref()?.get("ttl")?.as_str())
            == Some(Ttl::Hour1.label());
        caches && now.saturating_sub(*at) > if hour { HOUR_MS } else { FIVE_MINUTES_MS }
    });
    if lapsed {
        return (dropped >= MIN_DROPPED).then_some("expired");
    }
    // An hour-cache write costs twice a five-minute one.
    let hour = replies
        .first()
        .is_some_and(|(usage, _, _)| usage.cache_write1h.unwrap_or(0) > 0);
    let reads = rewrite_reads(model) * if hour { 2 } else { 1 };
    let horizon = turns.min(MAX_HORIZON_TURNS);
    let pays = dropped >= MIN_DROPPED
        && (!caches || dropped.saturating_mul(horizon) > rewritten.saturating_mul(reads));
    pays.then_some("breakeven")
}

/// In a forked child `self` and "you" mean the child, so its seed's placeholders name the parent.
pub(crate) fn for_fork(cut: &Mutex<Cut>, messages: Vec<AgentMessage>) -> Vec<AgentMessage> {
    let cut = cut.lock().unwrap_or_else(PoisonError::into_inner);
    render(messages, &cut, ("the parent", "parent", "the parent's"))
}

/// Every cut result as its placeholder: kept nothing of how much, the rule, and the read
/// that restores it, all from the stored result, so the line is the same on every request.
pub(crate) fn apply(messages: Vec<AgentMessage>, cut: &Cut) -> Vec<AgentMessage> {
    render(messages, cut, ("you", "self", "this session's"))
}

fn render(
    mut messages: Vec<AgentMessage>,
    cut: &Cut,
    (who, agent, store): (&str, &str, &str),
) -> Vec<AgentMessage> {
    if cut.marks.is_empty() {
        return messages;
    }
    let briefs: Vec<Option<String>> = (calls(&messages).into_iter())
        .map(|call| call.map(|(name, args)| brief(name, args)))
        .collect();
    for (message, call) in messages.iter_mut().zip(briefs) {
        let Some(mark) = find(&cut.marks, message) else {
            continue;
        };
        // A discard is refused once its result is cut, so this reads the same on every request.
        let rule = match find(&cut.discards, message) {
            Some(()) => format!("{who} discarded it"),
            None => format!(
                "results older than {} requests (reclaim.keep_turns) are cut once dropping them costs less than resending them",
                crate::levers::get().reclaim_keep_turns
            ),
        };
        let Ok(image) = reattach(message) else {
            continue;
        };
        if let AgentMessage::ToolResult { content, .. } = message {
            let call = call.as_deref().unwrap_or("a tool call");
            let (kept, restore) = match image {
                Some(paths) => {
                    let count = paths.len();
                    let paths: Vec<String> = (paths.iter())
                        .map(|path| serde_json::Value::from(path.as_str()).to_string())
                        .collect();
                    let command = attach_image_call(paths.iter().map(String::as_str));
                    let image = match count {
                        1 => "its image dropped".to_owned(),
                        images => format!("its {images} images dropped"),
                    };
                    let restore = match &mark.entry {
                        Some(entry) => {
                            format!("get it: {command}; text: read(\"history://{agent}/{entry}\")")
                        }
                        None => format!("get it: {command}; its text is not in {store} store"),
                    };
                    (
                        format!(
                            "whole result dropped, none of {} chars kept, {image}",
                            chars(content)
                        ),
                        restore,
                    )
                }
                None => (
                    format!("none of {} chars kept", chars(content)),
                    match &mark.entry {
                        Some(entry) => format!("get it: read(\"history://{agent}/{entry}\")"),
                        None => format!("it is not in {store} store"),
                    },
                ),
            };
            let text = format!(
                "[reclaimed: {call} → {kept}; {rule} (cut at request {}); {restore}]",
                mark.turn,
            );
            *content = vec![Content::Text {
                text,
                text_signature: None,
            }];
        }
    }
    messages
}

pub(crate) fn brief(name: &str, args: &Args) -> String {
    let field = ["path", "command", "pattern", "url"]
        .iter()
        .find_map(|key| args.get(*key).and_then(serde_json::Value::as_str));
    match field {
        Some(value) => {
            let short: String = value.chars().take(80).collect();
            let more = if short.len() < value.len() { "…" } else { "" };
            format!("{name} `{short}{more}`")
        }
        None => name.to_owned(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{Cut, apply, decide};
    use serde_json::json;
    use yi_types::entry::{CustomRecord, Entry};
    use yi_types::message::{AgentMessage, Content, UserContent};
    use yi_types::model::Model;
    use yi_types::reclaim::{AnnotationRecord, ReclaimRecord, Reclaimed};

    type Fallible = Result<(), Box<dyn std::error::Error>>;

    /// The recorded session `01a0d6e4` (scrubbed by `evals/record.py`), message rows in order.
    fn recorded() -> Result<Vec<AgentMessage>, Box<dyn std::error::Error>> {
        let mut messages = Vec::new();
        for row in include_str!("../../tui/tests/fixtures/sessions/01a0d6e4.jsonl").lines() {
            let row: serde_json::Value = serde_json::from_str(row)?;
            if row["type"] == "message" {
                messages.push(serde_json::from_value(row["message"].clone())?);
            }
        }
        Ok(messages)
    }

    /// The recording with an edit of README.md spliced in after its first reads, in the shape
    /// the edit tool printed in `01a129b6`, and every reply stamped long before the real clock.
    fn recorded_with_an_edit() -> Result<Vec<AgentMessage>, Box<dyn std::error::Error>> {
        let mut messages = recorded()?;
        for message in &mut messages {
            if let AgentMessage::Assistant { timestamp, .. } = message {
                *timestamp = REPLIED;
            }
        }
        // README.md's read is answered just before docs/ARCHITECTURE.md's, the batch's last.
        let readme = messages.iter().position(|message| {
            matches!(message, AgentMessage::ToolResult { content, .. }
                if yi_types::message::join_text(content, "").starts_with("[README.md#"))
        });
        let after = readme.ok_or("no README.md read")?.saturating_add(2);
        let patch = json!({"patch": "[README.md#DDA9]\nPUT 3.=3:\n+Personal coding agent."});
        let edited = serde_json::from_value(json!({"role": "toolResult", "toolCallId": "c-edit",
            "toolName": "edit", "isError": false, "timestamp": 0, "content": [{"type": "text",
            "text": "[README.md#2D18]\nupdated; first change at line 3\n3:Personal coding agent."}]}))?;
        let edit = assistant(&[("c-edit".to_owned(), "edit", patch)], 0);
        messages.splice(after..after, [edit, edited]);
        Ok(messages)
    }

    /// Dies with a memo keyed wrongly: the newest read of the edited README.md is cut, or a
    /// request's bytes differ from a parse made fresh for each request.
    #[test]
    fn a_memo_leaves_every_request_of_a_recorded_session_byte_identical() -> Fallible {
        let messages = recorded_with_an_edit()?;
        let ends = (0..messages.len())
            .filter(|at| matches!(messages.get(*at), Some(AgentMessage::Assistant { .. })));
        let ends: Vec<usize> = ends.collect();
        let views = |fresh: bool| -> Result<Vec<Vec<AgentMessage>>, Box<dyn std::error::Error>> {
            let cut = std::sync::Arc::new(std::sync::Mutex::new(Cut::default()));
            let overlay =
                super::Overlay::new(std::sync::Arc::clone(&cut), Box::new(|| None), opus()?);
            let mut sent = Vec::new();
            for end in &ends {
                if fresh {
                    cut.lock().map_err(|_| "poisoned")?.sections = super::Sections::default();
                }
                sent.push(overlay.view(messages.get(..*end).unwrap_or_default().to_vec()));
            }
            Ok(sent)
        };
        let sent = views(false)?;
        assert_eq!(sent, views(true)?);
        let last = serde_json::to_string(sent.last().ok_or("no request")?)?;
        assert!(
            last.contains("[reclaimed: read `docs/ARCHITECTURE.md`"),
            "an aged read is cut"
        );
        assert!(
            last.contains(r"[README.md#DDA9]\n1:# Yi"),
            "the edited file's newest read stays whole"
        );
        let (memo, mut named) = (super::Sections::default(), 0);
        for message in &messages {
            if let AgentMessage::ToolResult { content, .. } = message {
                let paths = memo.paths(content);
                assert_eq!(paths, super::Sections::default().paths(content));
                named += usize::from(!paths.is_empty());
            }
        }
        assert_eq!(named, 11, "the edit and all ten reads name a file");
        Ok(())
    }

    /// Dies with the parse outside the lock: sixteen readers of one new result parse it sixteen
    /// times. The same call's text edited is a new text and parses again.
    #[test]
    fn sixteen_readers_parse_one_new_result_once_and_an_edit_reparses() -> Fallible {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let sections = std::sync::Arc::new(super::Sections::default());
        let parses = std::sync::Arc::new(AtomicUsize::new(0));
        let text = |head: &str| {
            vec![Content::Text {
                text: format!("{head}\n1:fn tidy() {{}}"),
                text_signature: None,
            }]
        };
        let gate = std::sync::Arc::new(std::sync::Barrier::new(16));
        let readers: Vec<_> = (0..16)
            .map(|_| {
                let (sections, parses, gate) = (sections.clone(), parses.clone(), gate.clone());
                let content = text("[src/lib.rs#BDF3]");
                std::thread::spawn(move || {
                    gate.wait();
                    sections.memo(&content, |_| {
                        parses.fetch_add(1, Ordering::SeqCst);
                        std::thread::sleep(std::time::Duration::from_millis(50));
                        vec!["src/lib.rs".to_owned()]
                    })
                })
            })
            .collect();
        for reader in readers {
            let paths = reader.join().map_err(|_| "a reader panicked")?;
            assert_eq!(paths, ["src/lib.rs"]);
        }
        assert_eq!(
            parses.load(Ordering::SeqCst),
            1,
            "one new result, one parse"
        );
        assert_eq!(sections.paths(&text("[src/main.rs#11AA]")), ["src/main.rs"]);
        assert_eq!(sections.paths(&text("[src/lib.rs#BDF3]")), ["src/lib.rs"]);
        Ok(())
    }

    #[test]
    fn read_notices_are_not_patch_sections() -> Fallible {
        use yi_tools::Tool;

        let dir = crate::scratch::Scratch::new("yi-reclaim-read-notices")?;
        let body: String = (1..=2_001)
            .map(|line| format!("pub fn row{line}() {{}}\n"))
            .collect();
        std::fs::write(dir.join("lib.rs"), body)?;
        let read = yi_tools::hashline::tool::HashlineReadTool::new(
            yi_tools::hashline::tool::shared_hashline_state(),
        )
        .execute(
            serde_json::Map::from_iter([("path".to_owned(), json!("lib.rs"))]),
            &yi_tools::ToolContext::new(dir.to_path_buf()),
        );
        assert!(!read.is_error);
        let text = yi_types::message::join_text(&read.result.content, "");
        assert!(text.contains("[showing lines 1-2000 of 2001"));
        assert!(text.contains("[skeleton:"));
        assert_eq!(
            super::Sections::default().paths(&read.result.content),
            ["lib.rs"]
        );
        Ok(())
    }

    fn model(
        (id, name, max_tokens): (&str, &str, u64),
        [input, output, cache_read, cache_write]: [f64; 4],
    ) -> Result<Model, serde_json::Error> {
        serde_json::from_value(json!({
            "id": id, "name": name, "api": "anthropic-messages",
            "provider": "anthropic", "baseUrl": "https://api.anthropic.com", "reasoning": true,
            "input": ["text"], "contextWindow": 1_000_000, "maxTokens": max_tokens,
            "cost": {"input": input, "output": output, "cacheRead": cache_read, "cacheWrite": cache_write}
        }))
    }

    /// Opus 5.5's catalog rates: a read costs a tenth of input, a write 1.25 times it.
    fn opus() -> Result<Model, serde_json::Error> {
        model(("claude-opus-5-5", "Opus", 128_000), [5.0, 25.0, 0.5, 6.25])
    }

    /// Sonnet 5.5's catalog rates: a read costs a twentieth of input, a write 1.25 times it, so a
    /// rewrite costs twenty-five reads.
    fn sonnet() -> Result<Model, serde_json::Error> {
        model(
            ("claude-sonnet-5-5", "Sonnet", 64_000),
            [2.0, 10.0, 0.1, 2.5],
        )
    }

    fn unpriced() -> Result<Model, serde_json::Error> {
        model(("subscription", "Subscription", 64_000), [0.0; 4])
    }

    /// A reply's time as Pi's own writer stamps it (`v4-golden.jsonl`); `at` counts from it.
    const REPLIED: u64 = 1_765_000_000_001;

    fn assistant(calls: &[(String, &str, serde_json::Value)], at: u64) -> AgentMessage {
        let content = calls.iter().map(|(id, name, args)| Content::ToolCall {
            id: id.clone(),
            name: (*name).to_owned(),
            arguments: args.as_object().cloned().unwrap_or_default(),
            thought_signature: None,
            namespace: None,
        });
        let mut message = yi_ai::faux::faux_assistant_message(
            content.collect(),
            yi_types::message::StopReason::ToolUse,
        );
        if let AgentMessage::Assistant {
            timestamp, usage, ..
        } = &mut message
        {
            (*timestamp, usage.input) = (REPLIED.saturating_add(at), 1_000);
        }
        message
    }

    fn result(id: &str, name: &str, chars: usize) -> AgentMessage {
        AgentMessage::ToolResult {
            tool_call_id: id.to_owned(),
            tool_name: name.to_owned(),
            content: vec![Content::Text {
                text: "build output line\n".repeat(chars / 18),
                text_signature: None,
            }],
            details: None,
            usage: None,
            added_tool_names: None,
            is_error: false,
            timestamp: 0,
        }
    }

    /// Requests `turns`, each a bash call answered with `chars` chars, a minute apart.
    fn calls(turns: std::ops::Range<usize>, chars: usize, messages: &mut Vec<AgentMessage>) {
        for turn in turns {
            let id = format!("call-{turn}");
            let args = json!({"command": format!("cargo build -p crate{turn}")});
            messages.push(assistant(
                &[(id.clone(), "bash", args)],
                60_000 * turn as u64,
            ));
            messages.push(result(&id, "bash", chars));
        }
    }

    /// `turns` requests, each a bash call answered with 10,000 chars, a minute apart.
    fn bash_session(turns: usize) -> Vec<AgentMessage> {
        let mut messages = vec![AgentMessage::user_input(
            UserContent::Text("fix the build".to_owned()),
            0,
        )];
        calls(0..turns, 10_000, &mut messages);
        messages
    }

    /// One 10,000-char result up front, then `tail` requests answered with results too small to
    /// cut: the only cut is the first result, and everything after it is rewritten.
    fn one_big_then_small(tail: usize) -> Vec<AgentMessage> {
        let mut messages = bash_session(1);
        calls(1..tail + 1, 300, &mut messages);
        for message in &mut messages {
            if let AgentMessage::Assistant { usage, .. } = message {
                usage.cache_read = 5_000;
            }
        }
        messages
    }

    /// Incident: 34 cuts in 211 Sonnet requests each rewrote the conversation. A cut that pays
    /// at ten reads (the unpriced fallback) does not pay at this route's own twenty-five.
    #[test]
    fn a_cut_is_priced_by_the_routes_own_write_over_read_ratio() -> Fallible {
        let messages = one_big_then_small(39);
        let now = REPLIED + 60_000 * 39 + 1_000;
        let record = decide(&messages, &Cut::default(), &unpriced()?, now).ok_or("no cut")?;
        assert_eq!(record.reason, "breakeven");
        assert_eq!(decide(&messages, &Cut::default(), &sonnet()?, now), None);
        Ok(())
    }

    /// A long session has no more requests left than a young one: past forty, savings stop growing.
    #[test]
    fn a_long_session_does_not_make_a_cut_easier_to_pay_for() -> Fallible {
        let messages = one_big_then_small(399);
        let now = REPLIED + 60_000 * 399 + 1_000;
        assert_eq!(decide(&messages, &Cut::default(), &unpriced()?, now), None);
        Ok(())
    }

    /// The replay that set the rule: cutting every turn rewrote the cache each time, so the
    /// tokens a cut drops must outweigh ten reads of everything after it: fourteen requests in
    /// that does not hold, thirty does.
    #[test]
    fn a_cut_waits_until_the_tokens_it_saves_beat_ten_reads_of_the_rewrite() -> Fallible {
        let model = opus()?;
        let now = REPLIED + 60_000 * 13 + 1_000;
        assert_eq!(
            decide(&bash_session(14), &Cut::default(), &model, now),
            None
        );
        let now = REPLIED + 60_000 * 29 + 1_000;
        let record = decide(&bash_session(30), &Cut::default(), &model, now).ok_or("no cut")?;
        assert_eq!(record.reason, "breakeven");
        assert_eq!(record.items.len(), 25, "results older than five requests");
        assert_eq!(
            record.items.first().map(|item| item.tool_call_id.as_str()),
            Some("call-0")
        );
        Ok(())
    }

    /// A session stored before replies were stamped has no reply time: it ages nothing from it.
    #[test]
    fn a_lapsed_cache_cuts_at_once_and_an_unstamped_reply_is_no_lapse() -> Fallible {
        let late = REPLIED + 60_000 * 9 + 6 * 60_000;
        let record = decide(&bash_session(10), &Cut::default(), &opus()?, late).ok_or("no cut")?;
        assert_eq!((record.reason.as_str(), record.items.len()), ("expired", 5));
        let mut unstamped = bash_session(10);
        for message in &mut unstamped {
            if let AgentMessage::Assistant { timestamp, .. } = message {
                *timestamp = 0;
            }
        }
        let record = decide(&unstamped, &Cut::default(), &opus()?, late);
        assert_eq!(record.map(|record| record.reason), None);
        Ok(())
    }

    /// An edit needs the line tags of the newest read of its file, and `ask_user` carries the
    /// user's own words: neither is ever cut, however old. A file only read is cut once aged.
    #[test]
    fn the_newest_read_of_an_edited_file_and_the_users_answers_stay() -> Fallible {
        let mut messages = vec![AgentMessage::user_input(
            UserContent::Text("tidy lib.rs".to_owned()),
            0,
        )];
        // Result heads as the read, edit and write tools print them (a `yi ask` dogfood session).
        let calls = [
            (
                "old-read",
                "read",
                "src/lib.rs",
                "[src/lib.rs#BDF3]\n1:fn tidy() {}",
            ),
            (
                "glob-read",
                "read",
                "src/**/*.rs",
                "[src/main.rs#11AA]\n1:fn main() {}",
            ),
            (
                "doc-read",
                "read",
                "docs/style.md",
                "[docs/style.md#F464]\n1:# style",
            ),
            ("asked", "ask_user", "", "Tabs."),
            (
                "new-read",
                "read",
                "src/lib.rs",
                "[src/lib.rs#BDF3]\n1:fn tidy() {}",
            ),
            (
                "edited",
                "edit",
                "",
                "[/work/src/lib.rs#9A42]\nupdated; first change at line 1\nsyntax: ok",
            ),
            (
                "written",
                "write",
                "src/main.rs",
                "[src/main.rs#22BB]\nWrote 13 bytes to src/main.rs",
            ),
        ];
        for (id, name, path, head) in calls {
            messages.push(assistant(
                &[(id.to_owned(), name, json!({"path": path}))],
                0,
            ));
            let mut answer = result(id, name, 20_000);
            if let AgentMessage::ToolResult { content, .. } = &mut answer {
                let head = Content::Text {
                    text: format!("{head}\n"),
                    text_signature: None,
                };
                content.insert(0, head);
            }
            messages.push(answer);
        }
        messages.extend(bash_session(6).into_iter().skip(1));
        let record =
            decide(&messages, &Cut::default(), &opus()?, REPLIED + 3_600_000).ok_or("no cut")?;
        let cut: Vec<&str> = record
            .items
            .iter()
            .map(|item| item.tool_call_id.as_str())
            .collect();
        assert!(
            cut.contains(&"old-read") && cut.contains(&"doc-read"),
            "{cut:?}"
        );
        let kept = ["asked", "new-read", "glob-read"];
        assert!(!kept.iter().any(|id| cut.contains(id)), "{cut:?}");
        Ok(())
    }

    /// A file a cell edited through `yi.tools.edit` needs its newest read's tags as much as one
    /// the model edited: the cell's `ipython` result names the edit only in its `calls` rows.
    #[test]
    fn the_newest_read_of_a_file_a_cell_edited_stays() -> Fallible {
        let mut messages = vec![AgentMessage::user_input(
            UserContent::Text("tidy lib.rs".to_owned()),
            0,
        )];
        messages.push(assistant(
            &[("lib-read".to_owned(), "read", json!({"path": "src/lib.rs"}))],
            0,
        ));
        let mut read = result("lib-read", "read", 20_000);
        if let AgentMessage::ToolResult { content, .. } = &mut read {
            content.insert(
                0,
                Content::Text {
                    text: "[src/lib.rs#BDF3]\n1:fn tidy() {}\n".to_owned(),
                    text_signature: None,
                },
            );
        }
        messages.push(read);
        messages.push(assistant(
            &[(
                "cell".to_owned(),
                "ipython",
                json!({"code": "await yi.tools.edit(patch=p)"}),
            )],
            0,
        ));
        let mut cell = result("cell", "ipython", 40);
        if let AgentMessage::ToolResult { details, .. } = &mut cell {
            *details = Some(json!({"calls": [
                {"tool": "edit", "ok": true, "paths": ["src/lib.rs"], "durationMs": 3}
            ]}));
        }
        messages.push(cell);
        messages.extend(bash_session(6).into_iter().skip(1));
        let record =
            decide(&messages, &Cut::default(), &opus()?, REPLIED + 3_600_000).ok_or("no cut")?;
        let cut: Vec<&str> = (record.items.iter())
            .map(|item| item.tool_call_id.as_str())
            .collect();
        assert!(cut.contains(&"call-0"), "{cut:?}");
        assert!(!cut.contains(&"lib-read"), "{cut:?}");
        Ok(())
    }

    /// A cut changes the prompt's bytes once; every later request, and a resumed process that
    /// folds the ledger, must send the same placeholder or the cache is rewritten each time.
    #[test]
    fn a_cut_renders_the_same_bytes_live_and_after_a_resume() -> Fallible {
        let messages = bash_session(14);
        let late = REPLIED + 60_000 * 13 + 6 * 60_000;
        let mut record = decide(&messages, &Cut::default(), &opus()?, late).ok_or("no cut")?;
        if let Some(first) = record.items.first_mut() {
            first.entry_id = Some("e-17".to_owned());
        }
        let mut live = Cut::default();
        live.add(&record);
        let shown = apply(messages.clone(), &live);
        let entry = Entry::Custom {
            id: "r1".to_owned(),
            custom_type: ReclaimRecord::TYPE.to_owned(),
            data: Some(serde_json::to_value(&record)?),
            parent_id: None,
            seq: 1,
            timestamp: 0,
        };
        assert_eq!(apply(messages, &Cut::fold(&[entry])), shown);
        let first = shown
            .get(2)
            .map(AgentMessage::plain_text)
            .unwrap_or_default();
        assert_eq!(
            first,
            "[reclaimed: bash `cargo build -p crate0` → none of 9990 chars kept; results older than 5 requests (reclaim.keep_turns) are cut once dropping them costs less than resending them (cut at request 14); get it: read(\"history://self/e-17\")]"
        );
        let kept = shown
            .get(26)
            .map(AgentMessage::plain_text)
            .unwrap_or_default();
        assert!(kept.starts_with("build output line"), "{kept}");
        Ok(())
    }

    /// Review of #1149: GLM's leaked calls reuse `leak-0` in every message, so a cut keyed by the
    /// id alone hid the newest result too. An abort just before a lapsed request hides no lapse.
    #[test]
    fn a_repeated_call_id_cuts_only_the_old_result_and_a_synthesized_reply_hides_no_lapse()
    -> Fallible {
        let mut messages = bash_session(14);
        let leak = |text: &str| AgentMessage::ToolResult {
            tool_call_id: "leak-0".to_owned(),
            tool_name: "bash".to_owned(),
            content: vec![Content::Text {
                text: text.repeat(800),
                text_signature: None,
            }],
            details: None,
            usage: None,
            added_tool_names: None,
            is_error: false,
            timestamp: 0,
        };
        let call = |at| {
            assistant(
                &[("leak-0".to_owned(), "bash", json!({"command": "ls"}))],
                at,
            )
        };
        messages.splice(1..1, [call(1), leak("old ")]);
        messages.extend([call(60_000 * 14), leak("new ")]);
        let now = REPLIED + 60_000 * 14 + 1_000;
        let model = opus()?;
        let record = decide(&messages, &Cut::default(), &model, now);
        assert_ne!(
            record.as_ref().map(|record| record.reason.as_str()),
            Some("expired")
        );
        let mut aborted =
            yi_ai::faux::faux_assistant_message(vec![], yi_types::message::StopReason::Aborted);
        if let AgentMessage::Assistant { timestamp, .. } = &mut aborted {
            *timestamp = REPLIED + 60_000 * 19;
        }
        messages.push(aborted);
        let late = REPLIED + 60_000 * 14 + 6 * 60_000;
        let record = decide(&messages, &Cut::default(), &model, late).ok_or("no cut")?;
        assert_eq!(record.reason, "expired");
        let mut cut = Cut::default();
        cut.add(&record);
        let shown = apply(messages, &cut);
        let leaks: Vec<String> = (shown.iter())
            .filter(|message| matches!(message, AgentMessage::ToolResult { tool_call_id, .. } if tool_call_id == "leak-0"))
            .map(AgentMessage::plain_text)
            .collect();
        assert!(
            leaks
                .first()
                .is_some_and(|old| old.starts_with("[reclaimed: bash `ls`")),
            "{leaks:?}"
        );
        assert!(
            leaks.get(1).is_some_and(|new| new.starts_with("new new")),
            "{leaks:?}"
        );
        Ok(())
    }

    fn with_usage(turns: usize, set: impl Fn(&mut yi_types::message::Usage)) -> Vec<AgentMessage> {
        let mut messages = bash_session(turns);
        for message in &mut messages {
            if let AgentMessage::Assistant { usage, .. } = message {
                set(usage);
            }
        }
        messages
    }

    /// Seen red under `let caches = priced;`: a $0 route that has read its cache waits for the
    /// token rule, while the same session with no cache reads cuts as soon as it clears the floor.
    #[test]
    fn a_zero_priced_route_waits_only_while_its_cache_is_read() -> Fallible {
        let mut model = opus()?;
        model.cost.input = serde_json::Number::from(0u64);
        model.cost.cache_read = serde_json::Number::from(0u64);
        model.cost.cache_write = serde_json::Number::from(0u64);
        let now = REPLIED + 60_000 * 13 + 1_000;
        let read = with_usage(14, |usage| usage.cache_read = 500);
        assert_eq!(decide(&read, &Cut::default(), &model, now), None);
        let cold = with_usage(14, |_| {});
        assert!(decide(&cold, &Cut::default(), &model, now).is_some());
        Ok(())
    }

    /// A warm cache still cuts in the original 2,000-token band, with or without cache reads.
    #[test]
    fn a_warm_cache_or_no_cache_cuts_above_two_thousand_tokens() -> Fallible {
        let mut model = opus()?;
        model.cost.input = serde_json::Number::from(0u64);
        model.cost.cache_read = serde_json::Number::from(0u64);
        model.cost.cache_write = serde_json::Number::from(0u64);
        for cache_read in [500, 0] {
            let mut messages = with_usage(17, |usage| usage.cache_read = cache_read);
            for message in &mut messages {
                if let AgentMessage::ToolResult { content, .. } = message {
                    *content = vec![Content::Text {
                        text: "build output line\n".repeat(70),
                        text_signature: None,
                    }];
                }
            }
            let now = REPLIED + 60_000 * 16 + 1_000;
            let record = decide(&messages, &Cut::default(), &model, now).ok_or("no cut")?;
            assert_eq!(record.reason, "breakeven");
        }
        Ok(())
    }

    #[test]
    fn an_hour_cache_waits_twice_as_long_for_its_rewrite() -> Fallible {
        let model = opus()?;
        let now = REPLIED + 60_000 * 21 + 1_000;
        let five = with_usage(22, |_| {});
        assert!(decide(&five, &Cut::default(), &model, now).is_some());
        let hour = with_usage(22, |usage| usage.cache_write1h = Some(1_000));
        assert_eq!(decide(&hour, &Cut::default(), &model, now), None);
        Ok(())
    }

    /// #1762: a route whose catalog prices cache at $0 (a subscription) still reads and rewrites
    /// tokens, so it cuts on the same rule as a priced one; an image result would lose its image
    /// to a text-only notice.
    #[test]
    fn a_route_priced_at_zero_cuts_by_tokens_and_an_image_result_stays() -> Fallible {
        let mut model = opus()?;
        model.cost.input = serde_json::Number::from(0u64);
        model.cost.cache_read = serde_json::Number::from(0u64);
        model.cost.cache_write = serde_json::Number::from(0u64);
        let mut messages = with_usage(30, |usage| usage.cache_read = 500);
        if let Some(AgentMessage::ToolResult { content, .. }) = messages.get_mut(2) {
            content.push(Content::Image {
                data: "aGk=".to_owned(),
                mime_type: "image/png".to_owned(),
            });
        }
        let now = REPLIED + 60_000 * 29 + 1_000;
        let record = decide(&messages, &Cut::default(), &model, now).ok_or("no cut")?;
        assert_eq!(record.reason, "breakeven");
        assert!(
            !record
                .items
                .iter()
                .any(|item| item.tool_call_id == "call-0"),
            "{record:?}"
        );
        assert_eq!(record.items.len(), 24, "{record:?}");
        Ok(())
    }

    /// Stage 4: the model's pin keeps a result the price rule would cut, its discard cuts one too
    /// young and short for the floors, and a later discard lets go of a pin.
    #[test]
    fn a_pin_keeps_an_old_result_and_a_discard_takes_a_young_one() -> Fallible {
        let mut messages = bash_session(30);
        messages.push(assistant(
            &[("young".to_owned(), "bash", json!({"command": "ls"}))],
            60_000 * 29,
        ));
        messages.push(result("young", "bash", 400));
        let note = |kind: &str, id: &str, chars: usize| AnnotationRecord {
            kind: kind.to_owned(),
            target: Some(Reclaimed {
                tool_call_id: id.to_owned(),
                hash: super::hash(&[Content::Text {
                    text: "build output line\n".repeat(chars / 18),
                    text_signature: None,
                }]),
                entry_id: None,
            }),
            call: None,
            note: None,
            extra: serde_json::Map::new(),
        };
        // Through the ledger, as a resume folds it; the last mark on `call-1` lets go of its pin.
        let marks = [
            ("pin", "call-0", 10_000),
            ("discard", "young", 400),
            ("pin", "call-1", 10_000),
            ("discard", "call-1", 10_000),
        ];
        let entries: Vec<Entry> = (marks.iter().enumerate())
            .map(|(seq, (kind, id, chars))| {
                Ok(Entry::Custom {
                    id: format!("a{seq}"),
                    custom_type: AnnotationRecord::TYPE.to_owned(),
                    data: Some(serde_json::to_value(note(kind, id, *chars))?),
                    parent_id: None,
                    seq: seq as u64,
                    timestamp: 0,
                })
            })
            .collect::<Result<_, serde_json::Error>>()?;
        let mut cut = Cut::fold(&entries);
        let record =
            decide(&messages, &cut, &opus()?, REPLIED + 60_000 * 29 + 1_000).ok_or("no cut")?;
        let ids: Vec<&str> = record
            .items
            .iter()
            .map(|item| item.tool_call_id.as_str())
            .collect();
        assert!(
            !ids.contains(&"call-0") && ids.contains(&"call-1"),
            "{ids:?}"
        );
        assert!(ids.contains(&"young"), "{ids:?}");
        cut.add(&record);
        let shown = apply(messages, &cut);
        let young = shown
            .last()
            .map(AgentMessage::plain_text)
            .unwrap_or_default();
        assert!(
            young.contains("; you discarded it (cut at request 31)"),
            "{young}"
        );
        Ok(())
    }

    pub(crate) fn image_result(id: &str, path: &str) -> AgentMessage {
        let metadata = std::fs::metadata(path).ok();
        let bytes = metadata.as_ref().map(std::fs::Metadata::len);
        let modified_ns = metadata.as_ref().and_then(|metadata| {
            let modified = metadata.modified().ok()?;
            let since_epoch = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
            u64::try_from(since_epoch.as_nanos()).ok()
        });
        AgentMessage::ToolResult {
            tool_call_id: id.to_owned(),
            tool_name: "ipython".to_owned(),
            content: vec![
                Content::Text {
                    text: format!("Loaded image into context: {path}"),
                    text_signature: None,
                },
                Content::Image {
                    data: "iVBORw0KGgo=".to_owned(),
                    mime_type: "image/png".to_owned(),
                },
            ],
            details: serde_json::from_value(json!({
                "status": "ok", "durationMs": 12, "attachments": 1,
                "attachmentMedia": [{
                    "mime_type": "image/png", "path": path,
                    "bytes": bytes, "modified_ns": modified_ns,
                }],
            }))
            .ok(),
            usage: None,
            added_tool_names: None,
            is_error: false,
            timestamp: 0,
        }
    }

    fn image_session(plots: &[String]) -> Vec<AgentMessage> {
        let turns = plots.len() + 6;
        let mut messages = vec![AgentMessage::user_input(
            UserContent::Text("plot the drift".to_owned()),
            0,
        )];
        for turn in 0..turns {
            let id = format!("call-{turn}");
            let code = json!({"code": format!("plot_drift({turn})")});
            messages.push(assistant(
                &[(id.clone(), "ipython", code)],
                60_000 * turn as u64,
            ));
            messages.push(match plots.get(turn) {
                Some(path) => image_result(&id, path),
                None => result(&id, "ipython", 10_000),
            });
        }
        messages
    }

    pub(crate) fn plots() -> Result<(crate::scratch::Scratch, Vec<String>), std::io::Error> {
        let dir = crate::scratch::Scratch::new("yi-reclaim-images")?;
        let paths: Vec<String> = (0..4)
            .map(|index| dir.join(format!("plot-{index}.png")).display().to_string())
            .collect();
        for path in &paths {
            std::fs::write(path, b"\x89PNG\r\n\x1a\n")?;
        }
        Ok((dir, paths))
    }

    #[test]
    fn a_same_size_overwrite_of_an_image_source_keeps_its_result_whole() -> Fallible {
        let (_scratch, paths) = plots()?;
        let messages = image_session(&paths);
        let late = REPLIED + 60_000 * 9 + 6 * 60_000;
        let record = decide(&messages, &Cut::default(), &opus()?, late).ok_or("no cut")?;
        assert!(
            (record.items.iter()).any(|item| item.tool_call_id == "call-0"),
            "an untouched source is cut: {:?}",
            record.items
        );
        std::fs::write(&paths[0], b"not png!")?;
        let file = std::fs::OpenOptions::new().write(true).open(&paths[0])?;
        file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(2_000_000_000))?;
        let record = decide(&messages, &Cut::default(), &opus()?, late).ok_or("no cut")?;
        assert!(
            !(record.items.iter()).any(|item| item.tool_call_id == "call-0"),
            "a same-size overwrite is a new picture: {:?}",
            record.items
        );
        Ok(())
    }

    /// Incident: every cut refused an image result for want of a placeholder that could say how
    /// to get the picture back, so a session plotting for an hour resent every plot.
    #[test]
    fn the_newest_three_image_results_are_never_cut_and_an_older_one_is() -> Fallible {
        let (_scratch, paths) = plots()?;
        let late = REPLIED + 60_000 * 9 + 6 * 60_000;
        let model = opus()?;
        let three = decide(&image_session(&paths[..3]), &Cut::default(), &model, late);
        let three = three.ok_or("no cut")?;
        let shown = ["call-0", "call-1", "call-2"];
        assert!(
            !three
                .items
                .iter()
                .any(|item| shown.contains(&item.tool_call_id.as_str())),
            "the newest three image results are never cut: {:?}",
            three.items
        );
        let mut four =
            decide(&image_session(&paths), &Cut::default(), &model, late).ok_or("no cut")?;
        let images: Vec<String> = (four.items.iter())
            .map(|item| item.tool_call_id.clone())
            .filter(|id| ["call-0", "call-1", "call-2", "call-3"].contains(&id.as_str()))
            .collect();
        let Some(item) = four
            .items
            .iter_mut()
            .find(|item| item.tool_call_id == "call-0")
        else {
            return Err("missing reclaimed image".into());
        };
        item.entry_id = Some("entry-0".to_owned());
        assert_eq!(images, ["call-0"], "only the oldest of four is cut");
        let mut cut = Cut::default();
        cut.add(&four);
        let shown = apply(image_session(&paths), &cut);
        let placeholder = shown
            .get(2)
            .map(AgentMessage::plain_text)
            .unwrap_or_default();
        assert_eq!(
            placeholder,
            format!(
                "[reclaimed: ipython → whole result dropped, none of {} chars kept, its image dropped; results older than 5 requests (reclaim.keep_turns) are cut once dropping them costs less than resending them (cut at request 10); get it: print(await attach_image({:?})); text: read(\"history://self/entry-0\")]",
                "Loaded image into context: ".len() + paths[0].chars().count(),
                paths[0]
            )
        );
        Ok(())
    }
    #[test]
    fn a_compressed_attachment_is_cut_from_its_original_file() -> Fallible {
        let (_scratch, paths) = plots()?;
        let mut messages = image_session(&paths);
        let Some(AgentMessage::ToolResult { content, .. }) = messages.get_mut(2) else {
            return Err("missing image result".into());
        };
        for part in content {
            if let Content::Image { data, mime_type } = part {
                *data = "/9j/2Q==".to_owned();
                *mime_type = "image/jpeg".to_owned();
            }
        }
        let record = decide(
            &messages,
            &Cut::default(),
            &opus()?,
            REPLIED + 60_000 * 9 + 6 * 60_000,
        )
        .ok_or("no cut")?;
        assert!(
            record
                .items
                .iter()
                .any(|item| item.tool_call_id == "call-0"),
            "a resized JPEG attached from a PNG source must not keep the result whole: {:?}",
            record.items
        );
        Ok(())
    }

    #[test]
    fn a_reclaimed_image_stays_when_its_file_changes_before_apply() -> Fallible {
        let (_scratch, paths) = plots()?;
        let late = REPLIED + 60_000 * 9 + 6 * 60_000;
        let messages = image_session(&paths);
        let record = decide(&messages, &Cut::default(), &opus()?, late).ok_or("no cut")?;
        let mut cut = Cut::default();
        cut.add(&record);
        let image = messages.get(2).ok_or("no image result")?;
        assert!(cut.cut_at(image).is_some(), "the oldest image is cut");
        std::fs::write(&paths[0], b"new image")?;
        assert_eq!(
            cut.cut_at(image),
            None,
            "a cut that goes out whole is no cut to the price or annotate"
        );
        let shown = apply(messages, &cut);
        let content = shown
            .get(2)
            .map(AgentMessage::plain_text)
            .unwrap_or_default();
        assert!(
            content.starts_with("Loaded image into context:"),
            "{content}"
        );
        Ok(())
    }

    #[test]
    fn a_reclaimed_image_stays_when_its_file_disappears_before_apply() -> Fallible {
        let (_scratch, paths) = plots()?;
        let late = REPLIED + 60_000 * 9 + 6 * 60_000;
        let messages = image_session(&paths);
        let record = decide(&messages, &Cut::default(), &opus()?, late).ok_or("no cut")?;
        let mut cut = Cut::default();
        cut.add(&record);
        let image = messages.get(2).ok_or("no image result")?;
        assert!(cut.cut_at(image).is_some(), "the oldest image is cut");
        std::fs::remove_file(&paths[0])?;
        let shown = apply(messages, &cut);
        let content = shown
            .get(2)
            .map(AgentMessage::plain_text)
            .unwrap_or_default();
        assert!(
            content.starts_with("Loaded image into context:"),
            "{content}"
        );
        Ok(())
    }

    #[test]
    fn an_image_whose_file_is_gone_is_never_cut() -> Fallible {
        let (_scratch, paths) = plots()?;
        let messages = image_session(&paths);
        let late = REPLIED + 60_000 * 9 + 6 * 60_000;
        let cut = |messages| -> Result<Vec<String>, Box<dyn std::error::Error>> {
            let record = decide(messages, &Cut::default(), &opus()?, late).ok_or("no cut")?;
            Ok(record
                .items
                .into_iter()
                .map(|item| item.tool_call_id)
                .collect())
        };
        assert!(
            cut(&messages)?.contains(&"call-0".to_owned()),
            "its file is there"
        );
        std::fs::remove_file(&paths[0])?;
        let ids = cut(&messages)?;
        assert!(!ids.contains(&"call-0".to_owned()), "{ids:?}");
        Ok(())
    }
}
