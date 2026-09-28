//! The memory store's journal (`<store>/ops.jsonl`) and the session's pointer to it.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::plan::canonical::{CanonicalError, Digest, canonical_bytes};

/// The `custom_type` of the session entry that points at one journal record.
pub const MEMORY_ENTRY_TYPE: &str = "memory";

/// One journal line. `op` is a string so an op a later build adds survives this one's
/// round trip; the bodies live in `objects/<hash hex>`, never here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub op: String,
    pub name: String,
    /// sha256 of the note's text as written to `<name>.md`.
    pub hash: Digest,
    /// Unix seconds.
    pub at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
    /// `sha256(previous record's digest ‖ canonical(this record without digest))`.
    pub digest: Digest,
}

impl MemoryRecord {
    /// # Errors
    /// The record does not serialize to canonical JSON.
    pub fn digest_of(&self, prev: Option<&Digest>) -> Result<Digest, CanonicalError> {
        let mut value = serde_json::to_value(self).map_err(|error| CanonicalError::Serialize {
            detail: error.to_string(),
        })?;
        if let Value::Object(map) = &mut value {
            map.remove("digest");
        }
        Ok(Digest::chained(prev, &canonical_bytes(&value)?))
    }

    /// # Errors
    /// The record does not serialize to canonical JSON.
    pub fn seal(mut self, prev: Option<&Digest>) -> Result<Self, CanonicalError> {
        self.digest = self.digest_of(prev)?;
        Ok(self)
    }

    /// # Errors
    /// The record does not serialize to canonical JSON.
    pub fn line(&self) -> Result<Vec<u8>, CanonicalError> {
        let value = serde_json::to_value(self).map_err(|error| CanonicalError::Serialize {
            detail: error.to_string(),
        })?;
        let mut bytes = canonical_bytes(&value)?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}

/// The `custom{memory}` entry payload: which store changed and how, never the note's text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryPointer {
    pub op: String,
    pub name: String,
    /// `repo` or `global`.
    pub scope: String,
    pub hash: Digest,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(op: &str) -> MemoryRecord {
        MemoryRecord {
            op: op.to_owned(),
            name: "buildhost-tmp-is-ram".to_owned(),
            hash: Digest::of(b"note"),
            at: 1_790_000_000,
            session: Some("01a0".to_owned()),
            extra: Map::new(),
            digest: Digest::of(b""),
        }
    }

    #[test]
    fn an_unknown_op_and_key_survive_a_round_trip() -> Result<(), Box<dyn std::error::Error>> {
        let line = r#"{"at":1,"by":"hand","digest":"sha256:0000000000000000000000000000000000000000000000000000000000000000","hash":"sha256:0000000000000000000000000000000000000000000000000000000000000000","name":"n","op":"compact"}"#;
        let parsed: MemoryRecord = serde_json::from_str(line)?;
        assert_eq!(parsed.op, "compact");
        assert_eq!(parsed.extra.get("by"), Some(&Value::from("hand")));
        let again = parsed.line()?;
        assert_eq!(String::from_utf8(again)?.trim_end(), line);
        Ok(())
    }

    #[test]
    fn the_digest_chains_on_the_previous_one() -> Result<(), Box<dyn std::error::Error>> {
        let first = record("save").seal(None)?;
        let second = record("read").seal(Some(&first.digest))?;
        assert_eq!(second.digest, second.digest_of(Some(&first.digest))?);
        assert_ne!(second.digest, record("read").seal(None)?.digest);
        Ok(())
    }
}
