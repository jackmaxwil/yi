use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::plan::doc::{DocError, TodoLabel, TodoStateName};

pub const TODO_ENTRY_TYPE: &str = "todo";
pub const TODO_INTERCEPT_ENTRY_TYPE: &str = "todo_intercept";
pub const PHASE_NAME_MAX: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PhaseName(String);

impl PhaseName {
    pub fn new(name: impl Into<String>) -> Result<Self, DocError> {
        let name = name.into();
        let name = name.trim().to_owned();
        if name.is_empty() {
            return Err(DocError::LabelEmpty);
        }
        if name.chars().count() > PHASE_NAME_MAX {
            return Err(DocError::LabelTooLong {
                label: name,
                max: PHASE_NAME_MAX,
            });
        }
        if name.contains(['\n', '\r']) {
            return Err(DocError::LabelNewline { label: name });
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PhaseName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for PhaseName {
    type Error = DocError;

    fn try_from(name: String) -> Result<Self, Self::Error> {
        Self::new(name)
    }
}

impl From<PhaseName> for String {
    fn from(name: PhaseName) -> Self {
        name.0
    }
}

/// Who a blocked todo waits on; `user` is the one value that ends a turn cleanly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BlockedOn {
    User,
    External,
    Child,
    #[serde(untagged)]
    Other(String),
}

impl BlockedOn {
    pub fn as_str(&self) -> &str {
        match self {
            Self::User => "user",
            Self::External => "external",
            Self::Child => "child",
            Self::Other(tag) => tag,
        }
    }
}

/// One todo: `note` is the blocker, the drop reason or the fail cause, `evidence`
/// the check quoted at `done`; a child carries the same fields one level down.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoItem {
    pub label: TodoLabel,
    pub state: TodoStateName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<BlockedOn>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<TodoItem>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl TodoItem {
    pub fn pending(label: TodoLabel) -> Self {
        Self {
            label,
            state: TodoStateName::Pending,
            on: None,
            note: None,
            evidence: None,
            children: Vec::new(),
            extra: Map::new(),
        }
    }

    pub fn is_open(&self) -> bool {
        matches!(self.state, TodoStateName::Pending | TodoStateName::Running)
    }

    pub fn is_closed(&self) -> bool {
        matches!(
            self.state,
            TodoStateName::Done | TodoStateName::Abandoned | TodoStateName::Failed
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoPhase {
    pub name: PhaseName,
    #[serde(default)]
    pub items: Vec<TodoItem>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The session's task list: the whole shape rides every `custom{todo}` entry so
/// the latest entry is the state and no replay is needed.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoList {
    #[serde(default)]
    pub phases: Vec<TodoPhase>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TodoProgress {
    pub done: usize,
    pub total: usize,
    pub open: usize,
    pub blocked: usize,
}

impl TodoList {
    pub fn items(&self) -> impl Iterator<Item = &TodoItem> {
        self.phases.iter().flat_map(|phase| {
            phase
                .items
                .iter()
                .flat_map(|item| std::iter::once(item).chain(item.children.iter()))
        })
    }

    pub fn progress(&self) -> TodoProgress {
        let mut progress = TodoProgress::default();
        for item in self.items() {
            progress.total = progress.total.saturating_add(1);
            match item.state {
                TodoStateName::Done => progress.done = progress.done.saturating_add(1),
                TodoStateName::Pending | TodoStateName::Running => {
                    progress.open = progress.open.saturating_add(1);
                }
                TodoStateName::Blocked => progress.blocked = progress.blocked.saturating_add(1),
                TodoStateName::Failed | TodoStateName::Abandoned | TodoStateName::Other(_) => {}
            }
        }
        progress
    }

    pub fn running(&self) -> Option<&TodoItem> {
        self.items()
            .find(|item| item.state == TodoStateName::Running)
    }

    /// Sorted open labels with their states: equal fingerprints mean the model
    /// moved nothing since the last look.
    pub fn fingerprint(&self) -> String {
        let mut rows: Vec<String> = self
            .items()
            .filter(|item| !item.is_closed())
            .map(|item| format!("{}={}", item.label, item.state))
            .collect();
        rows.sort();
        rows.join("\n")
    }
}

/// The `custom{todo}` entry payload: the op that ran, who ran it, and the list after it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoRecord {
    pub op: String,
    pub actor: String,
    pub at: u64,
    pub touched: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<TodoLabel>,
    pub list: TodoList,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The `custom{todo_intercept}` entry payload: one decision of the stop ladder, so a
/// re-driven turn is distinguishable from a silent model and the cycle survives a resume.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoInterceptRecord {
    pub at: u64,
    pub rung: u8,
    pub reason: String,
    pub fingerprint: String,
    pub cycle_total: u32,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
