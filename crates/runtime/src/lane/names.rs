//! The checked names a lane hands git: a branch, a sha, and what `HEAD` says.

use super::LaneError;

/// Invariant: checked against git's ref-format charset once; `git` never sees a flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchName(String);

impl BranchName {
    /// The trunk's name when a repository says nothing else. A literal, so it needs no parsing.
    pub(super) fn main() -> Self {
        Self("main".to_owned())
    }

    pub fn parse(text: &str) -> Result<Self, LaneError> {
        let reason = if text.is_empty() {
            Some("empty")
        } else if text.starts_with('-') {
            Some("leading dash")
        } else if text.contains("..") || text.contains("//") {
            Some("repeated separator")
        } else if text.ends_with('/') || text.ends_with('.') || text.ends_with(".lock") {
            Some("trailing separator")
        } else if !text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
        {
            Some("character outside [A-Za-z0-9._/-]")
        } else {
            None
        };
        match reason {
            Some(reason) => Err(LaneError::Branch {
                text: text.to_owned(),
                reason,
            }),
            None => Ok(Self(text.to_owned())),
        }
    }

    pub fn for_session(session: &str) -> Result<Self, LaneError> {
        Self::parse(&yi_types::lane::branch_for_session(session))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for BranchName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Invariant: forty hex digits, checked once; a sha reaches git argv only through this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sha(String);

impl Sha {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        (text.len() == 40 && text.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| Self(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn short(&self) -> &str {
        self.0.get(..8).unwrap_or(&self.0)
    }
}

/// What `.git/HEAD` names: the one reader the status row, the environment block
/// and the pool share, a file read with no git process behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    Branch(BranchName),
    Detached(Sha),
}

impl Head {
    pub fn branch(&self) -> Option<&BranchName> {
        match self {
            Self::Branch(branch) => Some(branch),
            Self::Detached(_) => None,
        }
    }

    /// The branch name, or the short sha when detached: one spelling on every surface.
    pub fn label(&self) -> String {
        match self {
            Self::Branch(branch) => branch.to_string(),
            Self::Detached(sha) => sha.short().to_owned(),
        }
    }
}
