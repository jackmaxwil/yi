//! Agent events into turns, messages and tool calls.

use std::borrow::Borrow;
use std::collections::{BTreeMap, BTreeSet};
use std::mem;

use serde_json::Value;
use yi_types::entry::Entry;
use yi_types::event::{AgentEvent, AssistantMessageEvent, RunEnd, Wait, apply, shown_failed};
use yi_types::message::{AgentMessage, Content, UserContent, join_text};
use yi_types::subagent::{CHILD_ENTRY, ChildTrail};

use crate::ids::{EntryId, Millis};
use crate::reduce::usage::Spend;
use crate::wire::frame::{Kept, MAX_FRAME};

/// Messages one transcript keeps; past it the oldest go, counted in [`Transcript::dropped`].
pub const MAX_MESSAGES: usize = 100_000;
/// Events of a kind newer than this app that one transcript keeps.
pub const MAX_UNREAD: usize = 32;
/// Characters of a message's first line used as a one-line summary.
const MAX_WORDS: usize = 200;
/// What one more content block counts for against a streaming message's size.
const BLOCK: usize = 64;

/// What one agent said and did, in order: a thread's own, or a child agent's. Messages the
/// server has stored come first, each with its entry id; those seen live and unstored follow.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Transcript {
    pub messages: Vec<Message>,
    /// Every tool call the messages name, by the call's id.
    pub tools: BTreeMap<CallId, ToolCall>,
    /// What holds the agent up besides the model and its tools: a retry, a compaction.
    pub wait: Option<Wait>,
    /// Why the last run ended.
    pub ended: Option<RunEnd>,
    /// Events of a kind newer than this app, as written; the oldest go first.
    pub unread: Vec<Value>,
    /// Messages let go to stay under [`MAX_MESSAGES`].
    pub dropped: u64,
    /// What the replies finished since the last whole replay have cost.
    pub since: Spend,
    /// Compactions stored on this branch, each counted once by its entry.
    pub compactions: u32,
    stored: BTreeSet<EntryId>,
    /// Bytes streamed into the message being written.
    streamed: usize,
}

/// One message of a transcript. A tool's result is not one: it settles its [`ToolCall`].
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    /// The session entry it was stored as, which rewind and fork name; none until stored.
    pub entry: Option<EntryId>,
    /// When it began: stamped here when seen live, else the server's time for its entry.
    pub at: Millis,
    /// When it was whole; none while it streams.
    pub done: Option<Millis>,
    pub body: Body,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    Message(Box<AgentMessage>),
    /// A message or entry of a kind newer than this app, as written.
    Raw(Value),
}

/// Where a tool call stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolState {
    /// Written by the model, not started.
    #[default]
    Queued,
    /// It waits for the user to allow it.
    Asking,
    Running,
    Done,
    /// It ran and fell short, was refused, or was never run.
    Failed,
    /// It waited on an approval this app no longer holds: asked before the app closed, and the
    /// server does not ask a new connection again.
    Lost,
}

/// The permission ask a tool call went through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gate {
    pub asked: Millis,
    pub answered: Option<Millis>,
    pub allowed: Option<bool>,
}

/// A tool call's id, as the model gave it; its result and its ask name it by this.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct CallId(String);

impl CallId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for CallId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ToolCall {
    pub id: CallId,
    pub name: String,
    /// The arguments as the model wrote them.
    pub args: Value,
    pub state: ToolState,
    /// What it printed: partial while it runs, whole once it ends.
    pub output: Vec<Content>,
    /// The result's own facts as written: exit code, patch, duration.
    pub details: Value,
    pub started: Option<Millis>,
    pub ended: Option<Millis>,
    pub gate: Option<Gate>,
}

/// One prompt from the user and everything the agent did in answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Turn {
    /// Counted from one.
    pub number: u32,
    /// Where its messages start in [`Transcript::messages`], and how many there are.
    pub first: usize,
    pub len: usize,
    pub started: Millis,
    /// When its last message was whole; none while one streams.
    pub ended: Option<Millis>,
    /// Tool calls made in it.
    pub steps: u32,
}

/// What a stored entry said beyond the transcript itself.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Fact {
    /// The permission ask for this tool call was answered.
    Permission(CallId),
    /// A child agent was started or ended.
    Child(ChildTrail, Millis),
}

impl ToolCall {
    /// How long it ran: the server's own measure when the result carries one.
    pub fn duration(&self) -> Option<Millis> {
        let measured = self.details.get("durationMs").and_then(Value::as_u64);
        let stamped = || Some(self.ended?.0.saturating_sub(self.started?.0));
        measured.or_else(stamped).map(Millis)
    }
}

/// What one answer spent, as the server counted it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spent {
    /// Dollars.
    pub cost: f64,
    /// Tokens the model wrote.
    pub output: u64,
}

impl Message {
    /// What this message spent: an answer's usage; none for any other kind.
    pub fn spent(&self) -> Option<Spent> {
        let Body::Message(message) = &self.body else {
            return None;
        };
        let AgentMessage::Assistant { usage, .. } = message.as_ref() else {
            return None;
        };
        Some(Spent {
            cost: usage.cost.total.as_f64().unwrap_or(0.0),
            output: u64::try_from(usage.output).unwrap_or(0),
        })
    }

    /// A prompt in the requester's own words opens a turn; what the host says does not.
    fn opens_turn(&self) -> bool {
        match &self.body {
            Body::Message(message) => match message.as_ref() {
                AgentMessage::User {
                    attribution,
                    timestamp,
                    ..
                } => attribution.reads_as_typed(*timestamp),
                _ => false,
            },
            Body::Raw(_) => false,
        }
    }

    pub(crate) fn calls(&self) -> impl Iterator<Item = &str> {
        let blocks = match &self.body {
            Body::Message(message) => match message.as_ref() {
                AgentMessage::Assistant { content, .. } => content.as_slice(),
                _ => &[],
            },
            Body::Raw(_) => &[],
        };
        blocks.iter().filter_map(|block| match block {
            Content::ToolCall { id, .. } => Some(id.as_str()),
            _ => None,
        })
    }
}

impl Transcript {
    /// The turns, found from the messages: each prompt the user typed starts one.
    pub fn turns(&self) -> Vec<Turn> {
        let mut turns: Vec<Turn> = Vec::new();
        for (index, message) in self.messages.iter().enumerate() {
            if message.opens_turn() {
                turns.push(Turn {
                    number: u32::try_from(turns.len().saturating_add(1)).unwrap_or(u32::MAX),
                    first: index,
                    len: 0,
                    started: message.at,
                    ended: None,
                    steps: 0,
                });
            }
            // What comes before the first prompt belongs to no turn.
            let Some(turn) = turns.last_mut() else {
                continue;
            };
            turn.len = turn.len.saturating_add(1);
            turn.ended = message.done;
            let steps = u32::try_from(message.calls().count()).unwrap_or(u32::MAX);
            turn.steps = turn.steps.saturating_add(steps);
        }
        turns
    }

    /// The stored user messages that begin with `text`. The server stores a prompt's
    /// blocks as one text, so a prompt with things attached begins with the words typed.
    pub(crate) fn prompted<'a>(&'a self, text: &'a str) -> impl Iterator<Item = &'a EntryId> {
        self.messages.iter().filter_map(move |message| {
            let Body::Message(body) = &message.body else {
                return None;
            };
            let AgentMessage::User { content, .. } = body.as_ref() else {
                return None;
            };
            let said = match content {
                UserContent::Text(said) => said.starts_with(text),
                UserContent::Blocks(blocks) => join_text(blocks, "").starts_with(text),
            };
            message.entry.as_ref().filter(|_| said)
        })
    }

    /// The first line of the last thing the agent said: what a finished thread is known by.
    pub fn last_words(&self) -> Option<String> {
        self.messages.iter().rev().find_map(|message| {
            let Body::Message(body) = &message.body else {
                return None;
            };
            let AgentMessage::Assistant { content, .. } = body.as_ref() else {
                return None;
            };
            let line = content.iter().rev().find_map(|block| match block {
                Content::Text { text, .. } => text.lines().find(|line| !line.trim().is_empty()),
                _ => None,
            })?;
            Some(line.chars().take(MAX_WORDS).collect())
        })
    }

    /// The call `id`, made if this is the first that is heard of it.
    fn call(&mut self, id: String, name: &str) -> &mut ToolCall {
        let call = self
            .tools
            .entry(CallId(id))
            .or_insert_with_key(|id| ToolCall {
                id: id.clone(),
                ..ToolCall::default()
            });
        if call.name.is_empty() {
            name.clone_into(&mut call.name);
        }
        call
    }

    /// Notes the calls an assistant message makes, so each has a row before it runs.
    fn name_calls(&mut self, message: &AgentMessage) {
        let AgentMessage::Assistant { content, .. } = message else {
            return;
        };
        for block in content {
            if let Content::ToolCall {
                id,
                name,
                arguments,
                ..
            } = block
            {
                self.call(id.clone(), name).args = Value::Object(arguments.clone());
            }
        }
    }

    /// A call's result, from the event that ends it or the message that stores it.
    fn settle(&mut self, id: String, name: &str, result: Settled, at: Millis) {
        let call = self.call(id, name);
        call.state = if shown_failed(result.failed, &result.details) {
            ToolState::Failed
        } else {
            ToolState::Done
        };
        call.output = result.output;
        call.details = result.details;
        call.ended.get_or_insert(at);
    }

    /// Folds one event of this agent's own stream.
    pub(crate) fn event(&mut self, event: AgentEvent, at: Millis) {
        match event {
            AgentEvent::MessageStart { message } => self.start(message, at),
            AgentEvent::MessageUpdate {
                assistant_message_event,
            } => self.grow(&assistant_message_event),
            AgentEvent::MessageEnd { message } => self.end(message, at),
            AgentEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            } => {
                let call = self.call(tool_call_id, &tool_name);
                call.args = args;
                call.state = ToolState::Running;
                call.started.get_or_insert(at);
            }
            AgentEvent::ToolExecutionUpdate {
                tool_call_id,
                tool_name,
                partial_result,
                ..
            } => {
                let call = self.call(tool_call_id, &tool_name);
                call.output = partial_result.content;
                call.started.get_or_insert(at);
                if call.state == ToolState::Queued {
                    call.state = ToolState::Running;
                }
            }
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                result,
                is_error,
            } => {
                let result = Settled {
                    output: result.content,
                    details: result.details,
                    failed: is_error,
                };
                self.settle(tool_call_id, &tool_name, result, at);
            }
            AgentEvent::PermissionRequested { tool_call_id, .. } => {
                self.gate(&tool_call_id, None, at);
            }
            AgentEvent::PermissionResolved {
                tool_call_id,
                allowed,
            } => {
                self.gate(&tool_call_id, Some(allowed), at);
            }
            AgentEvent::Wait { wait } => self.wait = wait,
            AgentEvent::AgentStart => (self.ended, self.wait) = (None, None),
            AgentEvent::AgentEnd { reason, .. } => (self.ended, self.wait) = (reason, None),
            AgentEvent::TurnStart
            | AgentEvent::TurnEnd { .. }
            | AgentEvent::ChildUpdate { .. }
            | AgentEvent::LandingState { .. }
            | AgentEvent::Other(_) => {}
        }
    }

    /// Folds one event of a child's stream. Incident: a child adopted mid-answer never sent
    /// that answer's start, and no stored entry follows a child's stream: its end brings it.
    pub(crate) fn child_event(&mut self, event: AgentEvent, at: Millis) {
        if let AgentEvent::MessageEnd { message } = &event
            && !self.open_as(message)
        {
            self.start(message.clone(), at);
        }
        self.event(event, at);
    }

    /// Whether the newest message is still open, and of `message`'s kind.
    fn open_as(&self, message: &AgentMessage) -> bool {
        self.messages.last().is_some_and(|last| {
            last.done.is_none()
                && matches!(&last.body, Body::Message(open)
                    if mem::discriminant(open.as_ref()) == mem::discriminant(message))
        })
    }

    /// Keeps an event this build has no type for, as written.
    pub(crate) fn keep(&mut self, raw: Value) {
        if self.unread.len() >= MAX_UNREAD {
            self.unread.remove(0);
        }
        self.unread.push(raw);
    }

    /// Notes that the call `id` waits on an ask, or how the ask went; false if the call is
    /// not one of this transcript's, as when a child agent made it.
    pub(crate) fn gate(&mut self, id: &str, allowed: Option<bool>, at: Millis) -> bool {
        let Some(call) = self.tools.get_mut(id) else {
            return false;
        };
        let known = call.gate.filter(|gate| gate.allowed.is_some());
        call.gate = known.or(Some(Gate {
            asked: call.gate.map_or(at, |gate| gate.asked),
            answered: allowed.map(|_| at),
            allowed,
        }));
        let waits = matches!(
            call.state,
            ToolState::Queued | ToolState::Running | ToolState::Lost
        );
        match allowed {
            None if waits => call.state = ToolState::Asking,
            // A denied call stays as it is until its result says it failed.
            Some(true) if call.state == ToolState::Asking => call.state = ToolState::Running,
            None | Some(_) => {}
        }
        true
    }

    fn start(&mut self, message: AgentMessage, at: Millis) {
        if matches!(message, AgentMessage::ToolResult { .. }) {
            return;
        }
        self.streamed = 0;
        self.messages.push(Message {
            entry: None,
            at,
            done: None,
            body: Body::Message(Box::new(message)),
        });
        self.trim();
    }

    fn grow(&mut self, event: &AssistantMessageEvent) {
        let Some(Message {
            done: None,
            body: Body::Message(message),
            ..
        }) = self.messages.last_mut()
        else {
            return;
        };
        let grows = match event {
            AssistantMessageEvent::TextDelta { delta, .. }
            | AssistantMessageEvent::ThinkingDelta { delta, .. } => delta.len(),
            AssistantMessageEvent::TextStart { .. }
            | AssistantMessageEvent::ThinkingStart { .. }
            | AssistantMessageEvent::ToolCallStart { .. }
            | AssistantMessageEvent::ToolCallEnd { .. } => BLOCK,
            AssistantMessageEvent::Start { .. }
            | AssistantMessageEvent::TextEnd { .. }
            | AssistantMessageEvent::ThinkingEnd { .. }
            | AssistantMessageEvent::ToolCallDelta { .. }
            | AssistantMessageEvent::Waiting { .. }
            | AssistantMessageEvent::Done { .. }
            | AssistantMessageEvent::Error { .. } => 0,
        };
        // Invariant: a message's end must fit one frame, so a stream past that is not one.
        let grown = self.streamed.saturating_add(grows);
        if u64::try_from(grown).is_ok_and(|grown| grown <= MAX_FRAME) {
            self.streamed = grown;
            apply(message, event);
        }
    }

    fn end(&mut self, message: AgentMessage, at: Millis) {
        if let AgentMessage::ToolResult {
            tool_call_id,
            tool_name,
            content,
            details,
            is_error,
            ..
        } = message
        {
            let result = Settled {
                output: content,
                details: details.unwrap_or_default(),
                failed: is_error,
            };
            return self.settle(tool_call_id, &tool_name, result, at);
        }
        self.name_calls(&message);
        // A message whose start was not seen is brought whole by the entry stored for it.
        if self.open_as(&message)
            && let Some(last) = self.messages.last_mut()
        {
            self.since.add(&message);
            last.body = Body::Message(Box::new(message));
            last.done = Some(at);
        }
    }

    /// Takes in one stored entry: a message joins the transcript once, under its entry id.
    /// `fresh` says the server just stored it, so a reply not seen live is new spending.
    pub(crate) fn absorb(&mut self, entry: Kept<Entry>, fresh: bool) -> Option<Fact> {
        let entry = match entry {
            Kept::Known(entry) => entry,
            Kept::Raw(raw) => {
                let id = raw.get("id").and_then(Value::as_str).map(EntryId::new);
                let at = raw.get("timestamp").and_then(Value::as_u64).unwrap_or(0);
                self.store(id?, Body::Raw(raw), Millis(at), false);
                return None;
            }
        };
        let at = Millis(entry.timestamp());
        match entry {
            Entry::Message { id, message, .. } => match message {
                AgentMessage::ToolResult {
                    tool_call_id,
                    tool_name,
                    content,
                    details,
                    is_error,
                    ..
                } => {
                    let result = Settled {
                        output: content,
                        details: details.unwrap_or_default(),
                        failed: is_error,
                    };
                    self.settle(tool_call_id, &tool_name, result, at);
                }
                message => {
                    self.name_calls(&message);
                    self.store(
                        EntryId::new(id),
                        Body::Message(Box::new(message)),
                        at,
                        fresh,
                    );
                }
            },
            Entry::Custom {
                custom_type,
                data: Some(data),
                ..
            } => return self.custom(&custom_type, data, at),
            Entry::Other(_) => {
                let id = EntryId::new(entry.id());
                if let Ok(raw) = serde_json::to_value(&entry) {
                    self.store(id, Body::Raw(raw), at, false);
                }
            }
            Entry::Compaction { id, .. } => {
                if self.stored.insert(EntryId::new(id)) {
                    self.compactions = self.compactions.saturating_add(1);
                }
            }
            Entry::Custom { data: None, .. }
            | Entry::ModelChange { .. }
            | Entry::ThinkingLevelChange { .. }
            | Entry::ActiveToolsChange { .. }
            | Entry::BranchSummary { .. } => {}
        }
        None
    }

    /// The custom entries this app reads: a permission's answer and a child agent's trail.
    fn custom(&mut self, kind: &str, data: Value, at: Millis) -> Option<Fact> {
        if kind == CHILD_ENTRY {
            return serde_json::from_value(data)
                .ok()
                .map(|trail| Fact::Child(trail, at));
        }
        if kind != "permission" {
            return None;
        }
        let call = data.get("toolCallId")?.as_str()?;
        let allowed = data.get("allowed")?.as_bool()?;
        // Only the answer is stored: the ask began once the call was made and the answer
        // before it was given, since calls are asked one at a time.
        let made = self
            .messages
            .iter()
            .rev()
            .find(|m| m.calls().any(|id| id == call));
        let made = made.map(|m| m.at);
        let before = self.tools.values().filter_map(|c| c.gate?.answered);
        let from = before.filter(|t| *t <= at).chain(made).max().unwrap_or(at);
        if let Some(open) = self.tools.get_mut(call).filter(|c| c.gate.is_none()) {
            open.gate = Some(Gate {
                asked: from,
                answered: None,
                allowed: None,
            });
        }
        self.gate(call, Some(allowed), at);
        Some(Fact::Permission(CallId::new(call)))
    }

    /// Puts a stored message in: once, as the live message it is, or before the unstored ones.
    fn store(&mut self, entry: EntryId, body: Body, at: Millis, fresh: bool) {
        if !self.stored.insert(entry.clone()) {
            return;
        }
        let unstored = self
            .messages
            .iter()
            .rposition(|message| message.entry.is_some())
            .map_or(0, |last| last.saturating_add(1));
        let live = self
            .messages
            .iter_mut()
            .skip(unstored)
            .find(|message| message.done.is_some() && message.body == body);
        match live {
            Some(live) => live.entry = Some(entry),
            None => {
                if let (true, Body::Message(message)) = (fresh, &body) {
                    self.since.add(message);
                }
                let stored = Message {
                    entry: Some(entry),
                    at,
                    done: Some(at),
                    body,
                };
                self.messages.insert(unstored, stored);
                self.trim();
            }
        }
    }

    /// Starts over from the branch as the server stored it, keeping what only this app
    /// measured: when a message was seen and where an unfinished call stands.
    pub(crate) fn rebuild(&mut self, entries: Vec<Kept<Entry>>) -> Vec<Fact> {
        let old = mem::take(self);
        (self.wait, self.ended, self.dropped) = (old.wait, old.ended, old.dropped);
        self.unread = old.unread;
        let facts = entries
            .into_iter()
            .filter_map(|entry| self.absorb(entry, false))
            .collect();
        let seen: BTreeMap<&EntryId, (Millis, Option<Millis>)> = old
            .messages
            .iter()
            .filter_map(|message| Some((message.entry.as_ref()?, (message.at, message.done))))
            .collect();
        for message in &mut self.messages {
            let stamps = message.entry.as_ref().and_then(|entry| seen.get(entry));
            if let Some((at, done)) = stamps {
                (message.at, message.done) = (*at, *done);
            }
        }
        for (id, was) in old.tools {
            let Some(call) = self.tools.get_mut(&id) else {
                continue;
            };
            call.started = was.started.or(call.started);
            call.gate = was.gate.or(call.gate);
            match call.ended {
                Some(_) => call.ended = was.ended.or(call.ended),
                None => (call.state, call.output) = (was.state, was.output),
            }
        }
        facts
    }

    /// Lets the oldest messages go once there are too many, with the calls they made. They
    /// go a batch at a time, so a long transcript is not shifted once for every message.
    fn trim(&mut self) {
        if self.messages.len() <= MAX_MESSAGES {
            return;
        }
        let over = self.messages.len().saturating_sub(MAX_MESSAGES);
        let batch = over.saturating_add(MAX_MESSAGES / 16);
        let gone: Vec<Message> = self.messages.drain(..batch).collect();
        for message in &gone {
            for call in message.calls() {
                self.tools.remove(call);
            }
            if let Some(entry) = &message.entry {
                self.stored.remove(entry);
            }
        }
        self.dropped = self
            .dropped
            .saturating_add(u64::try_from(gone.len()).unwrap_or(u64::MAX));
    }
}

/// A tool call's result, whichever frame brought it.
struct Settled {
    output: Vec<Content>,
    details: Value,
    failed: bool,
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use yi_types::message::{AgentMessage, UserContent};

    use super::{MAX_MESSAGES, Transcript};
    use crate::ids::Millis;
    use yi_types::event::AgentEvent;

    #[test]
    fn an_answer_says_what_it_cost_and_how_much_it_wrote() -> Result<(), Box<dyn Error>> {
        const ANSWER: &str = r#"{"role":"assistant","content":[],"api":"anthropic-messages","provider":"anthropic","model":"claude-opus-5-5","usage":{"input":0,"output":612,"cacheRead":0,"cacheWrite":0,"totalTokens":612,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0.38}},"stopReason":"stop","timestamp":0}"#;
        let of = |message: AgentMessage| super::Message {
            entry: None,
            at: Millis(0),
            done: None,
            body: super::Body::Message(Box::new(message)),
        };
        let spent = of(serde_json::from_str(ANSWER)?).spent();
        let spent = spent.ok_or("an answer has usage")?;
        assert_eq!((spent.output, spent.cost), (612, 0.38));
        let prompt = AgentMessage::user_input(UserContent::Text("hi".to_owned()), 0);
        assert_eq!(of(prompt).spent(), None, "a prompt spends nothing");
        Ok(())
    }

    #[test]
    fn a_transcript_lets_its_oldest_messages_go_at_the_cap() -> Result<(), Box<dyn Error>> {
        let mut transcript = Transcript::default();
        let prompt = |n: usize| AgentMessage::user_input(UserContent::Text(n.to_string()), 0);
        for n in 0..=MAX_MESSAGES {
            let message = prompt(n);
            transcript.event(AgentEvent::MessageStart { message }, Millis(1));
            let message = prompt(n);
            transcript.event(AgentEvent::MessageEnd { message }, Millis(2));
        }
        let (kept, dropped) = (transcript.messages.len(), transcript.dropped);
        assert!(kept <= MAX_MESSAGES, "{kept} messages are kept");
        assert!(dropped > 0, "nothing went at the cap");
        let all = u64::try_from(MAX_MESSAGES + 1)?;
        assert_eq!(
            u64::try_from(kept)? + dropped,
            all,
            "each one is kept or counted"
        );
        let first = transcript.messages.first().ok_or("no message")?;
        let oldest = prompt(usize::try_from(dropped)?);
        assert_eq!(
            first.body,
            super::Body::Message(Box::new(oldest)),
            "the oldest messages are the ones that went"
        );
        Ok(())
    }
}
