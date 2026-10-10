//! Incident: replaying one Opus session, a watermark that cut old tool results every turn lost
//! $316-466 in cache rewrites; a cut here is made only when the reads it saves pay for one.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};

use yi_types::entry::{CustomRecord, Entry};
use yi_types::message::{AgentMessage, Content};
use yi_types::model::{CACHE_DIAGNOSTIC, Model, Ttl};
use yi_types::reclaim::{AnnotationRecord, ReclaimRecord, Reclaimed};

const KEEP_TURNS: u64 = 5;
/// Below this many tokens a cut's rewrite and its notice cost more than the reads it saves.
const MIN_DROPPED: u64 = 2_000;
const MIN_RESULT_CHARS: usize = 1_200;
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

    fn get(&self, message: &AgentMessage) -> Option<&Mark> {
        find(&self.marks, message)
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

fn price(number: &serde_json::Number) -> f64 {
    number.as_f64().unwrap_or(0.0)
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

fn path_of(arguments: &Args) -> Option<&str> {
    arguments.get("path").and_then(serde_json::Value::as_str)
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

/// The results that are the newest read of their path, by index: an edit needs their tags.
pub(crate) fn newest_reads(calls: &[Option<(&str, &Args)>]) -> BTreeSet<usize> {
    let mut latest = BTreeMap::new();
    for (index, call) in calls.iter().enumerate() {
        if let Some(path) = call
            .filter(|(name, _)| *name == "read")
            .and_then(|(_, args)| path_of(args))
        {
            latest.insert(path, index);
        }
    }
    latest.into_values().collect()
}

/// Why no cut ever takes this result, whatever its age or the model's discard.
pub(crate) fn never_cut(message: &AgentMessage, newest_read: bool) -> Option<&'static str> {
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
    } else if !content
        .iter()
        .all(|part| matches!(part, Content::Text { .. }))
    {
        Some("a result with an image is never cut, since its placeholder could not say so")
    } else if newest_read {
        Some("the newest read of a file is never cut, since an edit needs its line tags")
    } else {
        None
    }
}

/// A new cut, when one pays; pure over the messages, the cut so far, the prices and the clock.
pub(crate) fn decide(
    messages: &[AgentMessage],
    cut: &Cut,
    model: &Model,
    now: u64,
) -> Option<ReclaimRecord> {
    let calls = calls(messages);
    let newest = newest_reads(&calls);
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
            .is_some_and(|at| turn.saturating_sub(*at) >= KEEP_TURNS);
        let marked = |map| find(map, message).is_some();
        let kept = never_cut(message, newest.contains(&index)).is_some()
            || cut.get(message).is_some()
            || marked(&cut.pins);
        // The model's discard waives the age and size floors, never the rest.
        let floors = (aged && chars(content) >= MIN_RESULT_CHARS) || marked(&cut.discards);
        floors && !kept
    };
    // A result already cut costs its placeholder, so a new cut's rewrite is the view's own size.
    let sizes: Vec<u64> = (messages.iter())
        .map(|message| match cut.get(message) {
            Some(_) => PLACEHOLDER_TOKENS,
            None => yi_context::estimate_message(message).0,
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
        if dropped < MIN_DROPPED {
            return None;
        }
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

/// Why a cut pays now, if it does: the cache lapsed, or the reads it saves over as many more
/// requests as were sent so far outweigh the next request's rewrite of everything after it.
fn reason(
    messages: &[AgentMessage],
    model: &Model,
    now: u64,
    (dropped, rewritten, turns): (u64, u64, u64),
) -> Option<&'static str> {
    // A synthesized reply (an abort before the first token, an error) carries no time or usage.
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
    let input = price(&model.cost.input);
    let read_rate = price(&model.cost.cache_read);
    let reads_seen = replies.iter().any(|(usage, _, _)| usage.cache_read > 0);
    let caches = read_rate > 0.0 || reads_seen;
    let lapsed = replies.first().is_some_and(|(_, at, diagnostics)| {
        let hour = (diagnostics.iter().flatten())
            .find(|note| note.diagnostic_type == CACHE_DIAGNOSTIC)
            .and_then(|note| note.details.as_ref()?.get("ttl")?.as_str())
            == Some(Ttl::Hour1.label());
        caches && now.saturating_sub(*at) > if hour { HOUR_MS } else { FIVE_MINUTES_MS }
    });
    if lapsed {
        return Some("expired");
    }
    let hour = replies
        .first()
        .is_some_and(|(usage, _, _)| usage.cache_write1h.unwrap_or(0) > 0);
    let write_rate = price(&model.cost.cache_write);
    // A route that caches at a rate its catalog omits reads at zero, so no cut ever pays there.
    let read = if caches { read_rate } else { input };
    let write = match (hour, write_rate > 0.0) {
        (true, _) => 2.0 * input,
        (false, true) => write_rate,
        (false, false) => input,
    };
    #[expect(clippy::cast_precision_loss, reason = "token counts far below 2^52")]
    let pays = dropped as f64 * read * turns as f64 > rewritten as f64 * (write - read);
    pays.then_some("breakeven")
}

/// Every cut result as its placeholder: kept nothing of how much, the rule, and the read
/// that restores it, all from the stored result, so the line is the same on every request.
pub(crate) fn apply(mut messages: Vec<AgentMessage>, cut: &Cut) -> Vec<AgentMessage> {
    if cut.marks.is_empty() {
        return messages;
    }
    let briefs: Vec<Option<String>> = (calls(&messages).into_iter())
        .map(|call| call.map(|(name, args)| brief(name, args)))
        .collect();
    for (message, call) in messages.iter_mut().zip(briefs) {
        let Some(mark) = cut.get(message) else {
            continue;
        };
        // A discard is refused once its result is cut, so this reads the same on every request.
        let rule = match find(&cut.discards, message) {
            Some(()) => "you discarded it".to_owned(),
            None => format!(
                "results older than {KEEP_TURNS} requests are cut once dropping them costs less than resending them"
            ),
        };
        if let AgentMessage::ToolResult { content, .. } = message {
            let call = call.as_deref().unwrap_or("a tool call");
            let restore = match &mark.entry {
                Some(entry) => format!("get it: read(\"history://self/{entry}\")"),
                None => "it is not in this session's store".to_owned(),
            };
            let text = format!(
                "[reclaimed: {call} → none of {} chars kept; {rule} (cut at request {}); {restore}]",
                chars(content),
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
mod tests {
    use super::{Cut, apply, decide};
    use serde_json::json;
    use yi_types::entry::{CustomRecord, Entry};
    use yi_types::message::{AgentMessage, Content, UserContent};
    use yi_types::model::Model;
    use yi_types::reclaim::{AnnotationRecord, ReclaimRecord, Reclaimed};

    type Fallible = Result<(), Box<dyn std::error::Error>>;

    /// Opus 5.5's catalog rates: a read costs a tenth of input, a write 1.25 times it.
    fn opus() -> Result<Model, serde_json::Error> {
        serde_json::from_value(json!({
            "id": "claude-opus-5-5", "name": "Opus", "api": "anthropic-messages",
            "provider": "anthropic", "baseUrl": "https://api.anthropic.com", "reasoning": true,
            "input": ["text"], "contextWindow": 1_000_000, "maxTokens": 128_000,
            "cost": {"input": 5, "output": 25, "cacheRead": 0.5, "cacheWrite": 6.25}
        }))
    }

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
            (*timestamp, usage.input) = (at, 1_000);
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

    /// `turns` requests, each a bash call answered with 10,000 chars, a minute apart.
    fn bash_session(turns: usize) -> Vec<AgentMessage> {
        let mut messages = vec![AgentMessage::user_input(
            UserContent::Text("fix the build".to_owned()),
            0,
        )];
        for turn in 0..turns {
            let id = format!("call-{turn}");
            let args = json!({"command": format!("cargo build -p crate{turn}")});
            messages.push(assistant(
                &[(id.clone(), "bash", args)],
                60_000 * turn as u64,
            ));
            messages.push(result(&id, "bash", 10_000));
        }
        messages
    }

    /// The replay that set the rule: cutting every turn lost money on Opus, where a cached token
    /// costs a tenth of input, so ten requests in a cut does not pay and thirty does.
    #[test]
    fn a_cut_waits_until_the_reads_it_saves_beat_the_rewrite() -> Fallible {
        let model = opus()?;
        let now = 60_000 * 9 + 1_000;
        assert_eq!(
            decide(&bash_session(10), &Cut::default(), &model, now),
            None
        );
        let now = 60_000 * 29 + 1_000;
        let record = decide(&bash_session(30), &Cut::default(), &model, now).ok_or("no cut")?;
        assert_eq!(record.reason, "breakeven");
        assert_eq!(record.items.len(), 25, "results older than five requests");
        assert_eq!(
            record.items.first().map(|item| item.tool_call_id.as_str()),
            Some("call-0")
        );
        Ok(())
    }

    #[test]
    fn a_lapsed_cache_cuts_at_once() -> Fallible {
        let late = 60_000 * 9 + 6 * 60_000;
        let record = decide(&bash_session(10), &Cut::default(), &opus()?, late).ok_or("no cut")?;
        assert_eq!((record.reason.as_str(), record.items.len()), ("expired", 5));
        Ok(())
    }

    /// An edit needs the line tags of the newest read of its file, and `ask_user` carries the
    /// user's own words: neither is ever cut, however old.
    #[test]
    fn the_newest_read_of_a_file_and_the_users_answers_stay() -> Fallible {
        let mut messages = vec![AgentMessage::user_input(
            UserContent::Text("tidy lib.rs".to_owned()),
            0,
        )];
        let calls = [
            ("old-read", "read", json!({"path": "src/lib.rs"})),
            ("asked", "ask_user", json!({"question": "Which style?"})),
            ("new-read", "read", json!({"path": "src/lib.rs"})),
        ];
        for (id, name, args) in calls {
            messages.push(assistant(&[(id.to_owned(), name, args)], 0));
            messages.push(result(id, name, 20_000));
        }
        messages.extend(bash_session(6).into_iter().skip(1));
        let record = decide(&messages, &Cut::default(), &opus()?, 3_600_000).ok_or("no cut")?;
        let cut: Vec<&str> = record
            .items
            .iter()
            .map(|item| item.tool_call_id.as_str())
            .collect();
        assert!(cut.contains(&"old-read"), "{cut:?}");
        assert!(
            !cut.contains(&"asked") && !cut.contains(&"new-read"),
            "{cut:?}"
        );
        Ok(())
    }

    /// A cut changes the prompt's bytes once; every later request, and a resumed process that
    /// folds the ledger, must send the same placeholder or the cache is rewritten each time.
    #[test]
    fn a_cut_renders_the_same_bytes_live_and_after_a_resume() -> Fallible {
        let messages = bash_session(10);
        let late = 60_000 * 9 + 6 * 60_000;
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
            "[reclaimed: bash `cargo build -p crate0` → none of 9990 chars kept; results older than 5 requests are cut once dropping them costs less than resending them (cut at request 10); get it: read(\"history://self/e-17\")]"
        );
        let kept = shown
            .get(12)
            .map(AgentMessage::plain_text)
            .unwrap_or_default();
        assert!(kept.starts_with("build output line"), "{kept}");
        Ok(())
    }

    /// Review of #1149: GLM's leaked calls reuse `leak-0` in every message, so a cut keyed by the
    /// id alone hid the newest result too; a synthesized reply's zero timestamp read as a lapse.
    #[test]
    fn a_repeated_call_id_cuts_only_the_old_result_and_a_synthesized_reply_is_no_lapse() -> Fallible
    {
        let mut messages = bash_session(9);
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
        messages.extend([call(60_000 * 9), leak("new ")]);
        let mut aborted =
            yi_ai::faux::faux_assistant_message(vec![], yi_types::message::StopReason::Aborted);
        if let AgentMessage::Assistant { timestamp, .. } = &mut aborted {
            *timestamp = 0;
        }
        messages.push(aborted);
        let now = 60_000 * 9 + 1_000;
        let model = opus()?;
        let record = decide(&messages, &Cut::default(), &model, now);
        assert_ne!(
            record.as_ref().map(|record| record.reason.as_str()),
            Some("expired")
        );
        let late = 60_000 * 9 + 6 * 60_000;
        let record = decide(&messages, &Cut::default(), &model, late).ok_or("no cut")?;
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

    /// A route that reads cache at a rate its catalog entry omits would price every cut as free,
    /// the per-turn watermark again; an image result would lose its image to a text-only notice.
    #[test]
    fn an_unpriced_cached_route_waits_for_a_lapse_and_an_image_result_stays() -> Fallible {
        let mut model = opus()?;
        model.cost.cache_read = serde_json::Number::from(0u64);
        model.cost.cache_write = serde_json::Number::from(0u64);
        let mut messages = bash_session(30);
        for message in &mut messages {
            if let AgentMessage::Assistant { usage, .. } = message {
                usage.cache_read = 500;
            }
        }
        assert_eq!(
            decide(&messages, &Cut::default(), &model, 60_000 * 29 + 1_000),
            None
        );
        if let Some(AgentMessage::ToolResult { content, .. }) = messages.get_mut(2) {
            content.push(Content::Image {
                data: "aGk=".to_owned(),
                mime_type: "image/png".to_owned(),
            });
        }
        let late = 60_000 * 29 + 6 * 60_000;
        let record = decide(&messages, &Cut::default(), &model, late).ok_or("no cut")?;
        assert_eq!(record.reason, "expired");
        assert!(
            !record
                .items
                .iter()
                .any(|item| item.tool_call_id == "call-0"),
            "{record:?}"
        );
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
        let record = decide(&messages, &cut, &opus()?, 60_000 * 29 + 1_000).ok_or("no cut")?;
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
}
