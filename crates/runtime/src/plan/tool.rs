use std::sync::Arc;

use serde_json::{Map, Value, json};
use yi_tools::{Tool, ToolContext, ToolKind, ToolOutput, error_output_kind, text_output};
use yi_types::plan::doc::{
    BlockedOn, Cites, Delegation, GoalText, PlanId, TODO_LABEL_MAX, Todo, TodoLabel, TodoState,
    TodoStateName, Waiver,
};
use yi_types::url::Url;

use super::ops::{
    Actor, Op, OpRequest, PlanEngine, PlanOpError, Reconciliation, Resolution, Resolve, SetRow,
    TodoSpec,
};
use super::render::render_outcome;
use super::table::{ALL_OPS, OpKind, op_name};
use yi_types::plan::op::{MODEL_OPS, SpecError, TodoSpecRepr};

fn legal_ops() -> String {
    ALL_OPS
        .iter()
        .take(MODEL_OPS)
        .map(|op| op_name(*op))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A key that belongs to another op or level, named where it goes; dogfood sessions sent a
/// block's `on` inside init, and a todo's `label` beside append's `todos`.
pub(super) fn misplaced(op: OpKind, in_todo: bool, key: &str) -> Option<&'static str> {
    match (op, in_todo, key) {
        (_, _, "on" | "note" | "options") if matches!(op, OpKind::Init | OpKind::Append) => Some(
            "; blocking is its own op after the todo exists: op=block, label, on: {user: null}, note, options",
        ),
        (OpKind::Init | OpKind::Append | OpKind::Decompose, false, "label") => {
            Some("; each todo's label goes inside todos: [{label, ...}]")
        }
        (_, true, "todos") => {
            Some("; a todo's own children come from decompose on it once it runs")
        }
        _ => None,
    }
}

/// What the caller evidently meant, per argument: F0e sessions sent prose or a bare path as
/// `output`, and a todo spec without its `spec`, against a schema text they had not read (#472).
pub(super) fn field_hint(field: &str) -> &'static str {
    match field {
        "output" => {
            "; output is a url of the product (tree://<child>/<path> or file:///abs/path), omitted when there is none, and a check's output line belongs to the todo tool's evidence"
        }
        "todos" | "delegation" | "title" | "deps" | "accept" => {
            "; a todo is {label, after?, intent?, waived?, contract?, delegation?: {spec: {role?, isolation?}, accept: {command: \"...\"}}}, and isolation worktree needs a contract"
        }
        "contract" | "check" | "acceptance" => {
            "; a contract is {class, items: [{id, critical, weight, decider: {cmd: \"shell command\"}}]}, or omit it and a worktree delegation's accept {command} is its contract"
        }
        "evidence" => "; evidence is the todo tool's field, done takes output (a url) or nothing",
        "after" => "; a todo's after is a list of labels; add_edge's after is one label",
        "intent" => "; intent is a list of user://<n> addresses, not prose",
        // Incident: three worktree children quoted the attempt nine times between them; the
        // refusal named the wanted type without saying it was the todo's own counter.
        "attempt" => {
            "; attempt is a bare integer, the attempt this todo is on, and 1 unless it was retried"
        }
        _ => "",
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ArgError {
    #[error("op is required; legal ops are {}", legal_ops())]
    NoOp,
    #[error("unknown op {got:?}; legal ops are {}", legal_ops())]
    UnknownOp { got: String },
    #[error("{} requires the {field:?} argument", op_name(*op))]
    Missing { op: OpKind, field: &'static str },
    #[error("{} argument {field:?} is malformed: {cause}{}", op_name(*op), field_hint(field))]
    Malformed {
        op: OpKind,
        field: &'static str,
        cause: serde_json::Error,
    },
    #[error("{} todo {index} is not an object", op_name(*op))]
    TodoShape { op: OpKind, index: usize },
    #[error(transparent)]
    Spec(#[from] SpecError),
    #[error(
        "set line {line} is not a checklist row (`- [ ] label`, `- [>] label`, `- [x] label`; two spaces nest): {text:?}"
    )]
    Checklist { line: usize, text: String },
    #[error("set list is empty")]
    EmptyList,
    #[error("set list nests deeper than {max} levels at line {line}")]
    TooDeep { line: usize, max: usize },
    #[error("actor is not an argument; the surface a request arrives on fixes its principal")]
    ActorArg,
    #[error("{0}")]
    Declared(String),
    #[error("{}{} does not take {key:?}; its arguments are {legal}{}", op_name(*op), todo.map(|at| format!(" todos[{at}]")).unwrap_or_default(), misplaced(*op, todo.is_some(), key).unwrap_or(field_hint(key)))]
    UnknownKey {
        op: OpKind,
        todo: Option<usize>,
        key: String,
        legal: String,
    },
    #[error(
        "only the plan owner may {op}; your plan tool only views: end your turn with your answer and the engine takes it as your work"
    )]
    ChildViews { op: String },
    #[error("{at}: label is {chars} chars, the cap is {max}")]
    LabelTooLong {
        at: String,
        chars: usize,
        max: usize,
    },
}

/// Every key an op reads, `op` and `plan` included; `todo` is the alias for `label`.
pub(super) fn known_keys(kind: OpKind) -> &'static [&'static str] {
    match kind {
        OpKind::Init => &["op", "plan", "goal", "todos"],
        OpKind::Append => &["op", "plan", "todos"],
        OpKind::Unblock | OpKind::Start => &["op", "plan", "label", "todo"],
        OpKind::Drop => &["op", "plan", "label", "todo", "disposition"],
        OpKind::Block => &["op", "plan", "label", "todo", "on", "note", "options"],
        OpKind::Reorder => &["op", "plan", "labels"],
        OpKind::AddEdge => &["op", "plan", "todo", "after"],
        OpKind::Done => &["op", "plan", "label", "todo", "output"],
        OpKind::Fail => &["op", "plan", "label", "todo", "cause", "disposition"],
        OpKind::Retry => &["op", "plan", "label", "todo", "delegation"],
        OpKind::Decompose => &["op", "plan", "label", "todo", "todos"],
        OpKind::Supersede => &["op", "plan", "reason", "todos"],
        OpKind::Set => &["op", "plan", "goal", "list", "todos"],
        OpKind::View => &["op", "plan", "full"],
        OpKind::FuseReset => &["op", "plan"],
        OpKind::Repair => &["op", "plan", "resolutions"],
        OpKind::Import => &["op", "plan", "source"],
        OpKind::Reconcile => &["op", "plan", "label", "todo", "effect_id", "outcome"],
        OpKind::Submit => &["op", "plan", "label", "todo", "attempt", "output"],
        OpKind::Resolve => &["op", "plan", "label", "todo", "attempt", "resolution"],
        OpKind::Accept => &["op", "plan", "label", "todo", "note", "output"],
        OpKind::Program => &["op", "plan", "cell_id", "source_ref"],
    }
}

pub(super) const TODO_SPEC_KEYS: [&str; 6] = [
    "label",
    "after",
    "delegation",
    "contract",
    "intent",
    "waived",
];

/// What [`super::natural::natural`] did not leave out: a key one edit from a known key, or one
/// with a hint saying where it goes.
fn refuse_unknown(
    args: &Map<String, Value>,
    op: OpKind,
    todo: Option<usize>,
    legal: &[&'static str],
) -> Result<(), ArgError> {
    match args.keys().find(|key| !legal.contains(&key.as_str())) {
        Some(key) => Err(ArgError::UnknownKey {
            op,
            todo,
            key: key.clone(),
            legal: legal.join(", "),
        }),
        None => Ok(()),
    }
}

const CHECKLIST_DEPTH: usize = 6;

/// `- [ ]` pending, `- [>]` running, `- [x]` done; indentation nests, two spaces or a tab
/// per level. Children carry their state directly; top rows become `set` rows.
fn parse_checklist(list: &str) -> Result<Vec<SetRow>, ArgError> {
    let mut roots: Vec<Todo> = Vec::new();
    let mut states: Vec<TodoStateName> = Vec::new();
    // Path from a root to the row being read, as indexes into `roots` then `children`.
    let mut path: Vec<(usize, usize)> = Vec::new();
    for (number, raw) in list.lines().enumerate() {
        let line = number.saturating_add(1);
        if raw.trim().is_empty() {
            continue;
        }
        let indent = raw
            .bytes()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .map(|byte| if byte == b'\t' { 2 } else { 1 })
            .sum::<usize>()
            / 2;
        let body = raw.trim_start();
        let (state, label) = ["- [ ] ", "- [>] ", "- [x] ", "- [X] "]
            .iter()
            .find_map(|marker| body.strip_prefix(marker).map(|label| (*marker, label)))
            .map(|(marker, label)| {
                let state = match marker {
                    "- [>] " => TodoStateName::Running,
                    "- [x] " | "- [X] " => TodoStateName::Done,
                    _ => TodoStateName::Pending,
                };
                (state, label.trim())
            })
            .ok_or_else(|| ArgError::Checklist {
                line,
                text: raw.to_owned(),
            })?;
        if indent > CHECKLIST_DEPTH {
            return Err(ArgError::TooDeep {
                line,
                max: CHECKLIST_DEPTH,
            });
        }
        // Incident: one F0e session shortened a label three times and never got under 80,
        // because the headline said the row was not a checklist row (#472).
        let (label, cited) = yi_types::todo::split_cited(label);
        let label = TodoLabel::new(label).map_err(|cause| match cause {
            yi_types::plan::doc::DocError::LabelTooLong { label, max } => ArgError::LabelTooLong {
                at: format!("set line {line}"),
                chars: label.chars().count(),
                max,
            },
            cause => ArgError::Checklist {
                line,
                text: cause.to_string(),
            },
        })?;
        let mut todo = Todo {
            state: match &state {
                TodoStateName::Running => TodoState::Running {
                    by: yi_types::plan::doc::AgentId::new(super::ops::OWNER_AGENT).map_err(
                        |cause| ArgError::Checklist {
                            line,
                            text: cause.to_string(),
                        },
                    )?,
                },
                TodoStateName::Done => TodoState::Done {
                    output: None,
                    resolution: None,
                },
                TodoStateName::Pending
                | TodoStateName::Blocked
                | TodoStateName::Failed
                | TodoStateName::Abandoned
                | TodoStateName::Other(_) => TodoState::Pending,
            },
            ..Todo::pending(label)
        };
        todo.cites.intent = cited;
        let depth = indent.min(path.len());
        path.truncate(depth);
        if depth == 0 {
            roots.push(todo);
            states.push(state);
            path.push((roots.len().saturating_sub(1), 0));
            continue;
        }
        let Some((root_index, _)) = path.first().copied() else {
            return Err(ArgError::Checklist {
                line,
                text: raw.to_owned(),
            });
        };
        let mut parent = roots
            .get_mut(root_index)
            .ok_or_else(|| ArgError::Checklist {
                line,
                text: raw.to_owned(),
            })?;
        for (_, child_index) in path.iter().skip(1) {
            parent = parent
                .children
                .get_mut(*child_index)
                .ok_or_else(|| ArgError::Checklist {
                    line,
                    text: raw.to_owned(),
                })?;
        }
        parent.children.push(todo);
        path.push((0, parent.children.len().saturating_sub(1)));
    }
    if roots.is_empty() {
        return Err(ArgError::EmptyList);
    }
    Ok(roots
        .into_iter()
        .zip(states)
        .map(|(todo, state)| SetRow {
            spec: TodoSpec {
                label: todo.label,
                after: Vec::new(),
                delegation: None,
                contract: None,
                children: todo.children,
                cites: todo.cites,
            },
            state,
        })
        .collect())
}

#[derive(Debug, thiserror::Error)]
pub enum PlanToolError {
    #[error(transparent)]
    Arg(#[from] ArgError),
    #[error(transparent)]
    Op(#[from] PlanOpError),
    #[error(transparent)]
    Submit(#[from] super::authority::SubmitError),
}

/// Invariant: yi-runtime carries no `serde` dependency, so the deserialize
/// bound is a local trait over `serde_json::from_value`.
pub(super) trait FromArg: Sized {
    fn from_arg(value: Value) -> Result<Self, serde_json::Error>;
}

macro_rules! from_arg {
    ($($ty:ty),* $(,)?) => {
        $(impl FromArg for $ty {
            fn from_arg(value: Value) -> Result<Self, serde_json::Error> {
                serde_json::from_value(value)
            }
        })*
    };
}

from_arg!(
    GoalText,
    TodoStateName,
    TodoLabel,
    Vec<TodoLabel>,
    PlanId,
    Url,
    Vec<Url>,
    Vec<Waiver>,
    BlockedOn,
    Delegation,
    yi_types::plan::contract::Contract,
    String,
    bool,
    Vec<Resolution>,
    yi_types::plan::ledger::EffectId,
    yi_types::plan::ledger::AttemptId,
    Reconciliation,
    Resolve,
    yi_types::plan::op::Choice,
    yi_types::plan::op::CellId,
    yi_types::plan::canonical::ArtifactRef,
);

pub(super) fn opt<T: FromArg>(
    args: &Map<String, Value>,
    op: OpKind,
    field: &'static str,
) -> Result<Option<T>, ArgError> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => T::from_arg(value.clone())
            .map(Some)
            .map_err(|cause| ArgError::Malformed { op, field, cause }),
    }
}

pub(super) fn need<T: FromArg>(
    args: &Map<String, Value>,
    op: OpKind,
    field: &'static str,
) -> Result<T, ArgError> {
    opt(args, op, field)?.ok_or(ArgError::Missing { op, field })
}

pub(super) fn label(args: &Map<String, Value>, op: OpKind) -> Result<TodoLabel, ArgError> {
    match opt(args, op, "label")? {
        Some(label) => Ok(label),
        None => need(args, op, "todo").map_err(|_| ArgError::Missing { op, field: "label" }),
    }
}

pub(super) fn todo_specs(args: &Map<String, Value>, op: OpKind) -> Result<Vec<TodoSpec>, ArgError> {
    let field = "todos";
    let raw: &Vec<Value> = match args.get(field) {
        Some(Value::String(text)) => match serde_json::from_str::<Vec<Value>>(text) {
            Err(cause) => return Err(ArgError::Malformed { op, field, cause }),
            Ok(_) => None,
        },
        other => other.and_then(Value::as_array),
    }
    .ok_or(ArgError::Missing { op, field })?;
    let mut specs = Vec::with_capacity(raw.len());
    for (index, value) in raw.iter().enumerate() {
        let Value::Object(spec) = value else {
            return Err(ArgError::TodoShape { op, index });
        };
        refuse_unknown(spec, op, Some(index), &TODO_SPEC_KEYS)?;
        specs.push(TodoSpec::try_from(TodoSpecRepr {
            label: need(spec, op, "label").map_err(|error| {
                let label = spec.get("label").and_then(Value::as_str);
                let (chars, max) = (label.unwrap_or_default().chars().count(), TODO_LABEL_MAX);
                let at = format!("{} todos[{index}]", op_name(op));
                if chars > max {
                    ArgError::LabelTooLong { at, chars, max }
                } else {
                    error
                }
            })?,
            after: opt(spec, op, "after")?.unwrap_or_default(),
            delegation: opt(spec, op, "delegation")?,
            contract: opt(spec, op, "contract")?,
            children: Vec::new(),
            cites: Cites {
                intent: opt(spec, op, "intent")?.unwrap_or_default(),
                waived: opt(spec, op, "waived")?.unwrap_or_default(),
            },
        })?);
    }
    Ok(specs)
}

/// `accept` is the word every surface takes for the `accepted_by_user` record.
fn parse_op(args: &Map<String, Value>) -> Result<Op, ArgError> {
    let name = match args.get("op").and_then(Value::as_str) {
        Some("accept") => "accepted_by_user",
        Some(name) => name,
        None => return Err(ArgError::NoOp),
    };
    let kind = ALL_OPS
        .into_iter()
        .find(|op| op_name(*op) == name)
        .ok_or_else(|| ArgError::UnknownOp {
            got: name.to_owned(),
        })?;
    refuse_unknown(args, kind, None, known_keys(kind))?;
    Ok(match kind {
        OpKind::Init => Op::Init {
            goal: need(args, kind, "goal")?,
            todos: todo_specs(args, kind)?,
        },
        OpKind::Append => Op::Append {
            todos: todo_specs(args, kind)?,
        },
        OpKind::Drop => Op::Drop {
            label: label(args, kind)?,
            disposition: opt(args, kind, "disposition")?,
        },
        OpKind::Block => super::ask::block(args, kind)?,
        OpKind::Unblock => Op::Unblock {
            label: label(args, kind)?,
            answer: None,
        },
        OpKind::Reorder => Op::Reorder {
            labels: need(args, kind, "labels")?,
        },
        OpKind::AddEdge => Op::AddEdge {
            todo: need(args, kind, "todo")?,
            after: need(args, kind, "after")?,
        },
        OpKind::Start => Op::Start {
            label: label(args, kind)?,
        },
        OpKind::Done => Op::Done {
            label: label(args, kind)?,
            output: opt(args, kind, "output")?,
        },
        OpKind::Fail => Op::Fail {
            label: label(args, kind)?,
            cause: need(args, kind, "cause")?,
            disposition: opt(args, kind, "disposition")?,
        },
        OpKind::Retry => Op::Retry {
            label: label(args, kind)?,
            delegation: opt::<Delegation>(args, kind, "delegation")?.map(Box::new),
        },
        OpKind::Decompose => Op::Decompose {
            label: label(args, kind)?,
            todos: todo_specs(args, kind)?,
        },
        OpKind::Supersede => Op::Supersede {
            reason: need(args, kind, "reason")?,
            todos: todo_specs(args, kind)?,
        },
        OpKind::Set => Op::Set {
            goal: opt(args, kind, "goal")?,
            rows: match args.get("todos") {
                Some(Value::Array(_)) if !args.contains_key("list") => {
                    super::apply::set_rows(args, kind)?
                }
                _ => parse_checklist(&need::<String>(args, kind, "list")?)?,
            },
        },
        OpKind::View => Op::View {
            full: opt(args, kind, "full")?.unwrap_or(false),
        },
        OpKind::FuseReset => Op::FuseReset,
        OpKind::Repair => Op::Repair {
            resolutions: opt::<Vec<Resolution>>(args, kind, "resolutions")?.unwrap_or_default(),
        },
        OpKind::Import => Op::Import {
            source: need(args, kind, "source")?,
        },
        OpKind::Reconcile => Op::Reconcile {
            label: label(args, kind)?,
            effect_id: opt(args, kind, "effect_id")?,
            outcome: need::<Reconciliation>(args, kind, "outcome")?,
        },
        OpKind::Submit => Op::Submit {
            label: label(args, kind)?,
            attempt: need(args, kind, "attempt")?,
            output: need(args, kind, "output")?,
        },
        OpKind::Resolve => Op::Resolve {
            label: label(args, kind)?,
            attempt: need(args, kind, "attempt")?,
            resolution: need(args, kind, "resolution")?,
        },
        OpKind::Accept => Op::Accept {
            label: label(args, kind)?,
            note: need(args, kind, "note")?,
            output: opt(args, kind, "output")?,
        },
        OpKind::Program => Op::Program {
            cell_id: need(args, kind, "cell_id")?,
            source_ref: need(args, kind, "source_ref")?,
        },
    })
}

pub(super) fn declared(
    actor: &Actor,
    args: &Map<String, Value>,
) -> Result<(OpRequest, Vec<super::declare::Blob>), ArgError> {
    if let (Actor::Child(_), Some(op)) = (actor, args.get("op").and_then(Value::as_str))
        && !args.contains_key("actor")
        && !matches!(op, "view" | "submit")
    {
        return Err(ArgError::ChildViews { op: op.to_owned() });
    }
    let (args, blobs) = super::declare::normalize(args).map_err(ArgError::Declared)?;
    Ok((request(actor, &args)?, blobs))
}

/// Invariant: the principal is a channel, never a string (§3.6): `actor` is refused, never read.
pub(super) fn request(actor: &Actor, args: &Map<String, Value>) -> Result<OpRequest, ArgError> {
    if args.contains_key("actor") {
        return Err(ArgError::ActorArg);
    }
    let op = parse_op(args)?;
    Ok(OpRequest {
        plan: opt(args, op.kind(), "plan")?,
        actor: actor.clone(),
        op,
        request_id: None,
        expected_revision: None,
    })
}

pub struct PlanTool {
    engine: Arc<PlanEngine>,
    actor: Actor,
    confirming: Option<super::authority::Confirming>,
}

impl PlanTool {
    pub fn new(engine: Arc<PlanEngine>, actor: Actor) -> Self {
        Self {
            engine,
            actor,
            confirming: None,
        }
    }

    pub fn confirming(self, confirming: super::authority::Confirming) -> Self {
        Self {
            confirming: Some(confirming),
            ..self
        }
    }

    pub(super) fn run(&self, args: &Map<String, Value>) -> Result<String, PlanToolError> {
        if super::apply::wants(args) {
            return super::apply::apply(self, args);
        }
        let mut args = args.clone();
        let blocks = super::natural::blocks(&mut args);
        let mut text = self.apply(&args)?;
        for block in &blocks {
            match self.apply(block) {
                Ok(reply) => text = reply,
                Err(err) => {
                    return Err(ArgError::Declared(format!("{text}\nblock refused: {err}")).into());
                }
            }
        }
        if !blocks.is_empty() {
            text.push_str("\nnote: a todo's on became its own block once the todo existed");
        }
        Ok(text)
    }

    pub(super) fn actor(&self) -> &Actor {
        &self.actor
    }

    pub(super) fn engine(&self) -> &PlanEngine {
        &self.engine
    }

    /// One op through the natural reading and its retries, without the whole-plan apply.
    pub(super) fn apply_one(&self, args: &Map<String, Value>) -> Result<String, PlanToolError> {
        self.apply(args)
    }

    fn apply(&self, args: &Map<String, Value>) -> Result<String, PlanToolError> {
        let (args, mut said) = super::natural::natural(args);
        if let Some(Value::Array(labels)) = args.get("labels")
            && (args.get("op").and_then(Value::as_str))
                .is_some_and(|op| super::natural::TARGETED.contains(&op))
        {
            return self.each(&args, labels, said);
        }
        let (request, blobs) = declared(&self.actor, &args)?;
        if let Op::Accept { .. } = &request.op {
            let confirming = self.confirming.as_ref();
            return Ok(
                super::authority::accept(&self.engine, &self.actor, confirming, &args)?.text(),
            );
        }
        let op = request.op.clone();
        let outcome = match self.engine.apply_with(request.clone(), &blobs) {
            Err(PlanOpError::IllegalStep {
                label,
                from: TodoStateName::Pending,
                op: kind @ (OpKind::Done | OpKind::Decompose | OpKind::Fail | OpKind::Retry),
            }) if self.runs_itself(&request, &label) => {
                let start = OpRequest {
                    op: Op::Start { label },
                    ..request.clone()
                };
                if kind == OpKind::Retry {
                    said.push("it had not run yet, so it was started".to_owned());
                    self.engine.apply(start)?
                } else {
                    self.engine.apply(start)?;
                    said.push("it was pending, so it was started first".to_owned());
                    self.engine.apply_with(request, &blobs)?
                }
            }
            Err(PlanOpError::IllegalStep {
                from: TodoStateName::Pending | TodoStateName::Running,
                op: OpKind::Unblock,
                ..
            }) => {
                said.push("it was not blocked, so nothing changed".to_owned());
                self.engine.apply(OpRequest {
                    op: Op::View { full: false },
                    ..request
                })?
            }
            Err(
                PlanOpError::NoPlan | PlanOpError::Store(super::store::StoreError::Missing { .. }),
            ) if request.plan.is_some() => {
                let mut again = args.clone();
                again.remove("plan");
                let mut text = self.run(&again)?;
                said.push(
                    "the plan named is not open, so the call ran on the open plan".to_owned(),
                );
                for line in said {
                    text.push_str(&format!("\nnote: {line}"));
                }
                return Ok(text);
            }
            Err(refused @ (PlanOpError::NoPlan | PlanOpError::NotActive { .. })) => {
                let first = match &request.op {
                    Op::Set { goal: None, rows } => rows.first().map(|row| row.spec.label.clone()),
                    Op::Append { todos } | Op::Supersede { todos, .. } => {
                        todos.first().map(|todo| todo.label.clone())
                    }
                    _ => None,
                };
                let Some(first) = first else {
                    return Err(refused.into());
                };
                let goal = GoalText::new(first.as_str()).map_err(PlanOpError::Doc)?;
                let op = match request.op.clone() {
                    Op::Set { rows, .. } => Op::Set {
                        goal: Some(goal),
                        rows,
                    },
                    Op::Append { todos } | Op::Supersede { todos, .. } => Op::Init { goal, todos },
                    other => other,
                };
                said.push(
                    "no plan was open, so one was opened, named after the first todo".to_owned(),
                );
                let opened = OpRequest {
                    op,
                    plan: None,
                    ..request
                };
                match self.engine.apply_with(opened, &blobs) {
                    Err(PlanOpError::PlanExists { .. }) => return Err(refused.into()),
                    other => other?,
                }
            }
            Err(PlanOpError::NotAPermutation { .. }) => {
                let Op::Reorder { labels } = &request.op else {
                    return Err(PlanOpError::NotAPermutation {
                        got: 0,
                        expected: 0,
                    }
                    .into());
                };
                let view = OpRequest {
                    op: Op::View { full: true },
                    ..request.clone()
                };
                let mut order = labels.clone();
                let rest: Vec<TodoLabel> = (self.engine.apply(view)?.plan.todos.iter())
                    .map(|todo| todo.label.clone())
                    .filter(|label| !labels.contains(label))
                    .collect();
                order.extend(rest);
                said.push("the todos not named keep their order after the named ones".to_owned());
                let reorder = Op::Reorder { labels: order };
                self.engine.apply_with(
                    OpRequest {
                        op: reorder,
                        ..request
                    },
                    &blobs,
                )?
            }
            other => other?,
        };
        let mut text = render_outcome(&op, &outcome);
        for line in said {
            text.push_str(&format!("\nnote: {line}"));
        }
        Ok(text)
    }

    /// The owner's own todo, not one a child would be spawned for, so starting it costs nothing.
    fn runs_itself(&self, request: &OpRequest, label: &TodoLabel) -> bool {
        let view = OpRequest {
            op: Op::View { full: true },
            ..request.clone()
        };
        self.actor == Actor::Owner
            && (self.engine.apply(view).ok()).is_some_and(|seen| {
                seen.plan
                    .todo(label)
                    .is_some_and(|t| t.delegation.is_none())
            })
    }
}

impl Tool for PlanTool {
    fn name(&self) -> &str {
        "plan"
    }

    fn description(&self) -> &str {
        match self.actor {
            Actor::Child(_) => CHILD_DESCRIPTION,
            _ => DESCRIPTION,
        }
    }

    fn schema(&self) -> Value {
        match self.actor {
            Actor::Child(_) => json!({
                "type": "object",
                "properties": {
                    "op": {"type": "string", "enum": ["view"]},
                    "full": {"type": "boolean", "description": "every todo instead of counts plus the frontier"}
                },
                "required": ["op"]
            }),
            _ => schema(),
        }
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Ledger
    }

    fn irreversible(&self, _input: &Map<String, Value>) -> bool {
        false
    }

    fn validate(&self, input: &Map<String, Value>) -> Result<(), String> {
        if super::apply::wants(input) {
            return Ok(());
        }
        (super::natural::calls(input).iter())
            .try_for_each(|call| declared(&self.actor, call).map(|_| ()))
            .map_err(|error| error.to_string())
    }

    fn arms(&self, input: &Map<String, Value>) -> Vec<String> {
        input
            .get("on")
            .and_then(|on| serde_json::from_value::<BlockedOn>(on.clone()).ok())
            .and_then(|on| crate::schedule::clock::armed_command(&on))
            .into_iter()
            .collect()
    }

    fn execute(&self, input: Map<String, Value>, _context: &ToolContext) -> ToolOutput {
        match self.run(&input) {
            Ok(text) => text_output(text),
            Err(error) => error_output_kind(error.to_string(), error.kind()),
        }
    }
}

/// Invariant: the request-prefix gate prices this tool through these two
/// items rather than a live [`PlanTool`], so what is measured is what ships.
pub const DESCRIPTION: &str = "The plan ledger. op=set with a markdown checklist (`- [ ] todo`, `- [>] running`, `- [x] done`, two spaces nest) is the whole list in one call: send it again to change anything. Declare todos with contracts and delegations; the engine starts, verifies and accepts delegated ones. done closes your own todos. A todo is a unit of decision, not of iteration. Batch ops with real work; never call it alone. It is the delegation ledger; the todo list shows it.";

const CHILD_DESCRIPTION: &str = "The plan that dispatched you, read-only: op=view. When your work is done, end your turn with your answer; the engine takes it as your work and accepts or refuses it.";

/// Invariant: a flat object, because one provider rebuilds the schema from `properties` and
/// `required` alone, so a root `oneOf` would vanish there. No live state: cached prefix.
pub fn schema() -> Value {
    let contract = json!({"type": "object", "required": ["class", "items"], "description": "what must hold when the todo is done; done runs its items and passes at the threshold", "properties": {
        "class": {"type": "string", "enum": ["writer", "reader", "inline"], "description": "writer needs a critical cmd or example item, reader a critical schema item, inline either"}, "covers": {"type": "array", "items": {"type": "string"}, "description": "globs whose writes preview the cmd items"}, "threshold": {"type": "integer", "minimum": 1, "maximum": 1000, "description": "thousandths of the decided weight that must pass; default 1000"}, "min_coverage": {"type": "integer", "minimum": 1, "maximum": 1000, "description": "thousandths of the whole weight that must decide rather than abstain; default 1000"},
        "items": {"type": "array", "minItems": 1, "items": {"type": "object", "required": ["id", "critical", "weight", "decider"], "properties": {"id": {"type": "string", "description": "the item's name, unique in the contract"}, "critical": {"type": "boolean", "description": "a failed critical item fails the todo, an abstaining one holds it"}, "weight": {"type": "integer", "minimum": 1, "maximum": 100, "description": "the item's share of the score"},
            "decider": {"type": "object", "description": format!("{{cmd: \"shell command that exits 0 only when the item holds\"}}, or {{cmd: {{checker: command, timeout_ms}}}} (default and ceiling {}); {{schema: {{schema: artifact}}}}; {{example: {{cases: artifact, runner: artifact, timeout_ms}}}}; an artifact is {{digest, media_type, length}}", crate::goal::DEFAULT_CHECK_TIMEOUT_MS)}}}}}});
    json!({
            "type": "object",
            "properties": {
                "op": {
                    "type": "string",
                    "enum": ALL_OPS.iter().take(MODEL_OPS).map(|op| op_name(*op)).collect::<Vec<_>>(),
                    "description": "set states the whole plan, as a checklist list or as todos rows with a state, and lands with each row it could not reach saying why; init goal+todos; append/drop/reorder/add_edge edit the cut; start/done/fail step a todo; block/unblock park one; retry a failed one; decompose a running one into a sub-plan; supersede replaces the whole cut; view echoes it; accepted_by_user asks the user, or the classifier in auto mode, to close a todo whose check cannot run here"
                },
                "list": {"type": "string", "description": "set: the checklist, one `- [ ] label` per line (`[>]` running, `[x]` done), nested by two-space indent; trailing `user://<n>` tokens cite the user's messages the row serves"},
                "plan": {"type": "string", "description": "Sub-plan id; omit for the root plan"},
                "goal": {"type": "string", "description": "init: the whole deliverable in one line"},
                "todos": {"type": "array", "minItems": 1, "description": "set/init/append/decompose/supersede: the todos. A delegation hands the todo to a child and its accept is mandatory; isolation worktree requires a contract, as does container:<image>, the same worktree with its bash run in a container of that image; done runs the contract", "items": {"type": "object", "required": ["label"], "properties": {
                    "label": {"type": "string", "maxLength": TODO_LABEL_MAX, "description": format!("the todo's name, imperative, at most {TODO_LABEL_MAX} chars")},
                    "state": {"type": "string", "enum": ["pending", "running", "done", "blocked"], "description": "set: the state the row should reach; done runs its contract first, blocked asks the user (on, note, options as in block); a row the engine cannot move says why in the reply"}, "after": {"type": "array", "items": {"type": "string"}, "description": "labels of the todos this one waits on"},
                    "intent": {"type": "array", "items": {"type": "string"}, "description": "user://<n> of each user message it serves; default the latest"}, "waived": {"type": "array", "items": {"type": "object"}, "description": "[{address, reason}]: a user message the plan leaves unserved"},
                    "delegation": {"type": "object", "description": "{spec: {role?, model?, effort?, isolation?}, accept: {command} | {stated}, context?: [url], output?: {schema: url}}"},
                    "contract": contract}}},
                "label": {"type": "string", "description": "drop/block/unblock/start/done/fail/retry/decompose/accepted_by_user: the todo"},
                "labels": {"type": "array", "items": {"type": "string"}, "description": "reorder: every label of the plan, in the new priority order"},
                "todo": {"type": "string", "description": "add_edge: the todo that waits"},
                "after": {"type": "string", "description": "add_edge: the sibling it waits on"},
                "on": {"type": "object", "description": "block: {\"child\": agent} | {\"user\": null} | {\"external\": {\"probe\": command}} | {\"channel\": {\"address\": \"clock://at <ISO time>\" or an exec://, file:// or channel:// address as in todo, \"filter\"?}}, unblocked by the first match"},
                "note": {"type": "string", "description": "block: what would unblock it; accepted_by_user: the check you ran by hand and its exit line"},
                "options": {"type": "array", "items": {"type": "object"}, "description": "block on user: 3 to 5 answers [{id, label, preview?}] the user picks one of by replying with its number, id or label; a preview is light (a line, a small diagram's source, or an address), at most 2048 bytes"},
                "cause": {"type": "string", "description": "fail: what went wrong"},
                "output": {"type": "string", "description": "done: url of the product; required when the delegation declares an output schema"},
                "delegation": {"type": "object", "description": "retry: replacement delegation, shaped as in todos"},
                "reason": {"type": "string", "description": "supersede: why the cut is wrong"},
                "full": {"type": "boolean", "description": "view: every todo instead of counts plus the frontier"}
            },
            "required": ["op"]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    type Fallible = Result<(), Box<dyn std::error::Error>>;

    fn fixture_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plans")
    }

    /// `repeat` and `reapFails` are the walkthrough's own keys, stripped before the engine
    /// sees the args, so the parser never learns them.
    fn strip_walkthrough_keys(args: &mut Map<String, Value>) {
        args.remove("reapFails");
        if let Some(Value::Array(todos)) = args.get_mut("todos") {
            for todo in todos.iter_mut().filter_map(Value::as_object_mut) {
                todo.remove("repeat");
            }
        }
    }

    #[test]
    fn every_fixture_op_argument_set_parses() -> Fallible {
        let mut seen = 0usize;
        for entry in std::fs::read_dir(fixture_dir())? {
            let path = entry?.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            let fixture: Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
            let Some(steps) = fixture.get("steps").and_then(Value::as_array) else {
                continue;
            };
            for step in steps {
                let Some(name) = step.get("op").and_then(Value::as_str) else {
                    continue;
                };
                let mut input = match step.get("args") {
                    Some(Value::Object(args)) => args.clone(),
                    _ => Map::new(),
                };
                strip_walkthrough_keys(&mut input);
                input.insert("op".to_owned(), Value::String(name.to_owned()));
                if let Some(plan) = step.get("plan") {
                    input.insert("plan".to_owned(), plan.clone());
                }
                let parsed = request(&Actor::Owner, &input);
                assert!(parsed.is_ok(), "{name}: {:?}", parsed.err());
                assert!(templated(&input).is_ok(), "{name}: {:?}", templated(&input));
                // The CLI reads one line: the same args must parse to the same request.
                let mut line = name.to_owned();
                if let Some(plan) = step.get("plan").and_then(Value::as_str) {
                    line.push(' ');
                    line.push_str(plan);
                }
                if let Some(Value::Object(args)) = step.get("args") {
                    line.push(' ');
                    line.push_str(&serde_json::to_string(args)?);
                }
                let mut through_cli = super::super::authority::cli_args(&line)
                    .map_err(|error| format!("{name}: {error}"))?;
                strip_walkthrough_keys(&mut through_cli);
                assert_eq!(
                    request(&Actor::Owner, &through_cli).ok(),
                    parsed.ok(),
                    "{name}: the CLI line parses differently from the tool"
                );
                seen = seen.saturating_add(1);
            }
        }
        assert!(seen > 30, "only {seen} fixture ops exercised");
        Ok(())
    }

    /// The call a repeated refusal prints back, parsed by the parser that refused it.
    fn templated(args: &Map<String, Value>) -> Result<(), String> {
        let line = crate::affordance::call_template("plan", &schema(), args);
        let shape = line
            .strip_prefix(&format!("{}call plan as ", crate::affordance::NEXT))
            .ok_or_else(|| format!("no template in {line}"))?;
        let Ok(Value::Object(shape)) = serde_json::from_str::<Value>(shape) else {
            return Err(format!("{shape} is not an object"));
        };
        request(&Actor::Owner, &shape)
            .map(|_| ())
            .map_err(|error| format!("{shape:?}: {error}"))
    }

    /// Incident: the template printed a submit without `attempt`, a key the flat schema does not
    /// name, so the surface taught the one call its own parser refuses (#478).
    #[test]
    fn a_submit_template_carries_the_attempt_its_parser_needs() -> Fallible {
        let Value::Object(args) = json!({
            "op": "submit",
            "plan": "three-independent-one-file-writes-alpha",
            "label": "gamma",
            "attempt": 1,
            "output": "local://gamma.txt"
        }) else {
            return Err("case is not an object".into());
        };
        templated(&args)?;
        Ok(())
    }

    #[test]
    fn schema_is_byte_identical_between_calls() -> Fallible {
        assert_eq!(
            serde_json::to_string(&schema())?,
            serde_json::to_string(&schema())?
        );
        Ok(())
    }

    #[test]
    fn an_unknown_argument_key_is_refused() -> Fallible {
        let cases = [
            json!({"op": "done", "label": "x", "outpt": "local://a"}),
            json!({"op": "set", "list": "- [ ] x", "expected_revision": 3}),
            json!({"op": "init", "goal": "ship it", "todos": [{"label": "x", "contrat": {}}]}),
        ];
        for case in cases {
            let Value::Object(input) = case else {
                return Err("case is not an object".into());
            };
            let message = request(&Actor::Owner, &input)
                .err()
                .ok_or_else(|| format!("{input:?} parsed with an unknown key"))?
                .to_string();
            assert!(message.contains("does not take"), "{message}");
            assert!(
                message.contains("outpt")
                    || message.contains("expected_revision")
                    || message.contains("contrat"),
                "{message}"
            );
        }
        Ok(())
    }

    #[test]
    fn an_unknown_op_names_the_legal_ones() -> Fallible {
        let mut input = Map::new();
        input.insert("op".to_owned(), Value::String("plan.create".to_owned()));
        let message = request(&Actor::Owner, &input)
            .err()
            .ok_or("plan.create parsed as an op")?
            .to_string();
        assert!(message.contains("init"), "{message}");
        assert!(message.contains("supersede"), "{message}");
        Ok(())
    }

    #[test]
    fn a_checklist_parses_into_nested_rows_with_states() -> Fallible {
        let rows = parse_checklist(
            "- [x] mapper\n- [>] rebase\n  - [x] remap\n  - [ ] prompt\n    - [ ] rule one\n- [ ] bridge\n",
        )?;
        let labels: Vec<&str> = rows.iter().map(|row| row.spec.label.as_str()).collect();
        assert_eq!(labels, ["mapper", "rebase", "bridge"]);
        assert_eq!(rows[0].state, TodoStateName::Done);
        assert_eq!(rows[1].state, TodoStateName::Running);
        assert_eq!(rows[1].spec.children.len(), 2);
        assert_eq!(
            rows[1].spec.children[1].children[0].label.as_str(),
            "rule one"
        );
        assert!(matches!(
            rows[1].spec.children[0].state,
            TodoState::Done { .. }
        ));

        let bad = parse_checklist("- mapper\n");
        assert!(bad.is_err());
        let mut input = Map::new();
        input.insert("op".to_owned(), Value::String("set".to_owned()));
        input.insert("list".to_owned(), Value::String("- [ ] a\n".to_owned()));
        assert!(matches!(parse_op(&input)?, Op::Set { .. }));
        Ok(())
    }

    /// Guards the actor refusal in `request`: drop the `actor` key check and the child's
    /// `view` below is honoured as the owner.
    #[test]
    fn the_tool_and_the_request_refuse_an_actor_argument() -> Fallible {
        let dir = crate::scratch::Scratch::new("yi-plan-tool-actor")?;
        let store = super::super::store::PlanStore::open(dir.to_path_buf())?;
        let engine = Arc::new(PlanEngine::new(store, Arc::new(NoChildren)));
        let child = Actor::Child(yi_types::plan::doc::AgentId::new("helper")?);
        let mut args = Map::new();
        args.insert("op".to_owned(), Value::String("init".to_owned()));
        args.insert("goal".to_owned(), Value::String("ship the seam".to_owned()));
        args.insert("todos".to_owned(), json!([{"label": "cut"}]));
        args.insert("actor".to_owned(), Value::String("agent://main".to_owned()));
        let tool = PlanTool::new(Arc::clone(&engine), child.clone());
        let output = tool.execute(args.clone(), &ToolContext::new(dir.to_path_buf()));
        assert!(output.is_error, "the tool honoured an actor argument");
        let refused = match output.result.content.first() {
            Some(yi_types::message::Content::Text { text, .. }) => text.clone(),
            _ => String::new(),
        };
        assert!(
            refused.contains("actor is not an argument"),
            "the tool refuses the key itself, not the op: {refused}"
        );
        let refusal = super::super::request::refusal_of(&child, &args);
        assert_eq!(refusal["refusal"]["code"], json!("bad_args"));
        assert!(
            refusal["refusal"]["message"]
                .as_str()
                .is_some_and(|text| text.contains("actor is not an argument")),
            "{refusal:?}"
        );
        assert!(
            engine.revision(None).is_err(),
            "no plan may open through a claimed actor"
        );
        let line = format!("init {}", serde_json::to_string(&args)?);
        let through_cli = super::super::authority::cli_args(&line)?;
        assert!(
            matches!(request(&child, &through_cli), Err(ArgError::ActorArg)),
            "the CLI line carries the key into the same refusal"
        );
        Ok(())
    }

    #[test]
    fn a_childs_plan_tool_views_and_is_refused_before_the_parse() -> Fallible {
        let dir = crate::scratch::Scratch::new("yi-plan-tool-child")?;
        let store = super::super::store::PlanStore::open(dir.to_path_buf())?;
        let engine = Arc::new(PlanEngine::new(store, Arc::new(NoChildren)));
        let child = Actor::Child(yi_types::plan::doc::AgentId::new("helper")?);
        let tool = PlanTool::new(engine, child.clone());
        assert_eq!(tool.schema()["properties"]["op"]["enum"], json!(["view"]));
        assert!(tool.description().contains("end your turn"));
        let mut args = Map::new();
        args.insert("op".to_owned(), Value::String("done".to_owned()));
        let refusal = super::super::request::refusal_of(&child, &args);
        assert_eq!(
            refusal["refusal"]["code"],
            json!("not_owner"),
            "{refusal:?}"
        );
        assert!(
            refusal["refusal"]["message"]
                .as_str()
                .is_some_and(|text| text.contains("end your turn")),
            "{refusal:?}"
        );
        Ok(())
    }

    /// Dies with the `try_from` on `TodoSpec`: build the fields straight and an uncontracted
    /// worktree todo lands through `init`, `append`, `decompose` and `supersede` on both roads.
    #[test]
    fn an_uncontracted_worktree_todo_is_refused_by_the_parse_on_every_declaring_op() -> Fallible {
        let bare = json!({"label": "build it apart", "delegation": {
            "spec": {"role": "writer", "isolation": "worktree"},
            "accept": {"stated": "the contract decides"}
        }});
        for (op, extra) in [
            ("init", json!({"goal": "ship it"})),
            ("append", json!({})),
            ("decompose", json!({"label": "parent"})),
            ("supersede", json!({"reason": "again"})),
        ] {
            let mut args = extra.as_object().cloned().unwrap_or_default();
            args.insert("op".to_owned(), Value::String(op.to_owned()));
            args.insert("todos".to_owned(), json!([bare.clone()]));
            let parsed = parse_op(&args);
            assert!(
                matches!(&parsed, Err(ArgError::Spec(_))),
                "{op} parsed the uncontracted shape: {parsed:?}"
            );
            let text = parsed
                .err()
                .map(|error| error.to_string())
                .unwrap_or_default();
            assert!(
                text.starts_with("todo build it apart: a worktree delegation needs a `contract`"),
                "{text}"
            );
            let refusal = super::super::request::refusal_of(&Actor::Owner, &args);
            assert_eq!(
                refusal["refusal"]["code"],
                json!("bad_args"),
                "{op}: {refusal:?}"
            );
        }
        let mut args = Map::new();
        args.insert("op".to_owned(), Value::String("append".to_owned()));
        args.insert(
            "todos".to_owned(),
            json!([{"label": "build it here", "delegation": {
                "spec": {"role": "writer"}, "accept": {"stated": "the contract decides"}
            }}]),
        );
        assert!(
            parse_op(&args).is_ok(),
            "an inline delegation needs no contract"
        );
        Ok(())
    }

    struct NoChildren;

    impl super::super::ops::Delegate for NoChildren {
        fn spawn(
            &self,
            _at: &yi_types::plan::doc::TodoAddr,
            _delegation: &Delegation,
        ) -> Result<yi_types::plan::doc::AgentId, String> {
            Err("no children in this test".to_owned())
        }

        fn reap(
            &self,
            _agent: &yi_types::plan::doc::AgentId,
            _supplied: &[Url],
        ) -> Result<Option<Url>, String> {
            Ok(None)
        }
    }
}
