use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The fields a relay routes a frame by, read in place; the frame itself travels as its bytes.
/// A field spelled with an escape fails the read, and the caller parses the whole frame.
#[derive(Debug, Default, Deserialize)]
pub struct AcpRoute<'a> {
    #[serde(borrow, default)]
    pub method: Option<&'a str>,
    /// Presence only, a null included, since presence is what makes a request.
    #[serde(default, deserialize_with = "present")]
    pub id: bool,
    #[serde(borrow, default)]
    pub params: Option<AcpRouteParams<'a>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRouteParams<'a> {
    #[serde(borrow, default)]
    pub session_id: Option<&'a str>,
    #[serde(borrow, default)]
    pub update: Option<AcpRouteUpdate<'a>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRouteUpdate<'a> {
    #[serde(borrow, default)]
    pub session_update: Option<&'a str>,
}

fn present<'de, D: serde::Deserializer<'de>>(value: D) -> Result<bool, D::Error> {
    serde::de::IgnoredAny::deserialize(value).map(|_| true)
}

/// The daemon's session ledger on disk, keyed by session id: what a
/// `session/list` without a cwd answers from after a restart.
#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonLedger {
    pub sessions: BTreeMap<String, DaemonLedgerEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonLedgerEntry {
    pub cwd: String,
    pub unseen: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_state: Option<String>,
    pub last_event_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// When the session was pinned to the top of the console's sidebar, which lists pins in
    /// this order; absent means unpinned. Not `pinned`: the v1 fixture keeps that as an unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_ms: Option<u64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// The spec forbids custom fields at the root of its types; Yi's ride `_meta.yi`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AcpMeta {
    pub yi: AcpYiMeta,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpYiMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The entry offset a resume replayed up to, for the next resume's `replayFrom`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replayed_to: Option<u64>,
    /// What the person typed in the entry a `before` fork left out, to ask again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefill: Option<String>,
}

/// `_yi/fork` params: a new session branched from `entryId`, leaving it out (`before`) or
/// keeping it (`at`); the source session is untouched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpForkParams {
    pub session_id: String,
    pub entry_id: String,
    #[serde(default)]
    pub position: AcpForkPosition,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpForkPosition {
    #[default]
    Before,
    At,
    #[serde(untagged)]
    Other(String),
}

/// `_yi/committed`: a user or assistant message with intervening active-main entries, so a
/// client can connect a live message through ancestors that are not visible in its transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcpCommitted {
    pub entry: crate::entry::Entry,
    /// Ancestors after the prior commit cursor, oldest first;
    /// [`AcpCommitted::entry`] is the suffix's final item.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ancestors: Vec<crate::entry::Entry>,
    /// Cut row after a capped ancestry suffix, naming the replay limits and the full-branch call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ancestors_notice: Option<String>,
}
