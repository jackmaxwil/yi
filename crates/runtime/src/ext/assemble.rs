use std::collections::BTreeMap;

use serde_json::{Value, json};
use yi_types::model::SYSTEM_BLOCK_SEPARATOR;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rank {
    Identity,
    Doctrine,
    Mode,
    Lang,
    Protocol,
    Tool,
    User,
    Catalog,
    Schema,
}

impl Rank {
    fn label(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Doctrine => "doctrine",
            Self::Mode => "mode",
            Self::Lang => "lang",
            Self::Protocol => "protocol",
            Self::Tool => "tool",
            Self::User => "user",
            Self::Catalog => "catalog",
            Self::Schema => "schema",
        }
    }

    fn parse(label: &str) -> Option<Self> {
        [
            Self::Identity,
            Self::Doctrine,
            Self::Mode,
            Self::Lang,
            Self::Protocol,
            Self::Tool,
            Self::User,
            Self::Catalog,
            Self::Schema,
        ]
        .into_iter()
        .find(|rank| rank.label() == label)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Slot {
    pub rank: Rank,
    pub name: String,
}

impl Slot {
    pub fn new(rank: Rank, name: &str) -> Self {
        Self {
            rank,
            name: name.to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Trust {
    Granted,
    Untrusted,
}

impl Trust {
    fn label(self) -> &'static str {
        match self {
            Self::Granted => "granted",
            Self::Untrusted => "untrusted",
        }
    }
}

const FENCE_SENTINEL: &str = "<<<";
const FENCE_ESCAPE: &str = "<\\<<";

#[derive(Debug, Clone, Default)]
pub struct PromptState {
    slots: BTreeMap<Slot, String>,
    yard: BTreeMap<(Trust, String), String>,
    nonce: String,
}

impl PromptState {
    pub fn new(nonce: String) -> Self {
        Self {
            slots: BTreeMap::new(),
            yard: BTreeMap::new(),
            nonce,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty() && self.yard.is_empty()
    }

    pub fn attach(&mut self, slot: Slot, text: String) -> bool {
        let changed = self.slots.get(&slot).is_none_or(|current| current != &text);
        self.slots.insert(slot, text);
        changed
    }

    pub fn has(&self, slot: &Slot) -> bool {
        self.slots.contains_key(slot)
    }

    pub fn attach_external(&mut self, source: &str, trust: Trust, text: &str) -> bool {
        let key = (trust, source.to_owned());
        let clean = sanitize(text).into_owned();
        let changed = self.yard.get(&key).is_none_or(|current| current != &clean);
        self.yard.insert(key, clean);
        changed
    }

    pub fn yard_is_empty(&self) -> bool {
        self.yard.is_empty()
    }

    pub fn assemble(&self) -> String {
        let mut blocks: Vec<String> = Vec::new();
        let universal = self.render_slots(|rank| rank <= Rank::Doctrine);
        if !universal.is_empty() {
            blocks.push(universal);
        }
        let trusted = self.render_slots(|rank| rank > Rank::Doctrine);
        if !trusted.is_empty() {
            blocks.push(trusted);
        }
        let yard = self.render_yard();
        if !yard.is_empty() {
            blocks.push(yard);
        }
        blocks.join(SYSTEM_BLOCK_SEPARATOR)
    }

    fn render_slots(&self, keep: impl Fn(Rank) -> bool) -> String {
        self.slots
            .iter()
            .filter(|(slot, _)| keep(slot.rank))
            .map(|(_, text)| text.trim_end())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn render_yard(&self) -> String {
        let mut out = String::new();
        for ((trust, source), text) in &self.yard {
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(&format!(
                "{FENCE_SENTINEL}yi-external {} source=\"{}\" trust=\"{}\">>>\n{}\n{FENCE_SENTINEL}end-yi-external {}>>>",
                self.nonce,
                sanitize(source),
                trust.label(),
                text,
                self.nonce
            ));
        }
        out
    }

    pub fn snapshot(&self) -> Value {
        let slots: Vec<Value> = self
            .slots
            .iter()
            .map(|(slot, text)| json!({"rank": slot.rank.label(), "name": slot.name, "text": text}))
            .collect();
        let yard: Vec<Value> = self
            .yard
            .iter()
            .map(|((trust, source), text)| {
                json!({"trust": trust.label(), "source": source, "text": text})
            })
            .collect();
        json!({"nonce": self.nonce, "slots": slots, "yard": yard})
    }

    pub fn restore(snapshot: &Value) -> Option<Self> {
        let nonce = snapshot.get("nonce").and_then(Value::as_str)?.to_owned();
        let mut state = Self::new(nonce);
        for entry in snapshot
            .get("slots")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let rank = entry
                .get("rank")
                .and_then(Value::as_str)
                .and_then(Rank::parse);
            let name = entry.get("name").and_then(Value::as_str);
            let text = entry.get("text").and_then(Value::as_str);
            if let (Some(rank), Some(name), Some(text)) = (rank, name, text) {
                state.attach(Slot::new(rank, name), text.to_owned());
            }
        }
        for entry in snapshot
            .get("yard")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let source = entry.get("source").and_then(Value::as_str);
            let text = entry.get("text").and_then(Value::as_str);
            let trust = match entry.get("trust").and_then(Value::as_str) {
                Some("granted") => Trust::Granted,
                _ => Trust::Untrusted,
            };
            if let (Some(source), Some(text)) = (source, text) {
                state.attach_external(source, trust, text);
            }
        }
        Some(state)
    }
}

/// External text cannot close its own fence, forge a trust label, or split a
/// cached block: the sentinel is escaped and control characters are dropped.
fn sanitize(text: &str) -> std::borrow::Cow<'_, str> {
    let clean = |text: &str| {
        text.replace(FENCE_SENTINEL, FENCE_ESCAPE)
            .chars()
            .filter(|ch| !ch.is_control() || *ch == '\n' || *ch == '\t')
            .collect::<String>()
    };
    if text.contains(FENCE_SENTINEL)
        || text
            .chars()
            .any(|ch| ch.is_control() && ch != '\n' && ch != '\t')
    {
        std::borrow::Cow::Owned(clean(text))
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}
