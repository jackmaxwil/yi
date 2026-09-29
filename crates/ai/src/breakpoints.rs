//! One typed set of cache breakpoints per request (D295): at most four, none naming
//! `LlmContext.transient`; the request paths accept only the [`Encoded`] body made here.

use std::fmt;
use std::ops::Deref;

use serde_json::{Value, json};
use yi_types::message::AgentMessage;
use yi_types::model::{Model, Reuse};

/// How long the entry a breakpoint writes lives. Along the prompt a longer TTL comes first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ttl {
    Min5,
    Hour1,
}

/// Where a breakpoint sits. A message position is an index into the transformed
/// `messages`; `transient` has none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Position {
    /// The end of the first system block, the universal prefix every session of the same
    /// identity shares; the first to give way when the other breakpoints fill the slots.
    UniversalEnd,
    /// The end of tools and system: the floor that survives any messages-tier invalidation.
    SystemEnd,
    /// The end of the run siblings share, a reader's partition (`shared_through`, D309, D314).
    SharedEnd(usize),
    /// The last user-role message ahead of the last reply: where the previous request's
    /// tail sat, a free read that also covers a lookback the appended blocks would overflow.
    PrevTail(usize),
    /// The last block before `transient`: where the next request reads from.
    Tail(usize),
}

impl Position {
    pub fn index(self) -> Option<usize> {
        match self {
            Self::UniversalEnd | Self::SystemEnd => None,
            Self::SharedEnd(index) | Self::PrevTail(index) | Self::Tail(index) => Some(index),
        }
    }

    /// Order along the prompt.
    pub fn rank(self) -> (u8, usize) {
        match self {
            Self::UniversalEnd => (0, 0),
            Self::SystemEnd => (1, 0),
            Self::SharedEnd(index) | Self::PrevTail(index) | Self::Tail(index) => (2, index),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Breakpoint {
    pub position: Position,
    pub ttl: Ttl,
}

/// The route's cache engine: a prior from the transport and the id (design §7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    /// Entries are written only at breakpoints, `slots` of them; `hour` when 1h is priced.
    Breakpoint { slots: usize, hour: bool },
    /// The last breakpoint snapshots the whole prompt (Gemini through OpenRouter), so a
    /// moving tail would write a new object every request: only the system end is marked.
    Snapshot,
    /// The provider caches its own prefix. Breakpoints change neither price nor hit rate,
    /// and still go out, so a route learned to be a breakpoint engine needs no new code.
    Prefix,
}

impl Engine {
    pub fn of(model: &Model) -> Self {
        Self::of_route(&model.api, &model.provider, &model.id)
    }

    /// The same prior from what an assistant record names, for a replay with no catalog.
    pub fn of_route(api: &str, provider: &str, id: &str) -> Self {
        let id = id.trim_start_matches('~');
        if api == "anthropic-messages" || id.starts_with("anthropic/") || id.starts_with("claude-")
        {
            return Self::Breakpoint {
                slots: SLOTS,
                hour: true,
            };
        }
        if id.contains("gemini") {
            return Self::Snapshot;
        }
        // OpenAI's explicit mode takes three breakpoints beside its automatic one, at one TTL.
        if id.starts_with("openai/") || provider == "openai" || provider == "openai-codex" {
            return Self::Breakpoint {
                slots: 3,
                hour: false,
            };
        }
        Self::Prefix
    }

    fn slots(self) -> usize {
        match self {
            Self::Breakpoint { slots, .. } => slots.min(SLOTS),
            Self::Snapshot => 1,
            Self::Prefix => SLOTS,
        }
    }
}

/// How a model's route is cached: its engine and the TTL the two system breakpoints carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CachePolicy {
    pub engine: Engine,
    /// The tail breakpoints are always 5m, so the order along the prompt holds.
    pub stable_ttl: Ttl,
}

impl CachePolicy {
    /// `hold_1h`: an interactive session keeps its stable prefix for an hour on an engine that
    /// prices one (D116). The adaptive rule that switches after an expiry miss is a later stage.
    pub fn of(model: &Model, hold_1h: bool) -> Self {
        let engine = Engine::of(model);
        let stable_ttl = match engine {
            Engine::Breakpoint { hour: true, .. } if hold_1h => Ttl::Hour1,
            _ => Ttl::Min5,
        };
        Self { engine, stable_ttl }
    }
}

/// The most breakpoints any engine takes.
pub const SLOTS: usize = 4;

/// Built only by [`Breakpoints::build`]: at most [`SLOTS`] breakpoints in prompt order, TTL
/// never rising, every message position inside the slice it was built over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Breakpoints {
    marks: [Option<Breakpoint>; SLOTS],
}

fn is_tail_kind(message: &AgentMessage) -> bool {
    matches!(
        message,
        AgentMessage::User { .. } | AgentMessage::ToolResult { .. }
    )
}

impl Breakpoints {
    /// `messages` is the transformed history, never `transient`; `shared` indexes it. Short of
    /// slots, it keeps the system end, tail, shared end, previous tail, universal end, in order.
    pub fn build(
        policy: &CachePolicy,
        messages: &[AgentMessage],
        reuse: Reuse,
        shared: Option<usize>,
    ) -> Self {
        let mut kept: Vec<Breakpoint> = Vec::with_capacity(SLOTS + 1);
        let mut keep = |position: Position, ttl| {
            let at = position.index();
            if at.is_none() || kept.iter().all(|mark| mark.position.index() != at) {
                kept.push(Breakpoint { position, ttl });
            }
        };
        keep(Position::SystemEnd, policy.stable_ttl);
        let (mut tail, mut prev_tail) = (None, None);
        if matches!(reuse, Reuse::Loop | Reuse::ReadOnly) {
            let last = messages.iter().rposition(is_tail_kind);
            prev_tail = last
                .and_then(|tail| {
                    messages
                        .get(..tail)?
                        .iter()
                        .rposition(|message| matches!(message, AgentMessage::Assistant { .. }))
                })
                .and_then(|reply| messages.get(..reply)?.iter().rposition(is_tail_kind));
            // A read-only request writes no tail of its own; its previous tail is the read.
            tail = last.filter(|_| reuse == Reuse::Loop);
        }
        let history = [
            tail.map(Position::Tail),
            shared
                .filter(|index| *index < messages.len())
                .map(Position::SharedEnd),
            prev_tail.map(Position::PrevTail),
        ];
        for position in history.into_iter().flatten() {
            keep(position, Ttl::Min5);
        }
        let slots = policy.engine.slots();
        if slots >= SLOTS {
            keep(Position::UniversalEnd, policy.stable_ttl);
        }
        kept.truncate(slots);
        kept.sort_by_key(|mark| mark.position.rank());
        let mut marks = [None; SLOTS];
        for (cell, mark) in marks.iter_mut().zip(kept) {
            *cell = Some(mark);
        }
        Self { marks }
    }

    pub fn marks(&self) -> impl Iterator<Item = Breakpoint> + '_ {
        self.marks.iter().flatten().copied()
    }
}

/// The wires that carry explicit breakpoints; every arm is matched, and none is a root mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    /// Anthropic Messages: `cache_control` on the first and last system blocks (the last tool
    /// when there is no system) and on the last block of the message a breakpoint names.
    AnthropicBlocks,
    /// openai-completions through OpenRouter: `cache_control` on the last text part of the
    /// system message and of a named message; the one-part system has no room for the universal end.
    OpenRouterParts,
}

/// A body the request paths accept: it left [`encode`] with its breakpoints spelled and its
/// per-request facts last, or [`Encoded::provider_prefix`] on a wire with no explicit mark.
#[derive(Clone, Debug)]
pub struct Encoded(Value);

impl Deref for Encoded {
    type Target = Value;

    fn deref(&self) -> &Value {
        &self.0
    }
}

impl fmt::Display for Encoded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Encoded {
    /// The wires with no explicit breakpoint (openai-completions off OpenRouter, the Responses
    /// API): the provider caches its own prefix. The per-request facts still render last.
    pub fn provider_prefix(mut params: Value, list: &str, transient: Vec<Value>) -> Self {
        append(&mut params, list, transient);
        Self(params)
    }

    /// For inspection; the request paths never take a bare body back.
    pub fn into_value(self) -> Value {
        self.0
    }
}

fn append(params: &mut Value, list: &str, transient: Vec<Value>) {
    if let Some(items) = params.get_mut(list).and_then(Value::as_array_mut) {
        items.extend(transient);
    }
}

fn ephemeral(ttl: Ttl) -> Value {
    match ttl {
        Ttl::Min5 => json!({"type": "ephemeral"}),
        Ttl::Hour1 => json!({"type": "ephemeral", "ttl": "1h"}),
    }
}

/// Marks the message's last block, or on OpenRouter its last text part: OpenRouter documents
/// `cache_control` on text parts only, so an image part never carries one, as in pi. A message
/// with no text part goes unmarked there; the previous tail still reads. Content is already
/// blocks or parts on both wires (D51).
fn mark_last_part(message: &mut Value, control: Value, dialect: Dialect) {
    let takes_mark = |part: &&mut Value| match dialect {
        Dialect::AnthropicBlocks => true,
        Dialect::OpenRouterParts => part["type"] == "text",
    };
    if let Some(part) = message["content"]
        .as_array_mut()
        .and_then(|parts| parts.iter_mut().rev().find(takes_mark))
    {
        part["cache_control"] = control;
    }
}

/// Spells the breakpoints into the body, then renders `transient` after them; `origins[j]`
/// is the `messages` index rendered as body message `j` (`None` for synthesized ones).
pub fn encode(
    breakpoints: &Breakpoints,
    dialect: Dialect,
    mut params: Value,
    origins: &[Option<usize>],
    transient: Vec<Value>,
) -> Encoded {
    match dialect {
        // D51: one shape every request, as the Messages adapter already renders. OpenRouter
        // merges adjacent user messages, and a string beside the part a mark made renders as
        // a different prompt once the mark moves on: live, request 3 of a tool loop read only
        // the system prefix (#742).
        Dialect::OpenRouterParts => {
            for message in params
                .get_mut("messages")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
                .filter(|message| message["role"] != "assistant")
            {
                if let Some(text) = message["content"].as_str() {
                    message["content"] = json!([{"type": "text", "text": text}]);
                }
            }
        }
        Dialect::AnthropicBlocks => {}
    }
    for mark in breakpoints.marks() {
        let control = ephemeral(mark.ttl);
        match (dialect, mark.position) {
            (Dialect::AnthropicBlocks, Position::UniversalEnd) => {
                // A lone block is the system end's already.
                if let Some(block) = params
                    .get_mut("system")
                    .and_then(Value::as_array_mut)
                    .filter(|blocks| blocks.len() > 1)
                    .and_then(|blocks| blocks.first_mut())
                {
                    block["cache_control"] = control;
                }
            }
            (Dialect::AnthropicBlocks, Position::SystemEnd) => {
                let has_system = params
                    .get("system")
                    .and_then(Value::as_array)
                    .is_some_and(|blocks| !blocks.is_empty());
                let key = if has_system { "system" } else { "tools" };
                if let Some(block) = params
                    .get_mut(key)
                    .and_then(Value::as_array_mut)
                    .and_then(|blocks| blocks.last_mut())
                {
                    block["cache_control"] = control;
                }
            }
            (Dialect::OpenRouterParts, Position::UniversalEnd) => {}
            (Dialect::OpenRouterParts, Position::SystemEnd) => {
                if let Some(system) = params
                    .get_mut("messages")
                    .and_then(Value::as_array_mut)
                    .and_then(|messages| messages.first_mut())
                    .filter(|message| {
                        matches!(message["role"].as_str(), Some("system" | "developer"))
                    })
                {
                    mark_last_part(system, control, dialect);
                }
            }
            (
                Dialect::AnthropicBlocks | Dialect::OpenRouterParts,
                Position::SharedEnd(index) | Position::PrevTail(index) | Position::Tail(index),
            ) => {
                let Some(at) = origins.iter().position(|origin| *origin == Some(index)) else {
                    continue;
                };
                if let Some(message) = params
                    .get_mut("messages")
                    .and_then(Value::as_array_mut)
                    .and_then(|messages| messages.get_mut(at))
                {
                    mark_last_part(message, control, dialect);
                }
            }
        }
    }
    append(&mut params, "messages", transient);
    Encoded(params)
}
