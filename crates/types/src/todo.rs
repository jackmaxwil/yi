use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::plan::doc::{DocError, TODO_LABEL_MAX, TodoLabel, TodoStateName};
use crate::url::{Scheme, Url};

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

/// A checklist row's trailing `user://<n>` tokens are its intent, cut from the label.
pub fn split_cited(text: &str) -> (&str, Vec<Url>) {
    let mut rest = text.trim_end();
    let mut cited = Vec::new();
    while let Some((head, token)) = rest.rsplit_once(' ')
        && let Ok(url) = token.parse::<Url>()
        && url.scheme() == &Scheme::User
        && url.path().bytes().all(|byte| byte.is_ascii_digit())
    {
        cited.push(url);
        rest = head.trim_end();
    }
    cited.reverse();
    (rest, cited)
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

/// A per-session item id, `t<n>`; minted by the store, never reused, additive so old
/// sessions rehydrate without one.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TodoId(String);

impl TodoId {
    pub fn minted(number: u64) -> Self {
        Self(format!("t{number}"))
    }

    pub fn parse(token: &str) -> Option<Self> {
        let digits = token.strip_prefix('t')?;
        (!digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| Self(token.to_owned()))
    }

    pub fn number(&self) -> Option<u64> {
        self.0.get(1..)?.parse().ok()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TodoId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A done todo's evidence held against the session ledger: `observed` is the recorded call
/// whose arguments or output hold a span the evidence quotes; `None` is a claim nothing checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Claim {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed: Option<String>,
}

/// One todo: `note` is the blocker, the drop reason or the fail cause, `evidence`
/// the check quoted at `done`; a child carries the same fields one level down.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoItem {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<TodoId>,
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
    /// The owner messages this item serves, by address only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub intent: Vec<Url>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl TodoItem {
    pub fn pending(label: TodoLabel) -> Self {
        Self {
            id: None,
            label,
            state: TodoStateName::Pending,
            on: None,
            note: None,
            evidence: None,
            children: Vec::new(),
            intent: Vec::new(),
            extra: Map::new(),
        }
    }

    /// Text over the label max is cut to a label and kept whole as the note; trailing
    /// `user://<n>` tokens are the intent.
    pub fn from_text(text: &str) -> Result<Self, DocError> {
        let (text, intent) = split_cited(text.trim());
        let cut: String = text.chars().take(TODO_LABEL_MAX).collect();
        let mut item = Self::pending(TodoLabel::new(cut.trim_end())?);
        if cut.len() < text.len() {
            item.note = Some(text.to_owned());
        }
        item.intent = intent;
        Ok(item)
    }

    pub fn is_cut(&self) -> bool {
        let label = self.label.as_str();
        self.note
            .as_deref()
            .is_some_and(|note| note.len() > label.len() && note.starts_with(label))
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
    #[serde(default)]
    pub next_id: u64,
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

    pub fn for_each_mut(&mut self, mut act: impl FnMut(&mut TodoItem)) {
        for phase in &mut self.phases {
            for item in &mut phase.items {
                act(item);
                for child in &mut item.children {
                    act(child);
                }
            }
        }
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
