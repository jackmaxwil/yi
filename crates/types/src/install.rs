//! The install receipt: where `yi update` may replace the binary.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `~/.yi/install.json`, written by the release install script and again after a
/// verified update. `prefix` is the directory that holds the `yi` binary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallReceipt {
    pub prefix: String,
    pub version: String,
    pub target: String,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}
