//! A channel (plan section 3.5): a named, append-only JSONL buffer with one home, fed by an
//! adapter and read by subscriptions. It is never authority for a message once delivered.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The largest `data` a channel keeps, serialized; bigger content travels as a `store://` or
/// path reference, and the home keeps a refusal naming this cap in its place.
pub const MESSAGE_MAX_BYTES: usize = 16 * 1024;

/// One line of `<name>.jsonl`: the home assigns `offset`, so order is total in a channel; the
/// source names `id` and `at`. `refused` stands in for `data` the home would not keep.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelEntry {
    pub offset: u64,
    pub id: String,
    pub at: u64,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub data: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// At least one bound is set: an entry past the newest `count` or older than `ageMs` may go,
/// once every subscription has acked it. The newest entry always stays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Retention {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub age_ms: Option<u64>,
}

/// `<name>.json` beside the buffer: the adapter URI feeding it, its retention, and each
/// subscription's acked offset, keyed by job id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelMeta {
    pub source: String,
    pub retention: Retention,
    #[serde(default)]
    pub acks: BTreeMap<String, u64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A job's subscription: `address` as written, `path` the buffer it reads, `filter` exact
/// `key=value` terms joined by `&` or a substring, `batch` the most messages one delivery carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelSub {
    pub address: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch: Option<u32>,
}

/// The key a delivery's stamp rides under in the todo it creates.
pub const CHANNEL_KEY: &str = "channel";

/// A delivery's stamp: the subscription and the message ids the todo stands for, so a
/// redelivery after a crash creates nothing twice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelStamp {
    pub job: String,
    pub ids: Vec<String>,
}
