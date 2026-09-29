use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::canonical::{CanonicalError, Digest, canonical_bytes, canonical_digest};
use super::doc::{PlanId, TodoLabel, TodoStateName};

pub const PLAN_OP_ENTRY_TYPE: &str = "plan_op";

/// Ids on the wire are bounded: one line, no whitespace, at most this many bytes.
pub const ID_MAX_BYTES: usize = 128;

/// The `custom{plan_op}` entry payload: one applied op appended to the owning session. The
/// journal record below carries the same envelope, so every duration diffs `at` either way.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanOpRecord {
    pub plan: PlanId,
    pub op: String,
    pub actor: String,
    pub at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub todo: Option<TodoLabel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<TodoStateName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<TodoStateName>,
    /// How many todos the plan carried once the op had applied, so a discovery
    /// ratio needs no second pass over the files a superseded generation left.
    pub todos: u32,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdError {
    Zero { what: &'static str },
    Overflow { what: &'static str },
    Empty { what: &'static str },
    TooLong { what: &'static str, bytes: usize },
    Whitespace { what: &'static str, id: String },
}

impl std::fmt::Display for IdError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Zero { what } => write!(formatter, "{what} starts at 1, not 0"),
            Self::Overflow { what } => write!(formatter, "{what} cannot advance past its cap"),
            Self::Empty { what } => write!(formatter, "{what} is empty"),
            Self::TooLong { what, bytes } => {
                write!(formatter, "{what} of {bytes} bytes exceeds {ID_MAX_BYTES}")
            }
            Self::Whitespace { what, id } => write!(formatter, "{what} {id:?} contains whitespace"),
        }
    }
}

impl std::error::Error for IdError {}

/// Journal position, per root, starting at 1 and monotonic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct Seq(u64);

impl Seq {
    pub const FIRST: Self = Self(1);

    /// # Errors
    /// Zero.
    pub fn new(value: u64) -> Result<Self, IdError> {
        if value == 0 {
            return Err(IdError::Zero { what: "seq" });
        }
        Ok(Self(value))
    }

    /// # Errors
    /// The counter is at `u64::MAX`.
    pub fn next(self) -> Result<Self, IdError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(IdError::Overflow { what: "seq" })
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

impl TryFrom<u64> for Seq {
    type Error = IdError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<Seq> for u64 {
    fn from(seq: Seq) -> Self {
        seq.0
    }
}

impl std::fmt::Display for Seq {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// Attempts of one todo, starting at 1; a retry is a new attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct AttemptId(u32);

impl AttemptId {
    pub const FIRST: Self = Self(1);

    /// # Errors
    /// Zero.
    pub fn new(value: u32) -> Result<Self, IdError> {
        if value == 0 {
            return Err(IdError::Zero { what: "attempt" });
        }
        Ok(Self(value))
    }

    /// # Errors
    /// The counter is at `u32::MAX`.
    pub fn next(self) -> Result<Self, IdError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(IdError::Overflow { what: "attempt" })
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

impl Default for AttemptId {
    fn default() -> Self {
        Self::FIRST
    }
}

impl TryFrom<u32> for AttemptId {
    type Error = IdError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<AttemptId> for u32 {
    fn from(attempt: AttemptId) -> Self {
        attempt.0
    }
}

impl std::fmt::Display for AttemptId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

pub(crate) fn check_id(what: &'static str, id: &str) -> Result<(), IdError> {
    if id.is_empty() {
        return Err(IdError::Empty { what });
    }
    if id.len() > ID_MAX_BYTES {
        return Err(IdError::TooLong {
            what,
            bytes: id.len(),
        });
    }
    if id.chars().any(char::is_whitespace) {
        return Err(IdError::Whitespace {
            what,
            id: id.to_owned(),
        });
    }
    Ok(())
}

macro_rules! text_id {
    ($name:ident, $what:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            /// # Errors
            /// Empty, over [`crate::plan::ledger::ID_MAX_BYTES`], or containing whitespace.
            pub fn new(id: impl Into<String>) -> Result<Self, $crate::plan::ledger::IdError> {
                let id = id.into();
                $crate::plan::ledger::check_id($what, &id)?;
                Ok(Self(id))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = $crate::plan::ledger::IdError;

            fn try_from(id: String) -> Result<Self, Self::Error> {
                Self::new(id)
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> Self {
                id.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

text_id!(RequestId, "request id");
text_id!(EffectId, "effect id");

/// One journal line (plan section 5.3): the op envelope plus the request identity, the canonical
/// `args` the reducer replays, and the digest chained to the previous record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalRecord {
    #[serde(flatten)]
    pub record: PlanOpRecord,
    pub seq: Seq,
    pub request_id: RequestId,
    pub expected_revision: u64,
    pub attempt: Option<AttemptId>,
    pub args: Value,
    pub args_hash: Digest,
    pub program_hash: Option<Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Value>,
    pub digest: Digest,
}

impl JournalRecord {
    /// The digest this record must carry: `sha256(prev || canonical(record without digest))`.
    pub fn digest_of(&self, prev: Option<&Digest>) -> Result<Digest, CanonicalError> {
        let mut value = serde_json::to_value(self).map_err(|error| CanonicalError::Serialize {
            detail: error.to_string(),
        })?;
        if let Value::Object(map) = &mut value {
            map.remove("digest");
        }
        Ok(Digest::chained(prev, &canonical_bytes(&value)?))
    }

    /// Fill `args_hash` from `args` and `digest` from the chain, in that order (the digest covers
    /// the hash).
    pub fn seal(mut self, prev: Option<&Digest>) -> Result<Self, CanonicalError> {
        self.args_hash = canonical_digest(&self.args)?;
        self.digest = self.digest_of(prev)?;
        Ok(self)
    }

    /// The bytes of one journal line: canonical JSON plus a newline.
    pub fn line(&self) -> Result<Vec<u8>, CanonicalError> {
        let value = serde_json::to_value(self).map_err(|error| CanonicalError::Serialize {
            detail: error.to_string(),
        })?;
        let mut bytes = canonical_bytes(&value)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    /// True when the record recorded a refusal: nothing moved, `to` is absent.
    pub fn is_refusal(&self) -> bool {
        self.record.extra.contains_key("refusal")
    }
}

impl crate::entry::CustomRecord for PlanOpRecord {
    const TYPE: &'static str = PLAN_OP_ENTRY_TYPE;
}
