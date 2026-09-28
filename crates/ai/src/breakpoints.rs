//! One typed set of cache breakpoints for every request (D295): at most four, none naming
//! `LlmContext.transient`, which the adapters render only after [`encode`] has run.

use serde_json::{Value, json};
use yi_types::message::AgentMessage;
use yi_types::model::{LlmContext, Model, ToolChoice};

/// How long the entry a mark writes lives. Along the prompt a longer TTL must come first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ttl {
    Min5,
    Hour1,
}

/// Where a breakpoint sits: the end of the stable prefix (tools and system), or the last block
/// of the message at that index of the transformed `messages`. `transient` has no position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Position {
    Stable,
    /// The previous request's tail: a free exact-position read that also covers a lookback
    /// the appended blocks would otherwise overflow.
    PrevTail(usize),
    /// The last block before `transient`: where the next request reads from.
    Tail(usize),
}

impl Position {
    pub fn index(self) -> Option<usize> {
        match self {
            Self::Stable => None,
            Self::PrevTail(index) | Self::Tail(index) => Some(index),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Breakpoint {
    pub position: Position,
    pub ttl: Ttl,
}

/// What the request is for. Only a loop request writes a tail: a one-shot's is never read
/// again and a `tool_choice: none` final has already invalidated the messages tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reuse {
    Loop,
    OneShot,
    Final,
}

impl Reuse {
    /// Read from the typed request, not its content: a forced `none` is a final, a request
    /// with no tool table is a one-shot (title, compaction, branch summary), the rest loop.
    pub fn of(context: &LlmContext) -> Self {
        if context.tool_choice == Some(ToolChoice::None) {
            Self::Final
        } else if context.tools.as_ref().is_none_or(Vec::is_empty) {
            Self::OneShot
        } else {
            Self::Loop
        }
    }
}

/// The route's cache engine: a prior from the transport and the id (design §7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    /// Entries are written only at marks, `slots` of them; `hour` when a 1h TTL is priced.
    Breakpoint { slots: usize, hour: bool },
    /// The last mark snapshots the whole prompt (Gemini through OpenRouter), so a moving
    /// tail would write a new object every request: only the stable prefix is marked.
    Snapshot,
    /// The provider caches its own prefix. Marks change neither price nor hit rate, and
    /// still go out, so a route learned to be a breakpoint engine needs no new code.
    Prefix,
}

impl Engine {
    pub fn of(model: &Model) -> Self {
        let id = model.id.trim_start_matches('~');
        if model.api == "anthropic-messages"
            || id.starts_with("anthropic/")
            || id.starts_with("claude-")
        {
            return Self::Breakpoint {
                slots: SLOTS,
                hour: true,
            };
        }
        if id.contains("gemini") {
            return Self::Snapshot;
        }
        // OpenAI's explicit mode takes three marks beside its automatic one, at one fixed TTL.
        if id.starts_with("openai/")
            || model.provider == "openai"
            || model.provider == "openai-codex"
        {
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

/// The route as the breakpoints see it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheRoute {
    pub engine: Engine,
    /// The stable mark's TTL; the tail marks are always 5m, so the order along the prompt holds.
    pub stable_ttl: Ttl,
}

impl CacheRoute {
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

/// The most marks any engine takes.
pub const SLOTS: usize = 4;

/// Built only by [`Breakpoints::build`]: at most [`SLOTS`] breakpoints in prompt order, TTL never
/// rising, every message position inside the slice it was built over.
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
    /// `messages` is the transformed history the adapter renders, never `transient`; slots
    /// fill stable, tail, then prev_tail (the last user-role message ahead of the last reply).
    pub fn build(route: &CacheRoute, messages: &[AgentMessage], reuse: Reuse) -> Self {
        let stable = Breakpoint {
            position: Position::Stable,
            ttl: route.stable_ttl,
        };
        let mut marks = [Some(stable), None, None, None];
        if reuse != Reuse::Loop {
            return Self { marks };
        }
        let slots = route.engine.slots();
        let tail = messages.iter().rposition(is_tail_kind);
        let prev_tail = tail
            .and_then(|tail| {
                messages
                    .get(..tail)?
                    .iter()
                    .rposition(|message| matches!(message, AgentMessage::Assistant { .. }))
            })
            .and_then(|reply| messages.get(..reply)?.iter().rposition(is_tail_kind));
        let five = |position| {
            Some(Breakpoint {
                position,
                ttl: Ttl::Min5,
            })
        };
        match (prev_tail, tail) {
            (Some(prev), Some(tail)) if slots >= 3 => {
                marks[1] = five(Position::PrevTail(prev));
                marks[2] = five(Position::Tail(tail));
            }
            (_, Some(tail)) if slots >= 2 => {
                marks[1] = five(Position::Tail(tail));
            }
            _ => {}
        }
        Self { marks }
    }

    pub fn marks(&self) -> impl Iterator<Item = Breakpoint> + '_ {
        self.marks.iter().flatten().copied()
    }
}

/// The wire shape breakpoints are spelled in; every arm is matched, and none is a root mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    /// Anthropic Messages: `cache_control` on the last system block (the last tool when
    /// there is no system) and on the last block of the message a breakpoint names.
    AnthropicBlocks,
    /// openai-completions through OpenRouter: `cache_control` on the last part of the
    /// system message and of the message a breakpoint names; a string content becomes one text part.
    OpenRouterParts,
    /// openai-completions elsewhere and the Responses API: the provider places its own
    /// breakpoint, and no explicit mark is known to be accepted there (design §11).
    Automatic,
}

fn ephemeral(ttl: Ttl) -> Value {
    match ttl {
        Ttl::Min5 => json!({"type": "ephemeral"}),
        Ttl::Hour1 => json!({"type": "ephemeral", "ttl": "1h"}),
    }
}

fn mark_last_part(message: &mut Value, control: Value) {
    if let Some(text) = message["content"].as_str() {
        message["content"] = json!([{"type": "text", "text": text}]);
    }
    if let Some(part) = message["content"]
        .as_array_mut()
        .and_then(|parts| parts.last_mut())
    {
        part["cache_control"] = control;
    }
}

/// Spells the breakpoints into the rendered body: `origins[j]` is the `messages` index rendered as
/// body message `j` (`None` for the system message and anything synthesized).
pub fn encode(
    breakpoints: &Breakpoints,
    dialect: Dialect,
    params: &mut Value,
    origins: &[Option<usize>],
) {
    for mark in breakpoints.marks() {
        let control = ephemeral(mark.ttl);
        match (dialect, mark.position) {
            (Dialect::Automatic, _) => {}
            (Dialect::AnthropicBlocks, Position::Stable) => {
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
            (Dialect::OpenRouterParts, Position::Stable) => {
                if let Some(system) = params
                    .get_mut("messages")
                    .and_then(Value::as_array_mut)
                    .and_then(|messages| messages.first_mut())
                    .filter(|message| {
                        matches!(message["role"].as_str(), Some("system" | "developer"))
                    })
                {
                    mark_last_part(system, control);
                }
            }
            (
                Dialect::AnthropicBlocks | Dialect::OpenRouterParts,
                Position::PrevTail(index) | Position::Tail(index),
            ) => {
                let Some(at) = origins.iter().position(|origin| *origin == Some(index)) else {
                    continue;
                };
                if let Some(message) = params
                    .get_mut("messages")
                    .and_then(Value::as_array_mut)
                    .and_then(|messages| messages.get_mut(at))
                {
                    mark_last_part(message, control);
                }
            }
        }
    }
}
