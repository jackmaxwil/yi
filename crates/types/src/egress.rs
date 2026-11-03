//! A network name a contained process may be allowed to reach.

use serde::{Deserialize, Serialize};

/// Invariant: ASCII letters, digits and hyphens in labels, lowercase, at most 253 bytes, and a last
/// label not all digits, so `2130706433`, `0x7f.1` and `[::1]` never parse. Matching is exact.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Domain(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{text:?} is not a host name: {reason}")]
pub struct BadDomain {
    text: String,
    reason: &'static str,
}

impl Domain {
    /// # Errors
    /// [`BadDomain`] naming what the text broke.
    pub fn parse(text: &str) -> Result<Self, BadDomain> {
        let refuse = |reason| BadDomain {
            text: text
                .chars()
                .filter(char::is_ascii_graphic)
                .take(64)
                .collect(),
            reason,
        };
        let name = text.strip_suffix('.').unwrap_or(text).to_ascii_lowercase();
        if name.is_empty() || name.len() > 253 {
            return Err(refuse("empty or past 253 bytes"));
        }
        let good_label = |label: &str| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        };
        if !name.split('.').all(good_label) {
            return Err(refuse("a label is not letters, digits and hyphens"));
        }
        let last = name.rsplit('.').next().unwrap_or_default();
        if last.bytes().all(|b| b.is_ascii_digit()) || last.starts_with("0x") {
            return Err(refuse("an address, not a name"));
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Domain {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for Domain {
    type Error = BadDomain;
    fn try_from(text: String) -> Result<Self, BadDomain> {
        Self::parse(&text)
    }
}

impl From<Domain> for String {
    fn from(domain: Domain) -> Self {
        domain.0
    }
}
