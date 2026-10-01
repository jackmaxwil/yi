mod assemble;
mod grid;
mod install;
pub(crate) mod orchestrate;
mod pack;
mod project;
mod telemetry;

use std::cell::OnceCell;
use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::Value;
use yi_types::message::{AgentMessage, UserContent};

use crate::Deliver;

pub use assemble::{PromptState, Rank, Slot, Trust};
pub(crate) use assemble::{fence, sanitize};
pub use install::{ExtOptions, install, narrow};
pub use orchestrate::{Features, Route, features, prefilter};
pub use pack::Pack;
pub use project::{
    TrustGate, content_hash, contributions, git_root, is_project_root, listed, resource_roots,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartReason {
    Fresh,
    Resume,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    SessionStart {
        cwd: PathBuf,
        reason: StartReason,
    },
    PromptSubmitted {
        prompt: String,
        repo_dirty: bool,
        named_paths: u32,
    },
    ToolCall {
        name: String,
        target: Option<PathBuf>,
    },
    ToolResult {
        name: String,
        exit: Option<i32>,
        files_matched: u32,
    },
    TurnEnd {
        turn: u32,
        tool_calls_this_turn: u32,
    },
    Usage {
        input: i64,
        cache_read: i64,
        cache_write: i64,
    },
    Compacted,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    AttachFragment {
        slot: Slot,
        text: String,
    },
    DetachFragment {
        slot: Slot,
    },
    /// Environment-sourced text. Never enters the trusted prefix: it renders in
    /// the yard behind a fence named by its hash, labeled with source and trust.
    AttachExternal {
        source: String,
        trust: Trust,
        text: String,
    },
    /// One line delivered with the next request, then part of the transcript.
    Remind {
        text: String,
    },
    Record {
        key: &'static str,
        value: Value,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventMask(u8);

impl EventMask {
    pub const SESSION_START: Self = Self(1);
    pub const PROMPT: Self = Self(2);
    pub const TOOL_CALL: Self = Self(4);
    pub const TOOL_RESULT: Self = Self(8);
    pub const TURN_END: Self = Self(16);
    pub const COMPACTED: Self = Self(32);
    pub const USAGE: Self = Self(64);

    pub const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    fn of(event: &Event) -> Self {
        match event {
            Event::SessionStart { .. } => Self::SESSION_START,
            Event::PromptSubmitted { .. } => Self::PROMPT,
            Event::ToolCall { .. } => Self::TOOL_CALL,
            Event::ToolResult { .. } => Self::TOOL_RESULT,
            Event::TurnEnd { .. } => Self::TURN_END,
            Event::Usage { .. } => Self::USAGE,
            Event::Compacted => Self::COMPACTED,
        }
    }

    fn covers(self, event: &Event) -> bool {
        self.0 & Self::of(event).0 != 0
    }
}

pub trait Extension: Send {
    fn name(&self) -> &'static str;
    fn interests(&self) -> EventMask;
    fn on(&mut self, event: &Event, out: &mut Vec<Effect>);
}

const STATE_ENTRY: &str = "ext_state";
const RECORD_ENTRY: &str = "ext_record";
/// A fragment attached after the first request rides the transcript as this custom entry,
/// which `yi_context::convert_to_llm` renders through the internal-context wrapper.
const FRAGMENT_ENTRY: &str = "fragment";

pub struct Host {
    extensions: Vec<Box<dyn Extension>>,
    state: PromptState,
    /// The bytes the first request sent; every later request sends the same (D310).
    frozen: OnceCell<String>,
    /// Slots attached after the freeze: their current text goes out again after a compaction.
    late: BTreeSet<Slot>,
    started: bool,
    turn: u32,
    tool_calls_this_turn: u32,
    mutated: bool,
    /// The one way the host puts a message in the transcript: a reminder line, or a fragment
    /// attached after the first request (D310). The session presents it at the next boundary.
    deliver: Option<Deliver>,
    cwd: PathBuf,
}

fn named_paths(prompt: &str) -> u32 {
    let count = prompt
        .split_whitespace()
        .filter(|token| {
            let token = token.trim_matches(|ch: char| !ch.is_ascii_graphic());
            token.contains('/') && !token.contains("://") && token.len() > 2
        })
        .count();
    u32::try_from(count).unwrap_or(u32::MAX)
}

impl Host {
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            extensions: Vec::new(),
            state: PromptState::default(),
            frozen: OnceCell::new(),
            late: BTreeSet::new(),
            started: false,
            turn: 0,
            tool_calls_this_turn: 0,
            mutated: false,
            deliver: None,
            cwd,
        }
    }

    pub fn register(&mut self, extension: Box<dyn Extension>) {
        self.extensions.push(extension);
    }

    pub fn set_deliver(&mut self, deliver: Deliver) {
        self.deliver = Some(deliver);
    }

    /// Before the first request the slot joins the prompt. After it, the text is delivered once
    /// as a `fragment` message and again after each compaction; the slot reaches the resume
    /// snapshot only through an extension effect, since this method holds no store.
    pub fn attach(&mut self, slot: Slot, text: String) -> bool {
        let message = self.frozen.get().is_some().then(|| fragment_message(&text));
        let changed = self.state.attach(slot.clone(), text);
        if changed && let Some(message) = message {
            self.late.insert(slot);
            self.send(message);
        }
        changed
    }

    fn send(&self, message: AgentMessage) {
        if let Some(deliver) = &self.deliver {
            deliver(message);
        }
    }

    /// A compaction drops every internal message, late fragments included, so each late slot's
    /// current text goes out again, unless the frozen prompt already says it (a mode that
    /// flipped back).
    fn redeliver(&self) {
        for slot in &self.late {
            if let Some(text) = self.state.slot_text(slot)
                && !self
                    .frozen
                    .get()
                    .is_some_and(|frozen| frozen.contains(text))
            {
                self.send(fragment_message(text));
            }
        }
    }

    pub fn state(&self) -> &PromptState {
        &self.state
    }

    /// The first rendering is the conversation's system prompt (D310): every request sends
    /// these bytes, and whatever attaches later goes out as a message instead.
    pub fn system_prompt(&self) -> String {
        self.frozen.get_or_init(|| self.state.assemble()).clone()
    }

    pub fn is_empty(&self) -> bool {
        self.state.is_empty()
    }

    pub fn turn(&self) -> u32 {
        self.turn
    }

    pub fn start(&mut self, store: Option<&yi_session::SharedSession>, resumed: bool) {
        if self.started {
            return;
        }
        self.started = true;
        if let Some(restored) = store.and_then(restore_state) {
            self.state = restored;
        }
        let reason = if resumed {
            StartReason::Resume
        } else {
            StartReason::Fresh
        };
        let cwd = self.cwd.clone();
        self.dispatch(&Event::SessionStart { cwd, reason }, store);
    }

    pub fn prompt_event(&self, prompt: &str) -> Event {
        Event::PromptSubmitted {
            prompt: prompt.to_owned(),
            repo_dirty: self.mutated,
            named_paths: named_paths(prompt),
        }
    }

    pub fn dispatch(&mut self, event: &Event, store: Option<&yi_session::SharedSession>) {
        match event {
            Event::ToolCall { name, .. } => {
                self.tool_calls_this_turn = self.tool_calls_this_turn.saturating_add(1);
                self.mutated |= name == "write" || name == "edit";
            }
            Event::TurnEnd { .. } => self.turn = self.turn.saturating_add(1),
            Event::Compacted => self.redeliver(),
            _ => {}
        }
        let mut effects: Vec<Effect> = Vec::new();
        for extension in &mut self.extensions {
            if extension.interests().covers(event) {
                extension.on(event, &mut effects);
            }
        }
        if matches!(event, Event::TurnEnd { .. }) {
            self.tool_calls_this_turn = 0;
        }
        self.apply(effects, store);
    }

    pub fn turn_end_event(&self) -> Event {
        Event::TurnEnd {
            turn: self.turn,
            tool_calls_this_turn: self.tool_calls_this_turn,
        }
    }

    fn apply(&mut self, effects: Vec<Effect>, store: Option<&yi_session::SharedSession>) {
        let mut changed = false;
        for effect in effects {
            match effect {
                Effect::AttachFragment { slot, text } => {
                    changed |= self.attach(slot, text);
                }
                Effect::DetachFragment { slot } => {
                    changed |= self.state.detach(&slot);
                }
                Effect::AttachExternal {
                    source,
                    trust,
                    text,
                } => {
                    // Every external attach fires at session start; a late one has no delivery
                    // (the yard's fence would be lost), so a debug build refuses it.
                    debug_assert!(
                        self.frozen.get().is_none(),
                        "an external attach after the first request reaches no request"
                    );
                    changed |= self.state.attach_external(&source, trust, &text);
                }
                Effect::Remind { text } => self.send(crate::session::user_message(&text)),
                Effect::Record { key, value } => record(store, key, &value),
            }
        }
        if changed && let Some(store) = store {
            persist_state(store, &self.state);
        }
    }
}

fn fragment_message(text: &str) -> AgentMessage {
    AgentMessage::Custom {
        custom_type: FRAGMENT_ENTRY.to_owned(),
        content: UserContent::Text(text.to_owned()),
        display: false,
        details: None,
        timestamp: yi_session::now_ms(),
    }
}

fn restore_state(store: &yi_session::SharedSession) -> Option<PromptState> {
    let entries = yi_session::lock_session(store)
        .find_entries(&yi_session::EntryQuery {
            order: yi_session::EntryOrder::NewestFirst,
            ..yi_session::EntryQuery::default()
        })
        .ok()?;
    entries.iter().find_map(|entry| match entry {
        yi_types::entry::Entry::Custom {
            custom_type, data, ..
        } if custom_type == STATE_ENTRY => data.as_ref().and_then(PromptState::restore),
        _ => None,
    })
}

fn persist_state(store: &yi_session::SharedSession, state: &PromptState) {
    let _unavailable_store_only_costs_the_rehydrate =
        yi_session::lock_session(store).append_custom("main", STATE_ENTRY, Some(state.snapshot()));
}

fn record(store: Option<&yi_session::SharedSession>, key: &str, value: &Value) {
    let Some(store) = store else {
        return;
    };
    let line = serde_json::json!({"key": key, "value": value});
    let _telemetry_is_never_load_bearing =
        yi_session::lock_session(store).append_custom("main", RECORD_ENTRY, Some(line));
}
