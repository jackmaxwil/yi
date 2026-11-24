//! The questions the source asks a language server, and the place in a file each is about.

use std::time::Duration;

use yd_core::command::table::{Run, Target};

use super::{START, TIMEOUT};

/// A place in a file the app asks about: its path under the root, line and column from 1.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Spot {
    pub(super) path: String,
    pub(super) line: usize,
    pub(super) col: usize,
}

impl Spot {
    /// Read from a `file` or `lines` address: `#L13`, `#L13:C5` or `#L13-L20`.
    pub(super) fn of(run: &Run) -> Option<Spot> {
        let Target::Thing(address) = &run.target else {
            return None;
        };
        let part = address.part_text().unwrap_or("L1");
        let first = part.split('-').next().unwrap_or(part);
        let (line, col) = first.split_once(":C").unwrap_or((first, "1"));
        let line = line.strip_prefix('L').and_then(|n| n.parse().ok());
        Some(Spot {
            path: address.id().to_owned(),
            line: line.unwrap_or(1usize).max(1),
            col: col.parse().unwrap_or(1usize).max(1),
        })
    }

    pub(super) fn id(&self) -> String {
        format!("{}:{}:{}", self.path, self.line, self.col)
    }
}

#[derive(Debug, Clone)]
pub(super) enum Ask {
    Initialize,
    Hover(Spot),
    Definition(Spot),
    References(Spot),
    Hints(String),
    Fixes(Spot),
}

impl Ask {
    pub(super) fn method(&self) -> &'static str {
        match self {
            Ask::Initialize => "initialize",
            Ask::Hover(_) => "textDocument/hover",
            Ask::Definition(_) => "textDocument/definition",
            Ask::References(_) => "textDocument/references",
            Ask::Hints(_) => "textDocument/inlayHint",
            Ask::Fixes(_) => "textDocument/codeAction",
        }
    }

    pub(super) fn path(&self) -> Option<&str> {
        match self {
            Ask::Initialize => None,
            Ask::Hints(path) => Some(path.as_str()),
            Ask::Hover(s) | Ask::Definition(s) | Ask::References(s) | Ask::Fixes(s) => {
                Some(s.path.as_str())
            }
        }
    }

    /// A newer question of the same kind about the same file makes the older one moot.
    pub(super) fn supersedes(&self, older: &Ask) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(older)
            && self.path().is_some()
            && self.path() == older.path()
    }

    pub(super) fn limit(&self) -> Duration {
        match self {
            Ask::Initialize => START,
            _ => TIMEOUT,
        }
    }
}
