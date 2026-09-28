//! The node card (plan section 3.4): one machine and the work it admits.

use std::collections::BTreeMap;
use std::num::NonZeroU8;

use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

/// `~/.yi/node.json`: computed on first use and read after, so an edit to it sticks; the
/// config's `node` overrides it field by field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeCard {
    pub name: String,
    pub always_on: bool,
    pub power: String,
    /// Live kernels this machine holds at once, whichever process owns them; a boot past
    /// them waits. Computed as the cores less one, 1 to 8.
    pub slots: NonZeroU8,
    pub capacity: NodeCapacity,
    /// `worktree`, and `container` when `docker version` answered as the card was computed.
    pub isolation: Vec<String>,
    pub price_per_hour: Number,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeCapacity {
    pub cpus: u32,
    pub mem_gb: u64,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}
