use serde::{Deserialize, Serialize};

/// The `custom{fetch}` entry payload (proposal §3.2): the URL as fetched, the
/// sha256 of the served text, and which backing store served it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchRecord {
    pub url: String,
    pub hash: String,
    pub served_by: String,
}

pub const FETCH_ENTRY_TYPE: &str = "fetch";
