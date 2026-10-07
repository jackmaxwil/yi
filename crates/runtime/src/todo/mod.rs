use std::sync::{Arc, Mutex};

use yi_types::plan::doc::{
    AgentId, BlockedOn, DocError, Note, PlanId, Todo, TodoLabel, TodoState, TodoStateName,
};
use yi_types::todo::{PhaseName, TodoId, TodoList, TodoPhase, TodoProgress, TodoRecord};

use crate::goal::StoreHandle;

pub mod claims;
pub mod coupling;
pub mod mirror;
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
        phases: Vec<(PhaseName, Vec<Todo>)>,
    },
    Append {
        phase: Option<PhaseName>,
        under: Option<TodoLabel>,
        items: Vec<Todo>,
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
        ask: Option<Box<yi_types::plan::ask::Ask>>,
    },
    Unblock {
        label: TodoLabel,
        answer: Option<Box<yi_types::plan::ask::Answer>>,
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
            Self::Start { label } | Self::Block { label, .. } | Self::Unblock { label, .. } => {
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
    #[error("done names no item and several are running: {running}; name one by id")]
    ManyRunning { running: String },
    #[error("no phase named {phase:?}; the list holds: {known}")]
    NoSuchPhase { phase: String, known: String },
    #[error("todo {label:?} is already in the list; labels are unique")]
    Duplicate { label: String },
    #[error(
        "todo {label:?} is a row of plan {plan}, and labels are unique; leave the plan's rows out of a set or init, and give a new todo a label of its own"
    )]
    DuplicateOfPlanRow { label: String, plan: String },
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
    #[error(
        "the list is plan {plan}; the plan tool changes it (append, block, drop, start and done on your own todos; the engine steps delegated todos)"
    )]
    Mirrored { plan: String },
    #[error(transparent)]
    Doc(#[from] DocError),
    #[error(transparent)]
    Unanswered(#[from] yi_types::plan::doc::PlanIssue),
}

impl TodoError {
    /// What the call met, for `details.errorKind`, by the plan tool's classes.
    pub fn kind(&self) -> yi_types::event::ToolErrorKind {
        use yi_types::event::ToolErrorKind;
        match self {
            Self::NoSuchLabel { .. }
            | Self::NoSuchPhase { .. }
            | Self::Illegal { .. }
            | Self::Stale { .. } => ToolErrorKind::Stale,
            Self::Duplicate { .. }
            | Self::DuplicateOfPlanRow { .. }
            | Self::ParentOpen { .. }
            | Self::SetClosed { .. }
            | Self::NoEvidence { .. }
            | Self::Unanswered(_) => ToolErrorKind::Verdict,
            Self::Ambiguous { .. }
            | Self::ManyRunning { .. }
            | Self::Empty
            | Self::Doc(_)
            | Self::Checklist { .. }
            | Self::TooDeep { .. }
            | Self::Mirrored { .. } => ToolErrorKind::InvalidArgs,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct State {
    list: TodoList,
    touched: u64,
}

pub type ResyncFn = dyn Fn(&TodoList) -> Option<TodoList> + Send + Sync;
/// `Ok(None)` is a [`Carried::Wait`] whose row no longer waits on its address.
pub type CarryFn = dyn Fn(&TodoLabel, Carried) -> Result<Option<String>, String> + Send + Sync;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Carried {
    Start,
    Done {
        pending: bool,
    },
    /// A channel wait's match, applied only while the row in `plan` still waits on `address`.
    Wait {
        plan: PlanId,
        address: String,
    },
}

pub struct TodoStore {
    state: Mutex<State>,
    store: StoreHandle,
    actor: String,
    on_change: Mutex<Vec<ChangeHook>>,
    resync: Mutex<Option<Arc<ResyncFn>>>,
    carry: Mutex<Option<Arc<CarryFn>>>,
}

pub struct Applied {
    pub before: TodoList,
    pub list: TodoList,
    pub touched: u64,
    pub changed: bool,
    /// What the call could not do as sent and what was done instead, one line each.
    pub notes: Vec<String>,
}

impl TodoStore {
    pub fn new(store: StoreHandle, actor: impl Into<String>) -> Arc<Self> {
        let this = Arc::new(Self {
            state: Mutex::new(State::default()),
            store,
            actor: actor.into(),
            on_change: Mutex::new(Vec::new()),
            resync: Mutex::new(None),
            carry: Mutex::new(None),
        });
        this.rehydrate();
        this
    }

    pub fn on_change(&self, hook: ChangeHook) {
        if let Ok(mut hooks) = self.on_change.lock() {
            hooks.push(hook);
        }
    }

    pub fn set_resync(&self, resync: Arc<ResyncFn>) {
        if let Ok(mut slot) = self.resync.lock() {
            *slot = Some(resync);
        }
    }

    pub fn set_carry(&self, carry: Arc<CarryFn>) {
        if let Ok(mut slot) = self.carry.lock() {
            *slot = Some(carry);
        }
    }

    /// A done that names no item means the one running item; several running is ambiguous.
    pub fn aim(&self, op: Op) -> Result<Op, TodoError> {
        let op = self.ask(op)?;
        let Op::Done {
            target: Target::All,
            evidence,
        } = op
        else {
            return Ok(op);
        };
        self.resync();
        let list = self.list();
        let running: Vec<&Todo> = list
            .items()
            .filter(|item| matches!(item.state, TodoState::Running { .. }))
            .collect();
        let target = match running.as_slice() {
            [] => Target::All,
            [one] => Target::Label(id_needle(one)),
            many => {
                return Err(TodoError::ManyRunning {
                    running: many
                        .iter()
                        .map(|item| named(item))
                        .collect::<Vec<_>>()
                        .join(", "),
                });
            }
        };
        Ok(Op::Done { target, evidence })
    }

    fn ask(&self, mut op: Op) -> Result<Op, TodoError> {
        let said = || {
            let store = (self.store)();
            store
                .map(|store| crate::plan::ask::said(&store))
                .unwrap_or_default()
        };
        match &mut op {
            Op::Block { ask: Some(ask), .. } => crate::plan::ask::stamp(ask, &said()),
            Op::Unblock { label, answer } => {
                let list = self.list();
                let at = locate(&list, label.as_str()).ok();
                let item = at.and_then(|at| list.items().nth(at));
                if let Some(item) =
                    item.filter(|item| matches!(item.state, TodoState::Blocked { .. }))
                    && let Some(ask) = item.ask.as_ref().filter(|ask| ask.answer.is_none())
                {
                    *answer = crate::plan::ask::reply_to(ask, &item.label, &said())?.map(Box::new);
                }
            }
            _ => {}
        }
        Ok(op)
    }

    pub fn carry(&self, op: &Op) -> Option<Result<String, String>> {
        let done = match op {
            Op::Start { .. } => false,
            Op::Done {
                target: Target::Label(_),
                ..
            } => true,
            _ => return None,
        };
        self.resync();
        let list = self.list();
        mirror::plan_of(&list)?;
        let index = locate(&list, op.label()?.as_str()).ok()?;
        let item = list.items().nth(index)?;
        item.extra.get(mirror::PLAN_KEY)?;
        let carry = self.carry.lock().ok()?.clone()?;
        let pending = matches!(item.state, TodoState::Pending);
        Some(
            carry(
                &item.label,
                if done {
                    Carried::Done { pending }
                } else {
                    Carried::Start
                },
            )
            .map(Option::unwrap_or_default),
        )
    }

    /// The clock's unblock of the session's own row; a plan's is [`TodoStore::unblock_in`]'s.
    pub fn unblock_as(&self, label: &TodoLabel, actor: &str) -> Result<(), String> {
        let op = Op::Unblock {
            label: label.clone(),
            answer: None,
        };
        self.apply_as(op, None, actor)
            .map(drop)
            .map_err(|error| error.to_string())
    }

    /// A wait in `plan`, shown in this list or not (a sub-plan's never is), unblocked by the
    /// engine as the host; `false` when the row no longer waits on `address`.
    pub fn unblock_in(
        &self,
        plan: &PlanId,
        label: &TodoLabel,
        address: &str,
    ) -> Result<bool, String> {
        let carry = self
            .carry
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .ok_or("the plan engine that owns this todo is gone")?;
        let wait = Carried::Wait {
            plan: plan.clone(),
            address: address.to_owned(),
        };
        carry(label, wait).map(|done| done.is_some())
    }

    /// Re-reads a mirrored list's plan: another engine, or a crash before the replace, moved it.
    pub fn resync(&self) {
        if let Some(resync) = self.resync.lock().ok().and_then(|slot| slot.clone()) {
            self.replace_with(|list| resync(list), mirror::ENGINE_ACTOR);
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

    /// Invariant: the list is the attached file's latest record or nothing; a `/new` swaps
    /// the file under a live store, and the old session's list must not survive it.
    pub fn rehydrate(&self) {
        let record = (self.store)().and_then(|session| latest_record(&session));
        if let Ok(mut state) = self.state.lock() {
            *state = record.map_or_else(State::default, |record| {
                let mut list = record.list;
                mint(&mut list);
                State {
                    list,
                    touched: record.touched,
                }
            });
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
        self.resync();
        let mut state = self.state.lock().map_err(|_| TodoError::Empty)?;
        // Invariant: a stale view is merged, never refused, and never deletes a row the model did
        // not see: a set from it keeps every row it left out.
        let stale = expected_touched.filter(|sent| *sent != state.touched);
        let mut notes: Vec<String> = (stale.iter())
            .map(|sent| format!("the list changed since you last saw it (touched {}, you sent {sent}); the call was applied to the list as it is now", state.touched))
            .collect();
        if matches!(op, Op::View) {
            return Ok(Applied {
                before: state.list.clone(),
                list: state.list.clone(),
                touched: state.touched,
                changed: false,
                notes,
            });
        }
        let before = state.list.clone();
        let mirrored = mirror::plan_of(&before).map(str::to_owned);
        let mut list = match &mirrored {
            Some(plan) => {
                let own = mirror::own(&before);
                let whole = matches!(op, Op::Set { .. } | Op::Init { .. } | Op::Append { .. });
                if !whole
                    && op
                        .label()
                        .is_none_or(|label| locate(&own, label.as_str()).is_err())
                {
                    if let Some(label) = op.label() {
                        locate(&before, label.as_str())?;
                    }
                    return Err(TodoError::Mirrored { plan: plan.clone() });
                }
                own
            }
            None => before.clone(),
        };
        let label = op.label().cloned();
        let name = op.name();
        let (prior, whole_set) = (list.clone(), matches!(op, Op::Set { .. }));
        step(&mut list, op, &mut notes)?;
        if stale.is_some() && whole_set {
            let kept = keep_omitted(&prior, &mut list);
            if !kept.is_empty() {
                notes.push(format!(
                    "rows your set left out were kept: {}",
                    kept.join(", ")
                ));
            }
        }
        if mirrored.is_some() {
            let planned: Vec<TodoId> = before
                .items()
                .filter(|item| item.extra.contains_key(mirror::PLAN_KEY))
                .filter_map(|item| item.id.clone())
                .collect();
            list.for_each_mut(|item| {
                if item.id.as_ref().is_some_and(|id| planned.contains(id)) {
                    item.id = None;
                }
            });
            list.next_id = list.next_id.max(before.next_id);
        }
        mint(&mut list);
        normalize(&mut list, mirrored.is_none());
        if mirrored.is_some() {
            list = mirror::rejoin(&before, list);
        }
        // Invariant: an op adds no row whose label another row holds. A repeat an older binary
        // wrote is not this op's, so that list still takes the moves that repair it.
        let rows = |list: &TodoList, label: &TodoLabel| {
            list.items().filter(|item| item.label == *label).count()
        };
        if let Some(label) = list
            .duplicates()
            .into_iter()
            .find(|label| rows(&list, label) > rows(&before, label))
        {
            // A finished plan's leftover rows keep their `plan` key; only an open plan owns them.
            let plan = mirrored.as_ref().and_then(|_| {
                before
                    .items()
                    .filter(|item| item.label == *label)
                    .find_map(mirror::row_plan)
            });
            let label = label.to_string();
            return Err(match plan {
                Some(plan) => TodoError::DuplicateOfPlanRow {
                    label,
                    plan: plan.to_string(),
                },
                None => TodoError::Duplicate { label },
            });
        }
        state.list = list.clone();
        state.touched = state.touched.saturating_add(1);
        let touched = state.touched;
        drop(state);
        self.record(name, actor, label, touched, &list);
        self.changed(&list);
        Ok(Applied {
            before,
            list,
            touched,
            changed: true,
            notes,
        })
    }

    /// Invariant: projected from the list under the lock, so no owner-row write is undone.
    pub fn replace_with(&self, project: impl FnOnce(&TodoList) -> Option<TodoList>, actor: &str) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let Some(list) = project(&state.list).filter(|list| *list != state.list) else {
            return;
        };
        state.list = list.clone();
        state.touched = state.touched.saturating_add(1);
        let touched = state.touched;
        drop(state);
        self.record("plan", actor, None, touched, &list);
        self.changed(&list);
    }

    fn changed(&self, list: &TodoList) {
        if let Ok(hooks) = self.on_change.lock() {
            for hook in hooks.iter() {
                hook(list);
            }
        }
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
        let _a_ledger_write_never_fails_an_op =
            yi_session::lock_session(&session).append_custom_record(&record);
    }
}

pub fn latest_record(session: &yi_session::SharedSession) -> Option<TodoRecord> {
    yi_session::lock_session(session)
        .custom_records(yi_session::EntryOrder::NewestFirst, Some(1))
        .pop()
}

fn named(item: &Todo) -> String {
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

fn nth_mut(list: &mut TodoList, index: usize) -> Option<&mut Todo> {
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
    let items: Vec<&Todo> = list.items().collect();
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
            let (label, rest) = (normalized(item.label.as_str()), normalized(rest));
            let same = !rest.is_empty() && (label.starts_with(&rest) || rest.starts_with(&label));
            same && item.id.as_ref().is_some_and(|own| own.as_str() == id)
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

fn resolve<'a>(list: &'a mut TodoList, needle: &TodoLabel) -> Result<&'a mut Todo, TodoError> {
    let index = locate(list, needle.as_str())?;
    let known = known_labels(list);
    nth_mut(list, index).ok_or(TodoError::NoSuchLabel {
        label: needle.to_string(),
        known,
    })
}

/// The needle [`locate`] resolves to this row alone: its id, which [`mint`] keeps unique, where
/// its label may be another row's too on a list an older binary wrote.
pub fn id_needle(item: &Todo) -> TodoLabel {
    item.id
        .as_ref()
        .and_then(|id| TodoLabel::new(id.as_str()).ok())
        .unwrap_or_else(|| item.label.clone())
}

/// Every id-less item gets `t{next_id}`, and so does a repeat of an id a `set` row copied; an
/// id a `set` row carried moves the counter past it.
pub fn mint(list: &mut TodoList) {
    let top = list
        .items()
        .filter_map(|item| item.id.as_ref()?.number())
        .max()
        .unwrap_or(0);
    let mut next = list.next_id.max(top.saturating_add(1)).max(1);
    let mut seen = std::collections::HashSet::new();
    list.for_each_mut(|item| {
        if item.id.as_ref().is_none_or(|id| !seen.insert(id.clone())) {
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

fn block(
    list: &mut TodoList,
    label: &TodoLabel,
    on: BlockedOn,
    note: String,
    ask: Option<Box<yi_types::plan::ask::Ask>>,
) -> Result<(), TodoError> {
    let item = resolve(list, label)?;
    if item.state.is_terminal() {
        return Err(illegal("block", item));
    }
    item.state = TodoState::Blocked { on, note };
    item.ask = ask.map(|ask| *ask);
    Ok(())
}

fn abandon(list: &mut TodoList, target: &Target, reason: &Note) -> Result<(), TodoError> {
    each_target(list, target, |item| {
        if !matches!(item.state, TodoState::Done { .. }) {
            item.state = TodoState::Abandoned;
            item.note = Some(reason.clone());
        }
        Ok(())
    })
}

fn illegal(op: &'static str, item: &Todo) -> TodoError {
    let from = TodoStateName::of(&item.state);
    TodoError::Illegal {
        op,
        label: item.label.to_string(),
        legal: legal_moves(&from).to_owned(),
        from,
    }
}

fn each_target<F>(list: &mut TodoList, target: &Target, mut act: F) -> Result<(), TodoError>
where
    F: FnMut(&mut Todo) -> Result<(), TodoError>,
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
    items: Vec<Todo>,
) -> Result<(), TodoError> {
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

/// The list an init replaces may already repeat a label, so the new list's own rows are checked.
fn init_list(phases: Vec<(PhaseName, Vec<Todo>)>) -> Result<TodoList, TodoError> {
    let mut fresh = TodoList::default();
    for (name, items) in phases {
        add_items(&mut fresh, Some(name), None, items)?;
    }
    if fresh.items().next().is_none() {
        return Err(TodoError::Empty);
    }
    if let Some(label) = fresh.duplicates().first() {
        return Err(TodoError::Duplicate {
            label: label.to_string(),
        });
    }
    Ok(fresh)
}

/// Rows of `prior` the new list lost, put back in their own phase; their labels, quoted.
fn keep_omitted(prior: &TodoList, list: &mut TodoList) -> Vec<String> {
    let mut kept = Vec::new();
    for phase in &prior.phases {
        let lost: Vec<Todo> = (phase.items.iter())
            .filter(|item| !list.items().any(|now| now.label == item.label))
            .cloned()
            .collect();
        if lost.is_empty() {
            continue;
        }
        kept.extend(lost.iter().map(|item| format!("{:?}", item.label.as_str())));
        match list.phases.iter_mut().find(|now| now.name == phase.name) {
            Some(now) => now.items.extend(lost),
            None => list.phases.push(TodoPhase {
                items: lost,
                ..phase.clone()
            }),
        }
    }
    kept
}

fn step(list: &mut TodoList, op: Op, notes: &mut Vec<String>) -> Result<(), TodoError> {
    let done = |item: &Todo| matches!(item.state, TodoState::Done { .. });
    match op {
        Op::View => Ok(()),
        Op::Set { list: source } => {
            let parsed = text::parse(&source)?;
            let merged = text::merge(list, parsed);
            // A set may not move an item to done; a new `[x]` row only records history.
            let closed: Vec<String> = merged
                .items()
                .filter(|item| done(item))
                .filter(|item| {
                    list.items()
                        .any(|prior| prior.label == item.label && !done(prior))
                })
                .map(|item| format!("{:?}", item.label.as_str()))
                .collect();
            let mut merged = merged;
            if !closed.is_empty() {
                merged.for_each_mut(|item| {
                    let prior = list
                        .items()
                        .find(|prior| prior.label == item.label && !done(prior));
                    if let (true, Some(prior)) = (done(item), prior) {
                        item.state = prior.state.clone();
                    }
                });
                notes.push(format!("{} kept open: done needs evidence, so `done <label>` with the command and its output line closes a row", closed.join(", ")));
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
            *list = init_list(phases)?;
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
            item.state = TodoState::Running {
                by: AgentId::owner(),
            };
            item.note = None;
            let started = (item.label.clone(), item.id.clone());
            list.for_each_mut(|other| demote(other, &started));
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
                    .filter(|child| !child.state.is_terminal())
                    .map(|child| format!("{:?}", child.label.as_str()))
                    .collect();
                if !open.is_empty() {
                    return Err(TodoError::ParentOpen {
                        label: label.to_string(),
                        open: open.join(", "),
                    });
                }
            }
            let named = matches!(target, Target::Label(_));
            each_target(list, &target, |item| {
                let closed = item.state.is_terminal();
                if matches!(item.state, TodoState::Other(_)) || (named && closed) {
                    return Err(illegal("done", item));
                }
                if closed {
                    return Ok(());
                }
                if evidence.is_none() {
                    return Err(TodoError::NoEvidence {
                        label: item.label.to_string(),
                    });
                }
                item.state = TodoState::Done {
                    output: None,
                    resolution: None,
                };
                item.note = None;
                item.evidence.clone_from(&evidence);
                Ok(())
            })
        }
        Op::Drop { target, reason } => abandon(list, &target, &Note::new(reason)?),
        Op::Block {
            label,
            on,
            note,
            ask,
        } => block(list, &label, on, note, ask),
        Op::Unblock { label, answer } => {
            let item = resolve(list, &label)?;
            if !matches!(item.state, TodoState::Blocked { .. }) {
                return Err(illegal("unblock", item));
            }
            item.state = TodoState::Pending;
            item.note = None;
            if let (Some(answer), Some(ask)) = (answer, item.ask.as_mut()) {
                if !item.cites.intent.contains(&answer.address) {
                    item.cites.intent.push(answer.address.clone());
                }
                ask.answer = Some(*answer);
            }
            Ok(())
        }
        Op::Rm { target } => match target {
            Target::Label(needle) => {
                let row = resolve(list, &needle)?.clone();
                // A list an older binary wrote may repeat a label; only the named row goes.
                let other = |item: &Todo| item.label != row.label || item.id != row.id;
                for phase in &mut list.phases {
                    phase.items.retain(other);
                    for item in &mut phase.items {
                        item.children.retain(other);
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

fn keep_one_running(row: &mut Todo, seen: &mut bool) {
    if matches!(row.state, TodoState::Running { .. }) {
        if *seen {
            row.state = TodoState::Pending;
        }
        *seen = true;
    }
}

/// A list an older binary wrote may repeat a label, so the started row is kept by label and id.
fn demote(item: &mut Todo, (label, id): &(TodoLabel, Option<TodoId>)) {
    if matches!(item.state, TodoState::Running { .. }) && (item.label != *label || item.id != *id) {
        item.state = TodoState::Pending;
    }
}

/// `promote` is off while a plan owns the list: its rows are the running work, not the owner's.
pub fn normalize(list: &mut TodoList, promote: bool) {
    let pending = |item: &Todo| matches!(item.state, TodoState::Pending);
    let run = |item: &mut Todo| {
        item.state = TodoState::Running {
            by: AgentId::owner(),
        }
    };
    let mut seen_running = false;
    for phase in &mut list.phases {
        for item in &mut phase.items {
            keep_one_running(item, &mut seen_running);
            for child in &mut item.children {
                keep_one_running(child, &mut seen_running);
            }
        }
    }
    if seen_running || !promote {
        return;
    }
    for phase in &mut list.phases {
        for item in &mut phase.items {
            if pending(item) && item.children.is_empty() {
                run(item);
                return;
            }
            for child in &mut item.children {
                if pending(child) {
                    run(child);
                    return;
                }
            }
            if pending(item) {
                run(item);
                return;
            }
        }
    }
}
