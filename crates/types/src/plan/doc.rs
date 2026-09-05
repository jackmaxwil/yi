use crate::plan::PlanVersion;
use crate::url::{Durability, Scheme, Url, UrlError};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

pub use super::ids::{
    AgentId, GOAL_TEXT_MAX, GoalText, INLINE_NOTE_MAX_BYTES, InlineNote, PLAN_FORMAT, PLAN_ID_MAX,
    PlanId, ProbeCommand, RetryCount, SLUG_MAX, SPAWN_CAP, Spawns, TODO_LABEL_MAX, TodoAddr,
    TodoLabel, TouchCount,
};

use super::ids::slugify;

/// The parent rides the variant: a tier flag beside an optional parent would
/// admit a sub-plan without one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanTier {
    Root,
    Sub {
        parent: TodoAddr,
    },
    Other {
        tier: String,
        parent: Option<TodoAddr>,
    },
}

/// Invariant: the plan file is ledger truth, so an unknown state tag from another version
/// degrades to [`PlanState::Other`] and re-emits verbatim rather than failing the document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanState {
    Active,
    Done,
    Superseded {
        by: PlanVersion,
    },
    Abandoned,
    #[serde(untagged)]
    Other(String),
}

impl std::fmt::Display for PlanState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active => formatter.write_str("active"),
            Self::Done => formatter.write_str("done"),
            Self::Superseded { by } => write!(formatter, "superseded by v{}", by.0),
            Self::Abandoned => formatter.write_str("abandoned"),
            Self::Other(tag) => formatter.write_str(tag),
        }
    }
}

/// The acceptance envelope: the runtime checks existence and provenance, the
/// content is for models.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Check {
    Command(String),
    Stated(String),
    #[serde(untagged)]
    Other(String),
}

/// The discriminant decides the loop's stop posture — child keeps working,
/// user ends the turn and asks, external checks on a cadence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BlockedOn {
    Child(AgentId),
    User,
    External {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        probe: Option<ProbeCommand>,
    },
    #[serde(untagged)]
    Other(String),
}

/// Ready is never a variant: it is derived from `after` plus states at read
/// time, so a stored ready bit cannot drift from the edges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TodoState {
    Pending,
    Running {
        by: AgentId,
    },
    Blocked {
        on: BlockedOn,
        note: String,
    },
    Done {
        output: Option<Url>,
    },
    /// Invariant: `last` is host-minted at reap, never model-supplied, and terminal-only;
    /// [`crate::url::Durability`] keeps an ephemeral trace out of a record that outlives it.
    Failed {
        cause: String,
        last: Option<Url>,
    },
    Abandoned,
    Other(String),
}

impl TodoState {
    /// Invariant: an unknown state written by another version is not terminal
    /// — counting it finished could close a plan over work still in flight.
    pub fn is_terminal(&self) -> bool {
        match self {
            Self::Done { .. } | Self::Failed { .. } | Self::Abandoned => true,
            Self::Pending | Self::Running { .. } | Self::Blocked { .. } | Self::Other(_) => false,
        }
    }

    /// Invariant: Abandoned clears the edge it holds and Failed does not: dropping a todo is
    /// a decision its successors must survive, while a failure still owes them the work.
    pub fn clears_edge(&self) -> bool {
        match self {
            Self::Done { .. } | Self::Abandoned => true,
            Self::Failed { .. }
            | Self::Pending
            | Self::Running { .. }
            | Self::Blocked { .. }
            | Self::Other(_) => false,
        }
    }
}

/// Invariant: a terminal record may not name a referent that dies before it — never a live
/// agent, and a kernel variable only in the plan owner's own namespace.
pub fn terminal_durability(url: &Url, owner: &AgentId) -> Durability {
    match url.scheme() {
        Scheme::Agent => Durability::Ephemeral,
        Scheme::Kernel => match url.path().split_once('/') {
            Some((agent, _)) if agent != owner.as_str() => Durability::Ephemeral,
            Some(_) | None => Durability::Durable,
        },
        Scheme::Local
        | Scheme::Plan
        | Scheme::History
        | Scheme::Checkpoint
        | Scheme::Mcp
        | Scheme::User
        | Scheme::External(_) => Durability::Durable,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Isolation {
    None,
    Worktree,
    #[serde(untagged)]
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenBudget(pub u64);

/// Describes the child to spawn; it never names a live one, so a stale spec
/// cannot point at a dead agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpawnSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<crate::model::Effort>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<Isolation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<TokenBudget>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Declaring a schema makes `done` without a validated output illegal — a
/// delegation property, checked in the step table, never todo state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputSchema {
    pub schema: Url,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The hard hand-off object: acceptance is mandatory because a child is about
/// to run blind on it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Delegation {
    pub spec: SpawnSpec,
    pub accept: Check,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<OutputSchema>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<Url>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<InlineNote>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `after` is ordering only — no data rides an edge; sibling-only and
/// acyclic, enforced by [`Plan::validate`] at every insert.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "TodoRepr", into = "TodoRepr")]
pub struct Todo {
    pub label: TodoLabel,
    pub after: Vec<TodoLabel>,
    pub state: TodoState,
    pub delegation: Option<Delegation>,
    pub subplan: Option<PlanId>,
    pub retries: RetryCount,
    /// Grouping only: a child has a label and a state, never an edge, a delegation or a
    /// sub-plan of its own, and the whole tree is replaced by one `set`.
    pub children: Vec<Todo>,
    pub extra: Map<String, Value>,
}

/// Rows done over rows in the whole tree, and the first running label, depth-first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub running: Option<TodoLabel>,
}

pub fn progress(todos: &[Todo]) -> Progress {
    let mut stack: Vec<&Todo> = todos.iter().rev().collect();
    let mut done = 0_usize;
    let mut total = 0_usize;
    let mut running: Option<TodoLabel> = None;
    while let Some(todo) = stack.pop() {
        total = total.saturating_add(1);
        match &todo.state {
            TodoState::Done { .. } => done = done.saturating_add(1),
            TodoState::Running { .. } => {
                if running.is_none() {
                    running = Some(todo.label.clone());
                }
            }
            TodoState::Pending
            | TodoState::Blocked { .. }
            | TodoState::Failed { .. }
            | TodoState::Abandoned
            | TodoState::Other(_) => {}
        }
        stack.extend(todo.children.iter().rev());
    }
    Progress {
        done,
        total,
        running,
    }
}

/// The state's name without its payload — what a duration in the ledger is
/// measured between, and the one vocabulary that keeps an unknown tag verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TodoStateName {
    Pending,
    Running,
    Blocked,
    Done,
    Failed,
    Abandoned,
    #[serde(untagged)]
    Other(String),
}

impl TodoStateName {
    pub fn of(state: &TodoState) -> Self {
        match state {
            TodoState::Pending => Self::Pending,
            TodoState::Running { .. } => Self::Running,
            TodoState::Blocked { .. } => Self::Blocked,
            TodoState::Done { .. } => Self::Done,
            TodoState::Failed { .. } => Self::Failed,
            TodoState::Abandoned => Self::Abandoned,
            TodoState::Other(tag) => Self::Other(tag.clone()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Abandoned => "abandoned",
            Self::Other(tag) => tag,
        }
    }
}

impl std::fmt::Display for TodoStateName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct BlockedRepr {
    on: BlockedOn,
    note: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct TodoRepr {
    label: TodoLabel,
    state: TodoStateName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    by: Option<AgentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    blocked: Option<BlockedRepr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cause: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last: Option<Url>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output: Option<Url>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    after: Vec<TodoLabel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delegation: Option<Delegation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subplan: Option<PlanId>,
    #[serde(default, skip_serializing_if = "RetryCount::is_zero")]
    retries: RetryCount,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    children: Vec<TodoRepr>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

impl From<Todo> for TodoRepr {
    fn from(todo: Todo) -> Self {
        let (state, by, blocked, cause, last, output) = match todo.state {
            TodoState::Pending => (TodoStateName::Pending, None, None, None, None, None),
            TodoState::Running { by } => (TodoStateName::Running, Some(by), None, None, None, None),
            TodoState::Blocked { on, note } => (
                TodoStateName::Blocked,
                None,
                Some(BlockedRepr { on, note }),
                None,
                None,
                None,
            ),
            TodoState::Done { output } => (TodoStateName::Done, None, None, None, None, output),
            TodoState::Failed { cause, last } => {
                (TodoStateName::Failed, None, None, Some(cause), last, None)
            }
            TodoState::Abandoned => (TodoStateName::Abandoned, None, None, None, None, None),
            TodoState::Other(tag) => (TodoStateName::Other(tag), None, None, None, None, None),
        };
        Self {
            label: todo.label,
            state,
            by,
            blocked,
            cause,
            last,
            output,
            after: todo.after,
            delegation: todo.delegation,
            subplan: todo.subplan,
            retries: todo.retries,
            children: todo.children.into_iter().map(Self::from).collect(),
            extra: todo.extra,
        }
    }
}

impl TryFrom<TodoRepr> for Todo {
    type Error = DocError;

    fn try_from(repr: TodoRepr) -> Result<Self, Self::Error> {
        let tag = repr.state.clone();
        let missing = |field| DocError::StateFieldMissing {
            state: tag.as_str().to_owned(),
            field,
        };
        let stray = |field| DocError::StateFieldStray {
            state: tag.as_str().to_owned(),
            field,
        };
        let mut by = repr.by;
        let mut blocked = repr.blocked;
        let mut cause = repr.cause;
        let mut last = repr.last;
        let mut output = repr.output;
        let state = match repr.state {
            TodoStateName::Pending => TodoState::Pending,
            TodoStateName::Running => TodoState::Running {
                by: by.take().ok_or_else(|| missing("by"))?,
            },
            TodoStateName::Blocked => {
                let repr = blocked.take().ok_or_else(|| missing("blocked"))?;
                TodoState::Blocked {
                    on: repr.on,
                    note: repr.note,
                }
            }
            TodoStateName::Done => TodoState::Done {
                output: output.take(),
            },
            TodoStateName::Failed => TodoState::Failed {
                cause: cause.take().ok_or_else(|| missing("cause"))?,
                last: last.take(),
            },
            TodoStateName::Abandoned => TodoState::Abandoned,
            TodoStateName::Other(other) => TodoState::Other(other),
        };
        if by.is_some() {
            return Err(stray("by"));
        }
        if blocked.is_some() {
            return Err(stray("blocked"));
        }
        if cause.is_some() {
            return Err(stray("cause"));
        }
        if last.is_some() {
            return Err(stray("last"));
        }
        if output.is_some() {
            return Err(stray("output"));
        }
        let children = repr
            .children
            .into_iter()
            .map(Self::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            label: repr.label,
            after: repr.after,
            state,
            delegation: repr.delegation,
            subplan: repr.subplan,
            retries: repr.retries,
            children,
            extra: repr.extra,
        })
    }
}

/// The frontmatter truth of one plan file; `format` pins the schema and every
/// key is a single lowercase word.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "PlanRepr", into = "PlanRepr")]
pub struct Plan {
    pub id: PlanId,
    pub goal: GoalText,
    pub version: PlanVersion,
    pub touched: TouchCount,
    pub tier: PlanTier,
    /// Invariant: the delegation fuse is monotonic, so [`Plan::charge_spawn`] and birth are
    /// its only writers; a user editing the plan file down is the sanctioned way back.
    spawns: Spawns,
    pub todos: Vec<Todo>,
    pub state: PlanState,
    pub extra: Map<String, Value>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum TierTag {
    Root,
    Sub,
    #[serde(untagged)]
    Other(String),
}

#[derive(Clone, Serialize, Deserialize)]
struct PlanRepr {
    format: u32,
    plan: PlanId,
    goal: GoalText,
    version: PlanVersion,
    #[serde(default)]
    touched: TouchCount,
    tier: TierTag,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent: Option<TodoAddr>,
    #[serde(default, skip_serializing_if = "Spawns::is_zero")]
    spawns: Spawns,
    state: PlanState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    todos: Vec<Todo>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

impl From<Plan> for PlanRepr {
    fn from(plan: Plan) -> Self {
        let (tier, parent) = match plan.tier {
            PlanTier::Root => (TierTag::Root, None),
            PlanTier::Sub { parent } => (TierTag::Sub, Some(parent)),
            PlanTier::Other { tier, parent } => (TierTag::Other(tier), parent),
        };
        Self {
            format: PLAN_FORMAT,
            plan: plan.id,
            goal: plan.goal,
            version: plan.version,
            touched: plan.touched,
            tier,
            parent,
            spawns: plan.spawns,
            state: plan.state,
            todos: plan.todos,
            extra: plan.extra,
        }
    }
}

impl TryFrom<PlanRepr> for Plan {
    type Error = DocError;

    fn try_from(repr: PlanRepr) -> Result<Self, Self::Error> {
        if repr.format != PLAN_FORMAT {
            return Err(DocError::Format {
                format: repr.format,
                expected: PLAN_FORMAT,
            });
        }
        let tier = match (repr.tier, repr.parent) {
            (TierTag::Root, None) => PlanTier::Root,
            (TierTag::Root, Some(parent)) => return Err(DocError::RootWithParent { parent }),
            (TierTag::Sub, Some(parent)) => PlanTier::Sub { parent },
            (TierTag::Sub, None) => return Err(DocError::SubWithoutParent),
            (TierTag::Other(tier), parent) => PlanTier::Other { tier, parent },
        };
        Ok(Self {
            id: repr.plan,
            goal: repr.goal,
            version: repr.version,
            touched: repr.touched,
            tier,
            spawns: repr.spawns,
            todos: repr.todos,
            state: repr.state,
            extra: repr.extra,
        })
    }
}

impl Plan {
    pub fn opening(id: PlanId, goal: GoalText, tier: PlanTier, todos: Vec<Todo>) -> Self {
        Self {
            id,
            goal,
            version: PlanVersion(1),
            touched: TouchCount(1),
            tier,
            spawns: Spawns::default(),
            todos,
            state: PlanState::Active,
            extra: Map::new(),
        }
    }

    pub fn spawns(&self) -> Spawns {
        self.spawns
    }

    pub fn charge_spawn(&mut self) {
        self.spawns = self.spawns.charge();
    }

    pub fn todo(&self, label: &TodoLabel) -> Option<&Todo> {
        self.todos.iter().find(|todo| &todo.label == label)
    }

    /// Ready = Pending with every `after` predecessor cleared, in Vec order —
    /// Vec order is priority, so the first entry is the dispatch pointer.
    pub fn ready(&self) -> Vec<&Todo> {
        self.todos
            .iter()
            .filter(|todo| {
                matches!(todo.state, TodoState::Pending)
                    && todo
                        .after
                        .iter()
                        .all(|label| self.todo(label).is_some_and(|dep| dep.state.clears_edge()))
            })
            .collect()
    }

    /// Finished = every todo terminal (Done, Failed, or Abandoned), not every
    /// todo Done — one dropped todo must not wedge the plan forever.
    pub fn finished(&self) -> bool {
        !self.todos.is_empty() && self.todos.iter().all(|todo| todo.state.is_terminal())
    }

    /// The invariant the fuzzer asserts after every op: labels unique, edges
    /// resolvable, graph acyclic. Empty means valid.
    pub fn validate(&self) -> Vec<PlanIssue> {
        let mut issues = Vec::new();
        let mut seen: HashSet<&TodoLabel> = HashSet::new();
        for todo in &self.todos {
            if !seen.insert(&todo.label) {
                issues.push(PlanIssue::DuplicateLabel {
                    label: todo.label.clone(),
                });
            }
        }
        // Invariant: labels must also be unique after slugging, since a URL addresses a todo
        // by its label's slug and two labels with one slug collide in URL space.
        let mut slugs: HashMap<String, &TodoLabel> = HashMap::new();
        for todo in &self.todos {
            let slug = slugify(todo.label.as_str());
            if let Some(first) = slugs.get(slug.as_str()) {
                if *first != &todo.label {
                    issues.push(PlanIssue::SlugCollision {
                        slug,
                        first: (*first).clone(),
                        second: todo.label.clone(),
                    });
                }
            } else {
                slugs.insert(slug, &todo.label);
            }
        }
        for todo in &self.todos {
            for after in &todo.after {
                if !seen.contains(after) {
                    issues.push(PlanIssue::UnresolvedEdge {
                        todo: todo.label.clone(),
                        after: after.clone(),
                    });
                }
            }
        }
        if !issues.is_empty() {
            return issues;
        }
        let mut pending: HashMap<&TodoLabel, usize> = self
            .todos
            .iter()
            .map(|todo| (&todo.label, todo.after.len()))
            .collect();
        let mut queue: Vec<&TodoLabel> = pending
            .iter()
            .filter(|(_, count)| **count == 0)
            .map(|(label, _)| *label)
            .collect();
        while let Some(done) = queue.pop() {
            pending.remove(done);
            for todo in &self.todos {
                let edges = todo.after.iter().filter(|after| *after == done).count();
                if edges > 0
                    && let Some(count) = pending.get_mut(&todo.label)
                {
                    *count = count.saturating_sub(edges);
                    if *count == 0 {
                        queue.push(&todo.label);
                    }
                }
            }
        }
        if !pending.is_empty() {
            let mut labels: Vec<TodoLabel> = pending.keys().map(|label| (*label).clone()).collect();
            labels.sort();
            issues.push(PlanIssue::Cycle { labels });
        }
        issues
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanIssue {
    DuplicateLabel {
        label: TodoLabel,
    },
    SlugCollision {
        slug: String,
        first: TodoLabel,
        second: TodoLabel,
    },
    UnresolvedEdge {
        todo: TodoLabel,
        after: TodoLabel,
    },
    Cycle {
        labels: Vec<TodoLabel>,
    },
}

impl std::fmt::Display for PlanIssue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateLabel { label } => {
                write!(formatter, "duplicate todo label {:?}", label.as_str())
            }
            Self::SlugCollision {
                slug,
                first,
                second,
            } => write!(
                formatter,
                "todo labels {:?} and {:?} collide at slug {slug:?}",
                first.as_str(),
                second.as_str()
            ),
            Self::UnresolvedEdge { todo, after } => write!(
                formatter,
                "todo {:?} is after {:?}, which is not in the plan",
                todo.as_str(),
                after.as_str()
            ),
            Self::Cycle { labels } => {
                let names: Vec<&str> = labels.iter().map(TodoLabel::as_str).collect();
                write!(formatter, "ordering cycle through {names:?}")
            }
        }
    }
}

impl std::error::Error for PlanIssue {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocError {
    PlanIdEmpty,
    PlanIdTooLong { id: String, max: usize },
    PlanIdChar { id: String },
    PlanIdDepth { id: String },
    SlugEmpty { text: String },
    ChildOfSub { parent: String },
    LabelEmpty,
    LabelTooLong { label: String, max: usize },
    LabelNewline { label: String },
    GoalEmpty,
    GoalTooLong { goal: String, max: usize },
    GoalNewline { goal: String },
    NoteEmpty,
    NoteTooLong { bytes: usize, max: usize },
    ProbeEmpty,
    ProbeNewline { probe: String },
    AgentIdEmpty,
    AgentIdWhitespace { id: String },
    AddrSyntax { addr: String },
    AddrUrl { url: String, cause: UrlError },
    SpawnCeilingExhausted { spent: Spawns, cap: Spawns },
    Format { format: u32, expected: u32 },
    RootWithParent { parent: TodoAddr },
    SubWithoutParent,
    StateFieldMissing { state: String, field: &'static str },
    StateFieldStray { state: String, field: &'static str },
}

impl std::fmt::Display for DocError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PlanIdEmpty => write!(formatter, "plan id is empty"),
            Self::PlanIdTooLong { id, max } => {
                write!(formatter, "plan id {id:?} exceeds {max} bytes")
            }
            Self::PlanIdChar { id } => write!(
                formatter,
                "plan id {id:?} is not lowercase alphanumerics, '-', and one interior '.'"
            ),
            Self::PlanIdDepth { id } => {
                write!(formatter, "plan id {id:?} nests more than one '.'")
            }
            Self::SlugEmpty { text } => {
                write!(formatter, "{text:?} slugifies to nothing")
            }
            Self::ChildOfSub { parent } => {
                write!(formatter, "sub-plan {parent:?} cannot open a child plan")
            }
            Self::LabelEmpty => write!(formatter, "todo label is empty"),
            Self::LabelTooLong { label, max } => {
                write!(formatter, "todo label {label:?} exceeds {max} chars")
            }
            Self::LabelNewline { label } => {
                write!(formatter, "todo label {label:?} contains a newline")
            }
            Self::GoalEmpty => write!(formatter, "goal text is empty"),
            Self::GoalTooLong { goal, max } => {
                write!(formatter, "goal text {goal:?} exceeds {max} chars")
            }
            Self::GoalNewline { goal } => {
                write!(formatter, "goal text {goal:?} contains a newline")
            }
            Self::NoteEmpty => write!(formatter, "inline note is empty"),
            Self::NoteTooLong { bytes, max } => {
                write!(formatter, "inline note of {bytes} bytes exceeds {max}")
            }
            Self::ProbeEmpty => write!(formatter, "probe command is empty"),
            Self::ProbeNewline { probe } => {
                write!(formatter, "probe command {probe:?} contains a newline")
            }
            Self::AgentIdEmpty => write!(formatter, "agent id is empty"),
            Self::AgentIdWhitespace { id } => {
                write!(formatter, "agent id {id:?} contains whitespace")
            }
            Self::AddrSyntax { addr } => {
                write!(formatter, "todo address {addr:?} is not <plan>/<label>")
            }
            Self::AddrUrl { url, cause } => {
                write!(formatter, "todo address url {url:?}: {cause}")
            }
            Self::SpawnCeilingExhausted { spent, cap } => write!(
                formatter,
                "spawn ceiling exhausted: {} spent of cap {}",
                spent.get(),
                cap.get()
            ),
            Self::Format { format, expected } => {
                write!(formatter, "plan format {format} is not {expected}")
            }
            Self::RootWithParent { parent } => {
                write!(formatter, "root plan carries parent {parent}")
            }
            Self::SubWithoutParent => write!(formatter, "sub plan carries no parent"),
            Self::StateFieldMissing { state, field } => {
                write!(formatter, "todo state {state} requires field {field}")
            }
            Self::StateFieldStray { state, field } => {
                write!(formatter, "todo state {state} does not take field {field}")
            }
        }
    }
}

impl std::error::Error for DocError {}
