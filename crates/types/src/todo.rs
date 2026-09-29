use std::collections::BTreeSet;

use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::plan::ask::Ask;
use crate::plan::doc::{
    AgentId, BlockedOn, DocError, NOTE_MAX_BYTES, Note, TODO_LABEL_MAX, Todo, TodoLabel, TodoState,
    TodoStateName,
};
use crate::url::{Scheme, Url};

pub const TODO_ENTRY_TYPE: &str = "todo";
/// A format-2 list holds plan [`Todo`]s; a list with no format is the name-only item shape
/// sessions wrote before the two todo types merged.
pub const TODO_LIST_FORMAT: u32 = 2;
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

impl Todo {
    /// Text over the label max is cut to a label and kept whole as the note; trailing
    /// `user://<n>` tokens are the intent.
    pub fn from_text(text: &str) -> Result<Self, DocError> {
        let (text, intent) = split_cited(text.trim());
        let cut: String = text.chars().take(TODO_LABEL_MAX).collect();
        let mut todo = Self::pending(TodoLabel::new(cut.trim_end())?);
        if cut.len() < text.len() {
            todo.note = Some(Note::new(text)?);
        }
        todo.cites.intent = intent;
        Ok(todo)
    }

    pub fn is_cut(&self) -> bool {
        let label = self.label.as_str();
        self.note.as_ref().is_some_and(|note| {
            note.as_str().len() > label.len() && note.as_str().starts_with(label)
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoPhase {
    pub name: PhaseName,
    pub items: Vec<Todo>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The session's task list: the whole shape rides every `custom{todo}` entry so
/// the latest entry is the state and no replay is needed.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(try_from = "ListRepr")]
pub struct TodoList {
    pub phases: Vec<TodoPhase>,
    pub next_id: u64,
    pub extra: Map<String, Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ListOut<'a> {
    format: u32,
    phases: &'a [TodoPhase],
    next_id: u64,
    #[serde(flatten)]
    extra: &'a Map<String, Value>,
}

impl Serialize for TodoList {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        ListOut {
            format: TODO_LIST_FORMAT,
            phases: &self.phases,
            next_id: self.next_id,
            extra: &self.extra,
        }
        .serialize(serializer)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListRepr {
    #[serde(default)]
    phases: Vec<PhaseRepr>,
    #[serde(default)]
    next_id: u64,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Deserialize)]
struct PhaseRepr {
    name: PhaseName,
    #[serde(default)]
    items: Vec<Value>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

impl TryFrom<ListRepr> for TodoList {
    type Error = serde_json::Error;

    fn try_from(repr: ListRepr) -> Result<Self, Self::Error> {
        let phases = repr
            .phases
            .into_iter()
            .map(|phase| {
                Ok(TodoPhase {
                    name: phase.name,
                    items: phase
                        .items
                        .into_iter()
                        .map(read_item)
                        .collect::<Result<_, _>>()?,
                    extra: phase.extra,
                })
            })
            .collect::<Result<_, Self::Error>>()?;
        let mut extra = repr.extra;
        extra.remove("format");
        Ok(Self {
            phases,
            next_id: repr.next_id,
            extra,
        })
    }
}

/// A format-2 item never carries `on` at its top level; one that does, or that fails the
/// format-2 read, was written in the name-only shape by this binary or an older one.
fn read_item(value: Value) -> Result<Todo, serde_json::Error> {
    if value.get("on").is_some() {
        return named(value);
    }
    serde_json::from_value::<Todo>(value.clone()).or_else(|error| named(value).map_err(|_| error))
}

/// Incident: a pre-merge binary rewrote a format-2 list and left the payload of the state it
/// replaced, and the strict read then lost the whole list; its `state` word is the newest fact.
fn named(value: Value) -> Result<Todo, serde_json::Error> {
    let mut item: Map<String, Value> = serde_json::from_value(value)?;
    for stale in ["blocked", "cause", "last", "output", "resolution"] {
        item.remove(stale);
    }
    let children: Vec<Value> = match item.remove("children") {
        Some(children) => serde_json::from_value(children)?,
        None => Vec::new(),
    };
    let named: FormatOneItem = serde_json::from_value(Value::Object(item))?;
    // Kept format-2 keys (attempt, after, contract) sit in `extra`; the second read seats them.
    let mut todo: Todo = serde_json::from_value(serde_json::to_value(Todo::from(named))?)?;
    todo.children = children
        .into_iter()
        .map(read_item)
        .collect::<Result<_, _>>()?;
    Ok(todo)
}

/// A format-1 item: the state by name, its payload in `on` and `note`.
#[derive(Deserialize)]
struct FormatOneItem {
    #[serde(default)]
    id: Option<TodoId>,
    label: TodoLabel,
    state: TodoStateName,
    #[serde(default)]
    on: Option<String>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    evidence: Option<String>,
    #[serde(default)]
    intent: Vec<Url>,
    #[serde(default)]
    ask: Option<Ask>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

/// Invariant: format 1 had no note cap, so one past [`NOTE_MAX_BYTES`] keeps its head rather
/// than failing the whole list it rides in.
fn capped(note: String) -> Option<Note> {
    let mut end = note.len().min(NOTE_MAX_BYTES);
    while !note.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    Note::new(note.get(..end)?).ok()
}

impl From<FormatOneItem> for Todo {
    fn from(item: FormatOneItem) -> Self {
        let FormatOneItem {
            id,
            label,
            state,
            on,
            mut note,
            evidence,
            intent,
            ask,
            mut extra,
        } = item;
        // A mirrored row kept its plan's runner in `by`; the session's own rows named none.
        let by = extra
            .remove("by")
            .and_then(|by| AgentId::new(by.as_str()?).ok())
            .unwrap_or_else(AgentId::owner);
        let state = match state {
            TodoStateName::Pending => TodoState::Pending,
            TodoStateName::Running => TodoState::Running { by },
            TodoStateName::Blocked => TodoState::Blocked {
                on: BlockedOn::from_word(on.as_deref()),
                note: note.take().unwrap_or_default(),
            },
            TodoStateName::Done => TodoState::Done {
                output: None,
                resolution: None,
            },
            TodoStateName::Failed => TodoState::Failed {
                cause: note.take().unwrap_or_default(),
                last: None,
            },
            TodoStateName::Abandoned => TodoState::Abandoned,
            TodoStateName::Other(tag) => TodoState::Other(tag),
        };
        let mut todo = Self::pending(label);
        todo.id = id;
        todo.state = state;
        todo.note = note.and_then(capped);
        todo.evidence = evidence;
        todo.cites.intent = intent;
        todo.ask = ask;
        todo.extra = extra;
        todo
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TodoProgress {
    pub done: usize,
    pub total: usize,
    pub open: usize,
    pub blocked: usize,
}

impl TodoList {
    pub fn items(&self) -> impl Iterator<Item = &Todo> {
        self.phases.iter().flat_map(|phase| {
            phase
                .items
                .iter()
                .flat_map(|item| std::iter::once(item).chain(item.children.iter()))
        })
    }

    /// Labels held by more than one row, children included.
    pub fn duplicates(&self) -> BTreeSet<&TodoLabel> {
        let mut seen = BTreeSet::new();
        self.items()
            .map(|item| &item.label)
            .filter(|label| !seen.insert(*label))
            .collect()
    }

    pub fn for_each_mut(&mut self, mut act: impl FnMut(&mut Todo)) {
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
                TodoState::Done { .. } => progress.done = progress.done.saturating_add(1),
                TodoState::Pending | TodoState::Running { .. } => {
                    progress.open = progress.open.saturating_add(1);
                }
                TodoState::Blocked { .. } => progress.blocked = progress.blocked.saturating_add(1),
                TodoState::Failed { .. } | TodoState::Abandoned | TodoState::Other(_) => {}
            }
        }
        progress
    }

    pub fn running(&self) -> Option<&Todo> {
        self.items()
            .find(|item| matches!(item.state, TodoState::Running { .. }))
    }

    /// Sorted open labels with their states: equal fingerprints mean the model
    /// moved nothing since the last look.
    pub fn fingerprint(&self) -> String {
        let mut rows: Vec<String> = self
            .items()
            .filter(|item| !item.state.is_terminal())
            .map(|item| format!("{}={}", item.label, TodoStateName::of(&item.state)))
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

impl crate::entry::CustomRecord for TodoInterceptRecord {
    const TYPE: &'static str = TODO_INTERCEPT_ENTRY_TYPE;
}

impl crate::entry::CustomRecord for TodoRecord {
    const TYPE: &'static str = TODO_ENTRY_TYPE;
}
