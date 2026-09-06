use std::sync::{Arc, Mutex};

use yi_types::plan::doc::{DocError, TodoLabel, TodoStateName};
use yi_types::todo::{
    BlockedOn, PhaseName, TODO_ENTRY_TYPE, TodoItem, TodoList, TodoPhase, TodoProgress, TodoRecord,
};

use crate::goal::StoreHandle;

pub mod coupling;
pub mod text;
pub mod tool;

pub const DEFAULT_PHASE: &str = "Tasks";

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
        phases: Vec<(PhaseName, Vec<TodoLabel>)>,
    },
    Append {
        phase: Option<PhaseName>,
        under: Option<TodoLabel>,
        items: Vec<TodoLabel>,
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
    #[error("no todo labelled {label:?}; the list holds: {known}")]
    NoSuchLabel { label: String, known: String },
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
                list: state.list.clone(),
                touched: state.touched,
                changed: false,
            });
        }
        let mut list = state.list.clone();
        let label = op.label().cloned();
        let name = op.name();
        step(&mut list, op)?;
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

fn known_labels(list: &TodoList) -> String {
    let labels: Vec<String> = list
        .items()
        .map(|item| format!("{:?}", item.label.as_str()))
        .collect();
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

fn require<'a>(list: &'a mut TodoList, label: &TodoLabel) -> Result<&'a mut TodoItem, TodoError> {
    let known = known_labels(list);
    find_mut(list, label).ok_or_else(|| TodoError::NoSuchLabel {
        label: label.to_string(),
        known,
    })
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
        Target::Label(label) => act(require(list, label)?),
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
            for phase in &mut list.phases {
                for item in &mut phase.items {
                    act(item)?;
                    for child in &mut item.children {
                        act(child)?;
                    }
                }
            }
            Ok(())
        }
    }
}

fn add_items(
    list: &mut TodoList,
    phase: Option<PhaseName>,
    under: Option<TodoLabel>,
    items: Vec<TodoLabel>,
) -> Result<(), TodoError> {
    for label in &items {
        if find_mut(list, label).is_some() {
            return Err(TodoError::Duplicate {
                label: label.to_string(),
            });
        }
    }
    if let Some(parent) = under {
        let parent = require(list, &parent)?;
        parent
            .children
            .extend(items.into_iter().map(TodoItem::pending));
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
    phase_mut(list, &name)?
        .items
        .extend(items.into_iter().map(TodoItem::pending));
    Ok(())
}

fn step(list: &mut TodoList, op: Op) -> Result<(), TodoError> {
    match op {
        Op::View => Ok(()),
        Op::Set { list: source } => {
            let parsed = text::parse(&source)?;
            *list = text::merge(list, parsed);
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
            let item = require(list, &label)?;
            item.state = TodoStateName::Running;
            item.on = None;
            item.note = None;
            let target = label;
            for other in list
                .phases
                .iter_mut()
                .flat_map(|phase| phase.items.iter_mut())
            {
                demote(other, &target);
                for child in &mut other.children {
                    demote(child, &target);
                }
            }
            Ok(())
        }
        Op::Done { target, evidence } => {
            if let Target::Label(label) = &target {
                let item = require(list, label)?;
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
            let item = require(list, &label)?;
            if item.is_closed() {
                return Err(illegal("block", item));
            }
            item.state = TodoStateName::Blocked;
            item.on = Some(on);
            item.note = Some(note);
            Ok(())
        }
        Op::Unblock { label } => {
            let item = require(list, &label)?;
            if item.state != TodoStateName::Blocked {
                return Err(illegal("unblock", item));
            }
            item.state = TodoStateName::Pending;
            item.on = None;
            item.note = None;
            Ok(())
        }
        Op::Rm { target } => match target {
            Target::Label(label) => {
                require(list, &label)?;
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
