//! Canonical JSON and the digest the journal chains on (plan section 5.3).

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};

pub const DIGEST_PREFIX: &str = "sha256:";

/// A sha256 digest: 32 bytes in memory, `sha256:<64 lowercase hex>` on the wire.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Digest([u8; 32]);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DigestError {
    Length { text: String, hex_len: usize },
    Hex { text: String },
}

impl fmt::Display for DigestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Length { text, hex_len } => {
                write!(
                    formatter,
                    "digest {text:?} has {hex_len} hex digits, not 64"
                )
            }
            Self::Hex { text } => write!(formatter, "digest {text:?} is not lowercase hex"),
        }
    }
}

impl std::error::Error for DigestError {}

impl Digest {
    pub fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    /// The chained form: `sha256(prefix || bytes)`, the prefix being the previous digest's
    /// wire text, or nothing for the first record of a root.
    pub fn chained(prev: Option<&Digest>, bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        if let Some(prev) = prev {
            hasher.update(prev.to_string().as_bytes());
        }
        hasher.update(bytes);
        Self(hasher.finalize().into())
    }

    /// # Errors
    /// The text is not `sha256:` plus 64 lowercase hex digits (the prefix may be omitted).
    pub fn parse(text: &str) -> Result<Self, DigestError> {
        let hex = text.strip_prefix(DIGEST_PREFIX).unwrap_or(text);
        if hex.len() != 64 {
            return Err(DigestError::Length {
                text: text.to_owned(),
                hex_len: hex.len(),
            });
        }
        let mut out = [0u8; 32];
        for (index, pair) in hex.as_bytes().chunks(2).enumerate() {
            let nibble = |digit: u8| match digit {
                b'0'..=b'9' => Some(digit - b'0'),
                b'a'..=b'f' => Some(digit - b'a' + 10),
                _ => None,
            };
            let (Some(&high), Some(&low)) = (pair.first(), pair.get(1)) else {
                return Err(DigestError::Hex {
                    text: text.to_owned(),
                });
            };
            let (Some(high), Some(low)) = (nibble(high), nibble(low)) else {
                return Err(DigestError::Hex {
                    text: text.to_owned(),
                });
            };
            if let Some(slot) = out.get_mut(index) {
                *slot = (high << 4) | low;
            }
        }
        Ok(Self(out))
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn hex(&self) -> String {
        let mut hex = String::with_capacity(64);
        for byte in self.0 {
            hex.push_str(&format!("{byte:02x}"));
        }
        hex
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{DIGEST_PREFIX}{}", self.hex())
    }
}

impl fmt::Debug for Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Digest({self})")
    }
}

impl TryFrom<String> for Digest {
    type Error = DigestError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::parse(&text)
    }
}

impl From<Digest> for String {
    fn from(digest: Digest) -> Self {
        digest.to_string()
    }
}

/// An immutable blob under `<plan dir>/artifacts/<hex>`, addressed by digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub digest: Digest,
    pub media_type: String,
    pub length: u64,
}

impl fmt::Display for ArtifactRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "artifact:{}", self.digest)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalError {
    /// Rule 6: no float appears in an op, so its spelling never has to be decided.
    Float {
        path: String,
    },
    Serialize {
        detail: String,
    },
}

impl fmt::Display for CanonicalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Float { path } => write!(formatter, "{path}: a float has no canonical form"),
            Self::Serialize { detail } => write!(formatter, "canonical json: {detail}"),
        }
    }
}

impl std::error::Error for CanonicalError {}

fn sorted_entries(map: &Map<String, Value>) -> Vec<(&String, &Value)> {
    let mut entries: Vec<(&String, &Value)> = map.iter().collect();
    entries.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
    entries
}

fn write_canonical(value: &Value, path: &str, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(flag) => out.extend_from_slice(if *flag { b"true" } else { b"false" }),
        Value::Number(number) => {
            if number.is_f64() {
                return Err(CanonicalError::Float {
                    path: path.to_owned(),
                });
            }
            out.extend_from_slice(number.to_string().as_bytes());
        }
        Value::String(text) => {
            serde_json::to_writer(&mut *out, text).map_err(|error| CanonicalError::Serialize {
                detail: error.to_string(),
            })?;
        }
        Value::Array(items) => {
            out.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                write_canonical(item, &format!("{path}[{index}]"), out)?;
            }
            out.push(b']');
        }
        Value::Object(map) => {
            out.push(b'{');
            for (index, (key, item)) in sorted_entries(map).into_iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                serde_json::to_writer(&mut *out, key).map_err(|error| {
                    CanonicalError::Serialize {
                        detail: error.to_string(),
                    }
                })?;
                out.push(b':');
                write_canonical(item, &format!("{path}.{key}"), out)?;
            }
            out.push(b'}');
        }
    }
    Ok(())
}

/// The canonical UTF-8 bytes of a value, no trailing newline.
pub fn canonical_bytes(value: &Value) -> Result<Vec<u8>, CanonicalError> {
    let mut out = Vec::new();
    write_canonical(value, "$", &mut out)?;
    Ok(out)
}

/// `sha256` over [`canonical_bytes`].
pub fn canonical_digest(value: &Value) -> Result<Digest, CanonicalError> {
    Ok(Digest::of(&canonical_bytes(value)?))
}

/// The same value with every object's keys sorted, for a pretty print that agrees with the
/// canonical order (the checkpoint render).
pub fn sorted(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            sorted_entries(map)
                .into_iter()
                .map(|(key, item)| (key.clone(), sorted(item)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_round_trips_with_and_without_the_prefix() -> Result<(), Box<dyn std::error::Error>> {
        let digest = Digest::of(b"");
        let text = digest.to_string();
        assert_eq!(
            text,
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(Digest::parse(&text)?, digest);
        assert_eq!(Digest::parse(&digest.hex())?, digest);
        assert!(matches!(
            Digest::parse("sha256:abc"),
            Err(DigestError::Length { hex_len: 3, .. })
        ));
        assert!(matches!(
            Digest::parse(&"G".repeat(64)),
            Err(DigestError::Hex { .. })
        ));
        Ok(())
    }

    #[test]
    fn a_float_is_refused_with_its_path() {
        let value = serde_json::json!({"a": {"b": [1, 2.5]}});
        assert_eq!(
            canonical_bytes(&value),
            Err(CanonicalError::Float {
                path: "$.a.b[1]".to_owned()
            })
        );
    }
}
