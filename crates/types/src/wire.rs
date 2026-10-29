use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use crate::entry::Entry;
use crate::record::LaneRecord;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HeaderKind {
    Header,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonlV4Header {
    pub kind: HeaderKind,
    pub version: u64,
    pub id: String,
    pub created_at: u64,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legacy_parent_session_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "fact", rename_all = "lowercase")]
pub enum Fact {
    Name {
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Label {
        target_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    /// Design §15.1: the session goal lives beside the header as a fact line,
    /// outside the entry tree, so compaction cannot lose it.
    Goal { goal: crate::goal::Goal },
    /// An older build's plan, which nothing reads; raw, so the line re-emits byte for byte.
    Plan { plan: Value },
    /// A `fact` this build does not know; kept as written.
    #[serde(untagged)]
    Other(UnknownFact),
}

/// The line's object less `kind` and `seq`, which the mutation carries.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(transparent)]
pub struct UnknownFact(Map<String, Value>);

impl<'de> Deserialize<'de> for UnknownFact {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        unknown_tag(deserializer, "fact", &["name", "label", "goal", "plan"]).map(Self)
    }
}

/// A line whose `kind` this build does not know; `seq` is read so the sequence stays whole.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(transparent)]
pub struct UnknownMutation(Map<String, Value>);

impl UnknownMutation {
    pub fn seq(&self) -> Option<u64> {
        self.0.get("seq").and_then(Value::as_u64)
    }
}

impl<'de> Deserialize<'de> for UnknownMutation {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        unknown_tag(deserializer, "kind", &["entry", "record", "lane", "fact"]).map(Self)
    }
}

/// Invariant: the typed variants are tried first, so a known tag reaching here did not decode.
fn unknown_tag<'de, D: Deserializer<'de>>(
    deserializer: D,
    tag: &str,
    known: &[&str],
) -> Result<Map<String, Value>, D::Error> {
    let raw = Map::deserialize(deserializer)?;
    match raw.get(tag).and_then(Value::as_str) {
        Some(name) if !known.contains(&name) => Ok(raw),
        _ => Err(serde::de::Error::custom(format!(
            "a known or missing {tag} that does not decode"
        ))),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[expect(
    clippy::large_enum_variant,
    reason = "wire DTOs mirror the JSON; variants are parse-then-drop, boxing buys nothing"
)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Mutation {
    Entry {
        #[serde(skip_serializing_if = "Option::is_none")]
        lane: Option<String>,
        #[serde(flatten)]
        entry: Entry,
    },
    Record {
        #[serde(flatten)]
        record: LaneRecord,
    },
    #[serde(rename_all = "camelCase")]
    Lane {
        seq: u64,
        lane: String,
        leaf_id: Option<String>,
    },
    Fact {
        seq: u64,
        #[serde(flatten)]
        fact: Fact,
    },
    /// A `kind` this build does not know; kept as written.
    #[serde(untagged)]
    Other(UnknownMutation),
}

impl Mutation {
    pub fn seq(&self) -> Option<u64> {
        match self {
            Self::Entry { entry, .. } => Some(entry.seq()),
            Self::Record { record } => Some(record.seq()),
            Self::Lane { seq, .. } | Self::Fact { seq, .. } => Some(*seq),
            Self::Other(unknown) => unknown.seq(),
        }
    }

    /// A line this build keeps as written and reads nothing from.
    pub fn is_unread(&self) -> bool {
        matches!(
            self,
            Self::Other(_)
                | Self::Entry {
                    entry: Entry::Other(_),
                    ..
                }
                | Self::Fact {
                    fact: Fact::Other(_),
                    ..
                }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStats {
    pub message_count: u64,
    pub cached_tokens: i64,
    pub uncached_tokens: i64,
    pub total_tokens: i64,
    pub cost_total: f64,
    /// A reply came back without usage, so `costTotal` is a lower bound; absent when none did.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unknown_usage: bool,
}

impl SessionStats {
    pub fn zero() -> Self {
        Self {
            message_count: 0,
            cached_tokens: 0,
            uncached_tokens: 0,
            total_tokens: 0,
            cost_total: 0.0,
            unknown_usage: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMetadata {
    pub id: String,
    pub created_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}
