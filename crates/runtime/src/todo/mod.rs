use std::sync::{Arc, Mutex};

use yi_types::plan::doc::{DocError, TodoLabel, TodoStateName};
use yi_types::todo::{
    BlockedOn, PhaseName, TODO_ENTRY_TYPE, TodoId, TodoItem, TodoList, TodoPhase, TodoProgress,
    TodoRecord,
};

use crate::goal::StoreHandle;

pub mod coupling;
pub mod text;
pub mod tool;

pub const DEFAULT_PHASE: &str = "Tasks";
pub const PREFIX_MIN: usize = 8;

pub type ChangeHook = Arc<dyn Fn(&TodoList) + Send + Sync>;

#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Label(TodoLabel),
    Phase(PhaseName),
    All,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Set {
        list: String,
    },
    Init {
        phases: Vec<(PhaseName, Vec<TodoItem>)>,
    },
    Append {
        phase: Option<PhaseName>,
        under: Option<TodoLabel>,
        items: Vec<TodoItem>,
    },
    Start {
        label: TodoLabel,
    },
    Done {
        target: Target,
        evidence: Option<String>,
    },
    Drop {
        target: Target,
        reason: String,
    },
    Block {
        label: TodoLabel,
        on: BlockedOn,
        note: String,
    },
    Unblock {
        label: TodoLabel,
    },
    Rm {
        target: Target,
    },
    View,
}

impl Op {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Set { .. } => "set",
            Self::Init { .. } => "init",
            Self::Append { .. } => "append",
            Self::Start { .. } => "start",
            Self::Done { .. } => "done",
            Self::Drop { .. } => "drop",
            Self::Block { .. } => "block",
            Self::Unblock { .. } => "unblock",
            Self::Rm { .. } => "rm",
            Self::View => "view",
        }
    }

    pub fn is_state_change(&self) -> bool {
        !matches!(self, Self::View)
    }

    fn label(&self) -> Option<&TodoLabel> {
        match self {
            Self::Start { label } | Self::Block { label, .. } | Self::Unblock { label } => {
                Some(label)
            }
            Self::Done {
                target: Target::Label(label),
                ..
            }
            | Self::Drop {
                target: Target::Label(label),
                ..
            }
            | Self::Rm {
                target: Target::Label(label),
            } => Some(label),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TodoError {
    #[error(
        "no todo {label:?} (a set or init renumbers; read the ids from its result); the list holds: {known}"
    )]
    NoSuchLabel { label: String, known: String },
    #[error("{needle:?} is a prefix of more than one todo: {candidates}; name one by id")]
    Ambiguous { needle: String, candidates: String },
    #[error("no phase named {phase:?}; the list holds: {known}")]
    NoSuchPhase { phase: String, known: String },
    #[error("todo {label:?} is already in the list; labels are unique")]
    Duplicate { label: String },
    #[error("{op} is not a move for {label:?} in state {from}; legal here: {legal}")]
    Illegal {
        op: &'static str,
        label: String,
        from: TodoStateName,
        legal: String,
    },
    #[error("{label:?} still has open children: {open}; finish or drop them first")]
    ParentOpen { label: String, open: String },
    #[error(
        "done needs evidence: the command you ran and the line of its output that proves {label:?}"
    )]
    NoEvidence { label: String },
    #[error("`set` cannot close {labels}; `done <label>` with evidence closes an item")]
    SetClosed { labels: String },
    #[error(
        "line {line} is not a checklist row (`- [ ] label`; `[>]` running, `[x]` done, `[-]` dropped, `[!]` blocked; `## Phase` heads a phase; two spaces nest one level): {text:?}"
    )]
    Checklist { line: usize, text: String },
    #[error("the list is empty; init or set it first")]
    Empty,
    #[error("nesting goes deeper than one level at line {line}")]
    TooDeep { line: usize },
    #[error(
        "the list changed since you last saw it (touched {now}, you sent {sent}); view it first"
    )]
    Stale { now: u64, sent: u64 },
    #[error(transparent)]
    Doc(#[from] DocError),
}

#[derive(Debug, Clone, Default)]
struct State {
    list: TodoList,
    touched: u64,
}

pub struct TodoStore {
    state: Mutex<State>,
    store: StoreHandle,
    actor: String,
    on_change: Mutex<Vec<ChangeHook>>,
}

pub struct Applied {
    pub before: TodoList,
    pub list: TodoList,
    pub touched: u64,
    pub changed: bool,
}

impl TodoStore {
    pub fn new(store: StoreHandle, actor: impl Into<String>) -> Arc<Self> {
        let this = Arc::new(Self {
            state: Mutex::new(State::default()),
            store,
            actor: actor.into(),
            on_change: Mutex::new(Vec::new()),
        });
        this.rehydrate();
        this
    }

    pub fn on_change(&self, hook: ChangeHook) {
        if let Ok(mut hooks) = self.on_change.lock() {
            hooks.push(hook);
        }
    }

    pub fn list(&self) -> TodoList {
        self.state
            .lock()
            .map(|state| state.list.clone())
            .unwrap_or_default()
    }

    pub fn touched(&self) -> u64 {
        self.state.lock().map(|state| state.touched).unwrap_or(0)
    }

    pub fn progress(&self) -> TodoProgress {
        self.list().progress()
    }

    fn rehydrate(&self) {
        let Some(session) = (self.store)() else {
            return;
        };
        let Some(record) = latest_record(&session) else {
            return;
        };
        if let Ok(mut state) = self.state.lock() {
            state.list = record.list;
            mint(&mut state.list);
            state.touched = record.touched;
        }
    }

    pub fn apply(&self, op: Op, expected_touched: Option<u64>) -> Result<Applied, TodoError> {
        self.apply_as(op, expected_touched, &self.actor)
    }

    pub fn apply_as(
        &self,
        op: Op,
        expected_touched: Option<u64>,
        actor: &str,
    ) -> Result<Applied, TodoError> {
        let mut state = self.state.lock().map_err(|_| TodoError::Empty)?;
        if let Some(sent) = expected_touched
            && sent != state.touched
        {
            return Err(TodoError::Stale {
                now: state.touched,
                sent,
            });
        }
        if matches!(op, Op::View) {
            return Ok(Applied {
                before: state.list.clone(),
                list: state.list.clone(),
                touched: state.touched,
                changed: false,
            });
        }
        let before = state.list.clone();
        let mut list = before.clone();
        let label = op.label().cloned();
        let name = op.name();
        step(&mut list, op)?;
        mint(&mut list);
        normalize(&mut list);
        state.list = list.clone();
        state.touched = state.touched.saturating_add(1);
        let touched = state.touched;
        drop(state);
        self.record(name, actor, label, touched, &list);
        if let Ok(hooks) = self.on_change.lock() {
            for hook in hooks.iter() {
                hook(&list);
            }
        }
        Ok(Applied {
            before,
            list,
            touched,
            changed: true,
        })
    }

    fn record(
        &self,
        op: &str,
        actor: &str,
        label: Option<TodoLabel>,
        touched: u64,
        list: &TodoList,
    ) {
        let Some(session) = (self.store)() else {
            return;
        };
        let record = TodoRecord {
            op: op.to_owned(),
            actor: actor.to_owned(),
            at: yi_session::now_ms(),
            touched,
            label,
            list: list.clone(),
            extra: serde_json::Map::new(),
        };
        let Ok(payload) = serde_json::to_value(&record) else {
            return;
        };
        let _a_ledger_write_never_fails_an_op = yi_session::lock_session(&session).append_custom(
            "main",
            TODO_ENTRY_TYPE,
            Some(payload),
        );
    }
}

pub fn latest_record(session: &yi_session::SharedSession) -> Option<TodoRecord> {
    let entries = yi_session::lock_session(session)
        .find_entries(&yi_session::EntryQuery {
            custom_type: Some(TODO_ENTRY_TYPE.to_owned()),
            order: yi_session::EntryOrder::NewestFirst,
            limit: Some(1),
            ..yi_session::EntryQuery::default()
        })
        .unwrap_or_default();
    entries.into_iter().find_map(|entry| match entry {
        yi_types::entry::Entry::Custom {
            data: Some(data), ..
        } => serde_json::from_value::<TodoRecord>(data).ok(),
        _ => None,
    })
}

fn named(item: &TodoItem) -> String {
    match &item.id {
        Some(id) => format!("{id} {:?}", item.label.as_str()),
        None => format!("{:?}", item.label.as_str()),
    }
}

fn known_labels(list: &TodoList) -> String {
    let labels: Vec<String> = list.items().map(named).collect();
    if labels.is_empty() {
        "nothing".to_owned()
    } else {
        labels.join(", ")
    }
}

fn known_phases(list: &TodoList) -> String {
    let names: Vec<String> = list
        .phases
        .iter()
        .map(|phase| format!("{:?}", phase.name.as_str()))
        .collect();
    if names.is_empty() {
        "nothing".to_owned()
    } else {
        names.join(", ")
    }
}

fn find_mut<'a>(list: &'a mut TodoList, label: &TodoLabel) -> Option<&'a mut TodoItem> {
    for phase in &mut list.phases {
        for item in &mut phase.items {
            if item.label == *label {
                return Some(item);
            }
            if let Some(child) = item.children.iter_mut().find(|child| child.label == *label) {
                return Some(child);
            }
        }
    }
    None
}

fn nth_mut(list: &mut TodoList, index: usize) -> Option<&mut TodoItem> {
    let mut at = 0_usize;
    for phase in &mut list.phases {
        for item in &mut phase.items {
            if at == index {
                return Some(item);
            }
            at = at.saturating_add(1);
            for child in &mut item.children {
                if at == index {
                    return Some(child);
                }
                at = at.saturating_add(1);
            }
        }
    }
    None
}

/// Incident: the renderer marks a cut label with an ellipsis and models retype one without
/// its backticks; both name one item, so both sides normalise before comparison (#474).
fn normalized(text: &str) -> String {
    let stripped = text
        .trim()
        .trim_end_matches('…')
        .trim_end_matches("...")
        .trim();
    stripped
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('`', "")
        .to_lowercase()
}

/// Exact label; label or id with backticks stripped; an id and its own label (`t1 read the
/// spec`); the same label [`normalized`]; a unique case-insensitive prefix of [`PREFIX_MIN`].
fn locate(list: &TodoList, needle: &str) -> Result<usize, TodoError> {
    let items: Vec<&TodoItem> = list.items().collect();
    if let Some(index) = items.iter().position(|item| item.label.as_str() == needle) {
        return Ok(index);
    }
    let bare = needle.trim().trim_matches('`').trim();
    if let Some(index) = items.iter().position(|item| {
        item.label.as_str() == bare || item.id.as_ref().is_some_and(|id| id.as_str() == bare)
    }) {
        return Ok(index);
    }
    if let Some((id, rest)) = bare.split_once(' ')
        && let Some(index) = items.iter().position(|item| {
            item.label.as_str() == rest && item.id.as_ref().is_some_and(|own| own.as_str() == id)
        })
    {
        return Ok(index);
    }
    let wanted = normalized(needle);
    if !wanted.is_empty() {
        let same: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(_, item)| normalized(item.label.as_str()) == wanted)
            .map(|(index, _)| index)
            .collect();
        if let [one] = same.as_slice() {
            return Ok(*one);
        }
    }
    let prefix = bare.to_lowercase();
    let hits: Vec<usize> = if bare.chars().count() >= PREFIX_MIN {
        items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.label.as_str().to_lowercase().starts_with(&prefix))
            .map(|(index, _)| index)
            .collect()
    } else {
        Vec::new()
    };
    match hits.as_slice() {
        [one] => Ok(*one),
        [] => Err(TodoError::NoSuchLabel {
            label: needle.to_owned(),
            known: known_labels(list),
        }),
        many => Err(TodoError::Ambiguous {
            needle: needle.to_owned(),
            candidates: many
                .iter()
                .filter_map(|index| items.get(*index))
                .map(|item| named(item))
                .collect::<Vec<_>>()
                .join(", "),
        }),
    }
}

fn resolve<'a>(list: &'a mut TodoList, needle: &TodoLabel) -> Result<&'a mut TodoItem, TodoError> {
    let index = locate(list, needle.as_str())?;
    let known = known_labels(list);
    nth_mut(list, index).ok_or(TodoError::NoSuchLabel {
        label: needle.to_string(),
        known,
    })
}

/// Every id-less item gets `t{next_id}`; an id a `set` row carried moves the counter past it.
pub fn mint(list: &mut TodoList) {
    let top = list
        .items()
        .filter_map(|item| item.id.as_ref()?.number())
        .max()
        .unwrap_or(0);
    let mut next = list.next_id.max(top.saturating_add(1)).max(1);
    list.for_each_mut(|item| {
        if item.id.is_none() {
            item.id = Some(TodoId::minted(next));
            next = next.saturating_add(1);
        }
    });
    list.next_id = next;
}

fn phase_mut<'a>(list: &'a mut TodoList, name: &PhaseName) -> Result<&'a mut TodoPhase, TodoError> {
    let known = known_phases(list);
    list.phases
        .iter_mut()
        .find(|phase| phase.name == *name)
        .ok_or_else(|| TodoError::NoSuchPhase {
            phase: name.to_string(),
            known,
        })
}

fn legal_moves(state: &TodoStateName) -> &'static str {
    match state {
        TodoStateName::Pending => "start, done, block, drop, rm",
        TodoStateName::Running => "done, block, drop, rm",
        TodoStateName::Blocked => "unblock, start, done, drop, rm",
        TodoStateName::Done | TodoStateName::Abandoned | TodoStateName::Failed => "start, rm",
        TodoStateName::Other(_) => "rm",
    }
}

fn illegal(op: &'static str, item: &TodoItem) -> TodoError {
    TodoError::Illegal {
        op,
        label: item.label.to_string(),
        from: item.state.clone(),
        legal: legal_moves(&item.state).to_owned(),
    }
}

fn close(
    item: &mut TodoItem,
    state: TodoStateName,
    note: Option<String>,
    evidence: Option<String>,
) {
    item.state = state;
    item.on = None;
    item.note = note;
    if evidence.is_some() {
        item.evidence = evidence;
    }
}

fn each_target<F>(list: &mut TodoList, target: &Target, mut act: F) -> Result<(), TodoError>
where
    F: FnMut(&mut TodoItem) -> Result<(), TodoError>,
{
    match target {
        Target::Label(label) => act(resolve(list, label)?),
        Target::Phase(name) => {
            for item in &mut phase_mut(list, name)?.items {
                act(item)?;
                for child in &mut item.children {
                    act(child)?;
                }
            }
            Ok(())
        }
        Target::All => {
            let mut first = Ok(());
            list.for_each_mut(|item| {
                if first.is_ok() {
                    first = act(item);
                }
            });
            first
        }
    }
}

fn add_items(
    list: &mut TodoList,
    phase: Option<PhaseName>,
    under: Option<TodoLabel>,
    items: Vec<TodoItem>,
) -> Result<(), TodoError> {
    for item in &items {
        if find_mut(list, &item.label).is_some() {
            return Err(TodoError::Duplicate {
                label: item.label.to_string(),
            });
        }
    }
    if let Some(parent) = under {
        resolve(list, &parent)?.children.extend(items);
        return Ok(());
    }
    let name = match phase {
        Some(name) => name,
        None => list
            .phases
            .last()
            .map(|phase| phase.name.clone())
            .unwrap_or(PhaseName::new(DEFAULT_PHASE)?),
    };
    if !list.phases.iter().any(|phase| phase.name == name) {
        list.phases.push(TodoPhase {
            name: name.clone(),
            items: Vec::new(),
            extra: serde_json::Map::new(),
        });
    }
    phase_mut(list, &name)?.items.extend(items);
    Ok(())
}

fn step(list: &mut TodoList, op: Op) -> Result<(), TodoError> {
    match op {
        Op::View => Ok(()),
        Op::Set { list: source } => {
            let parsed = text::parse(&source)?;
            let merged = text::merge(list, parsed);
            // A set may not move an item to done; a new `[x]` row only records history.
            let closed: Vec<String> = merged
                .items()
                .filter(|item| item.state == TodoStateName::Done)
                .filter(|item| {
                    list.items().any(|prior| {
                        prior.label == item.label && prior.state != TodoStateName::Done
                    })
                })
                .map(|item| format!("{:?}", item.label.as_str()))
                .collect();
            if !closed.is_empty() {
                return Err(TodoError::SetClosed {
                    labels: closed.join(", "),
                });
            }
            // A list that kept no label is a new list: its ids start at t1 like an init's.
            let survived = merged.items().any(|item| item.id.is_some());
            *list = merged;
            if !survived {
                list.next_id = 0;
            }
            Ok(())
        }
        Op::Init { phases } => {
            let mut fresh = TodoList::default();
            for (name, items) in phases {
                add_items(&mut fresh, Some(name), None, items)?;
            }
            if fresh.items().next().is_none() {
                return Err(TodoError::Empty);
            }
            *list = fresh;
            Ok(())
        }
        Op::Append {
            phase,
            under,
            items,
        } => {
            if items.is_empty() {
                return Err(TodoError::Empty);
            }
            add_items(list, phase, under, items)
        }
        Op::Start { label } => {
            let item = resolve(list, &label)?;
            item.state = TodoStateName::Running;
            item.on = None;
            item.note = None;
            let target = item.label.clone();
            list.for_each_mut(|other| demote(other, &target));
            Ok(())
        }
        Op::Done { target, evidence } => {
            let evidence = evidence
                .map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty());
            if let Target::Label(label) = &target {
                let item = resolve(list, label)?;
                let open: Vec<String> = item
                    .children
                    .iter()
                    .filter(|child| !child.is_closed())
                    .map(|child| format!("{:?}", child.label.as_str()))
                    .collect();
                if !open.is_empty() {
                    return Err(TodoError::ParentOpen {
                        label: label.to_string(),
                        open: open.join(", "),
                    });
                }
            }
            each_target(list, &target, |item| {
                if matches!(item.state, TodoStateName::Other(_)) {
                    return Err(illegal("done", item));
                }
                if evidence.is_none() {
                    return Err(TodoError::NoEvidence {
                        label: item.label.to_string(),
                    });
                }
                close(item, TodoStateName::Done, None, evidence.clone());
                Ok(())
            })
        }
        Op::Drop { target, reason } => each_target(list, &target, |item| {
            if item.state == TodoStateName::Done {
                return Ok(());
            }
            close(item, TodoStateName::Abandoned, Some(reason.clone()), None);
            Ok(())
        }),
        Op::Block { label, on, note } => {
            let item = resolve(list, &label)?;
            if item.is_closed() {
                return Err(illegal("block", item));
            }
            item.state = TodoStateName::Blocked;
            item.on = Some(on);
            item.note = Some(note);
            Ok(())
        }
        Op::Unblock { label } => {
            let item = resolve(list, &label)?;
            if item.state != TodoStateName::Blocked {
                return Err(illegal("unblock", item));
            }
            item.state = TodoStateName::Pending;
            item.on = None;
            item.note = None;
            Ok(())
        }
        Op::Rm { target } => match target {
            Target::Label(needle) => {
                let label = resolve(list, &needle)?.label.clone();
                for phase in &mut list.phases {
                    phase.items.retain(|item| item.label != label);
                    for item in &mut phase.items {
                        item.children.retain(|child| child.label != label);
                    }
                }
                Ok(())
            }
            Target::Phase(name) => {
                phase_mut(list, &name)?;
                list.phases.retain(|phase| phase.name != name);
                Ok(())
            }
            Target::All => {
                list.phases.clear();
                Ok(())
            }
        },
    }
}

fn keep_one_running(row: &mut TodoItem, seen: &mut bool) {
    if row.state == TodoStateName::Running {
        if *seen {
            row.state = TodoStateName::Pending;
        }
        *seen = true;
    }
}

fn demote(item: &mut TodoItem, keep: &TodoLabel) {
    if item.state == TodoStateName::Running && item.label != *keep {
        item.state = TodoStateName::Pending;
    }
}

pub fn normalize(list: &mut TodoList) {
    let mut seen_running = false;
    for phase in &mut list.phases {
        for item in &mut phase.items {
            keep_one_running(item, &mut seen_running);
            for child in &mut item.children {
                keep_one_running(child, &mut seen_running);
            }
        }
    }
    if seen_running {
        return;
    }
    for phase in &mut list.phases {
        for item in &mut phase.items {
            if item.state == TodoStateName::Pending && item.children.is_empty() {
                item.state = TodoStateName::Running;
                return;
            }
            for child in &mut item.children {
                if child.state == TodoStateName::Pending {
                    child.state = TodoStateName::Running;
                    return;
                }
            }
            if item.state == TodoStateName::Pending {
                item.state = TodoStateName::Running;
                return;
            }
        }
    }
}
