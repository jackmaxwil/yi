use std::sync::Arc;

use serde_json::{Map, Value, json};
use yi_tools::{Tool, ToolContext, ToolKind, ToolOutput, error_output, text_output};
use yi_types::plan::doc::{
    BlockedOn, Delegation, GoalText, Plan, PlanId, PlanTier, Todo, TodoLabel, TodoState,
    TodoStateName,
};
use yi_types::url::Url;

use super::ops::{Actor, Op, OpRequest, Outcome, PlanEngine, PlanOpError, TodoSpec};
use super::table::{OpKind, op_name};

const WINDOW: usize = 8;

const ALL_OPS: [OpKind; 14] = [
    OpKind::Init,
    OpKind::Append,
    OpKind::Drop,
    OpKind::Block,
    OpKind::Unblock,
    OpKind::Reorder,
    OpKind::AddEdge,
    OpKind::Start,
    OpKind::Done,
    OpKind::Fail,
    OpKind::Retry,
    OpKind::Decompose,
    OpKind::Supersede,
    OpKind::View,
];

fn legal_ops() -> String {
    ALL_OPS
        .iter()
        .map(|op| op_name(*op))
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Debug, thiserror::Error)]
pub enum ArgError {
    #[error("op is required; legal ops are {}", legal_ops())]
    NoOp,
    #[error("unknown op {got:?}; legal ops are {}", legal_ops())]
    UnknownOp { got: String },
    #[error("{} requires the {field:?} argument", op_name(*op))]
    Missing { op: OpKind, field: &'static str },
    #[error("{} argument {field:?} is malformed: {cause}", op_name(*op))]
    Malformed {
        op: OpKind,
        field: &'static str,
        cause: serde_json::Error,
    },
    #[error("{} todo {index} is not an object", op_name(*op))]
    TodoShape { op: OpKind, index: usize },
}

#[derive(Debug, thiserror::Error)]
pub enum PlanToolError {
    #[error(transparent)]
    Arg(#[from] ArgError),
    #[error(transparent)]
    Op(#[from] PlanOpError),
}

/// Invariant: yi-runtime carries no `serde` dependency, so the deserialize
/// bound is a local trait over `serde_json::from_value`.
trait FromArg: Sized {
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
    TodoLabel,
    Vec<TodoLabel>,
    PlanId,
    Url,
    BlockedOn,
    Delegation,
    String,
    bool,
);

fn opt<T: FromArg>(
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

fn need<T: FromArg>(
    args: &Map<String, Value>,
    op: OpKind,
    field: &'static str,
) -> Result<T, ArgError> {
    opt(args, op, field)?.ok_or(ArgError::Missing { op, field })
}

fn todo_specs(args: &Map<String, Value>, op: OpKind) -> Result<Vec<TodoSpec>, ArgError> {
    let field = "todos";
    let raw: &Vec<Value> = args
        .get(field)
        .and_then(Value::as_array)
        .ok_or(ArgError::Missing { op, field })?;
    let mut specs = Vec::with_capacity(raw.len());
    for (index, value) in raw.iter().enumerate() {
        let Value::Object(spec) = value else {
            return Err(ArgError::TodoShape { op, index });
        };
        specs.push(TodoSpec {
            label: need(spec, op, "label")?,
            after: opt(spec, op, "after")?.unwrap_or_default(),
            delegation: opt(spec, op, "delegation")?,
        });
    }
    Ok(specs)
}

fn parse_op(args: &Map<String, Value>) -> Result<Op, ArgError> {
    let name = args
        .get("op")
        .and_then(Value::as_str)
        .ok_or(ArgError::NoOp)?;
    let kind = ALL_OPS
        .into_iter()
        .find(|op| op_name(*op) == name)
        .ok_or_else(|| ArgError::UnknownOp {
            got: name.to_owned(),
        })?;
    Ok(match kind {
        OpKind::Init => Op::Init {
            goal: need(args, kind, "goal")?,
            todos: todo_specs(args, kind)?,
        },
        OpKind::Append => Op::Append {
            todos: todo_specs(args, kind)?,
        },
        OpKind::Drop => Op::Drop {
            label: need(args, kind, "label")?,
        },
        OpKind::Block => Op::Block {
            label: need(args, kind, "label")?,
            on: need(args, kind, "on")?,
            note: need(args, kind, "note")?,
        },
        OpKind::Unblock => Op::Unblock {
            label: need(args, kind, "label")?,
        },
        OpKind::Reorder => Op::Reorder {
            labels: need(args, kind, "labels")?,
        },
        OpKind::AddEdge => Op::AddEdge {
            todo: need(args, kind, "todo")?,
            after: need(args, kind, "after")?,
        },
        OpKind::Start => Op::Start {
            label: need(args, kind, "label")?,
        },
        OpKind::Done => Op::Done {
            label: need(args, kind, "label")?,
            output: opt(args, kind, "output")?,
        },
        OpKind::Fail => Op::Fail {
            label: need(args, kind, "label")?,
            cause: need(args, kind, "cause")?,
        },
        OpKind::Retry => Op::Retry {
            label: need(args, kind, "label")?,
            delegation: opt::<Delegation>(args, kind, "delegation")?.map(Box::new),
        },
        OpKind::Decompose => Op::Decompose {
            label: need(args, kind, "label")?,
            todos: todo_specs(args, kind)?,
        },
        OpKind::Supersede => Op::Supersede {
            reason: need(args, kind, "reason")?,
            todos: todo_specs(args, kind)?,
        },
        OpKind::View => Op::View {
            full: opt(args, kind, "full")?.unwrap_or(false),
        },
    })
}

fn request(actor: &Actor, args: &Map<String, Value>) -> Result<OpRequest, ArgError> {
    let op = parse_op(args)?;
    Ok(OpRequest {
        plan: opt(args, op.kind(), "plan")?,
        actor: actor.clone(),
        op,
    })
}

fn labels(of: &[TodoLabel]) -> String {
    of.iter()
        .map(TodoLabel::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

fn counts(todos: &[Todo]) -> String {
    let mut parts = Vec::new();
    for name in [
        TodoStateName::Pending,
        TodoStateName::Running,
        TodoStateName::Blocked,
        TodoStateName::Done,
        TodoStateName::Failed,
        TodoStateName::Abandoned,
    ] {
        let count = todos
            .iter()
            .filter(|todo| TodoStateName::of(&todo.state) == name)
            .count();
        if count > 0 {
            parts.push(format!("{count} {name}"));
        }
    }
    let unknown = todos
        .iter()
        .filter(|todo| matches!(TodoStateName::of(&todo.state), TodoStateName::Other(_)))
        .count();
    if unknown > 0 {
        parts.push(format!("{unknown} unknown"));
    }
    if parts.is_empty() {
        return "no todos".to_owned();
    }
    format!("{} todos: {}", todos.len(), parts.join(", "))
}

fn todo_line(todo: &Todo) -> String {
    let mut line = format!("- {} {}", TodoStateName::of(&todo.state), todo.label);
    if !todo.after.is_empty() {
        line.push_str(&format!(" after {}", labels(&todo.after)));
    }
    match &todo.state {
        TodoState::Running { by } => line.push_str(&format!(" by {by}")),
        TodoState::Blocked { on, note } => {
            let on = serde_json::to_string(on).unwrap_or_else(|_| "?".to_owned());
            line.push_str(&format!(" on {on}: {note}"));
        }
        TodoState::Done { output: Some(url) } => line.push_str(&format!(" -> {url}")),
        TodoState::Failed { cause, last } => {
            line.push_str(&format!(" ! {cause}"));
            if let Some(url) = last {
                line.push_str(&format!(" (last {url})"));
            }
        }
        TodoState::Pending
        | TodoState::Done { output: None }
        | TodoState::Abandoned
        | TodoState::Other(_) => {}
    }
    if !todo.retries.is_zero() {
        line.push_str(&format!(" retries {}", todo.retries.0));
    }
    if let Some(subplan) = &todo.subplan {
        line.push_str(&format!(" [subplan {subplan}]"));
    }
    line
}

fn header(plan: &Plan, out: &mut Vec<String>) {
    out.push(format!(
        "plan {} v{} touched {} {}",
        plan.id, plan.version.0, plan.touched.0, plan.state
    ));
    match &plan.tier {
        PlanTier::Root => {}
        PlanTier::Sub { parent } => out.push(format!("sub of {parent}")),
        PlanTier::Other { tier, parent } => {
            let parent = parent.as_ref().map(ToString::to_string).unwrap_or_default();
            out.push(format!("tier {tier} {parent}"));
        }
    }
    out.push(counts(&plan.todos));
}

fn body(outcome: &Outcome, full: bool, out: &mut Vec<String>) {
    let plan = &outcome.plan;
    let shown: Vec<&Todo> = if full {
        plan.todos.iter().collect()
    } else {
        plan.todos
            .iter()
            .filter(|todo| {
                outcome.ready.contains(&todo.label)
                    || matches!(
                        TodoStateName::of(&todo.state),
                        TodoStateName::Running | TodoStateName::Blocked
                    )
            })
            .take(WINDOW)
            .collect()
    };
    out.extend(shown.iter().map(|todo| todo_line(todo)));
    let hidden = plan.todos.len().saturating_sub(shown.len());
    if hidden > 0 {
        out.push(format!(
            "{hidden} more todos not shown; op view full for all"
        ));
    }
}

fn render(outcome: &Outcome, full: bool) -> String {
    let mut out = Vec::new();
    header(&outcome.plan, &mut out);
    if full {
        out.push(format!("goal: {}", outcome.plan.goal));
        out.push(format!(
            "spawns {} of {}",
            outcome.plan.spawns().get(),
            yi_types::plan::doc::SPAWN_CAP.get()
        ));
    }
    body(outcome, full, &mut out);
    if !outcome.ready.is_empty() {
        out.push(format!("ready: {}", labels(&outcome.ready)));
    }
    if !outcome.dispatched.is_empty() {
        out.push(format!("dispatched: {}", labels(&outcome.dispatched)));
    }
    if !outcome.held.is_empty() {
        out.push(format!(
            "held behind the dispatch width: {} ({})",
            outcome.held.len(),
            labels(&outcome.held)
        ));
    }
    for url in &outcome.spawned {
        out.push(format!("spawned {url}"));
    }
    for url in &outcome.reaped {
        out.push(format!("reaped {url}"));
    }
    if let Some(subplan) = &outcome.subplan {
        out.push(format!("subplan {subplan}"));
    }
    out.join("\n")
}

pub struct PlanTool {
    engine: Arc<PlanEngine>,
    actor: Actor,
}

impl PlanTool {
    pub fn new(engine: Arc<PlanEngine>, actor: Actor) -> Self {
        Self { engine, actor }
    }

    fn run(&self, args: &Map<String, Value>) -> Result<String, PlanToolError> {
        let request = request(&self.actor, args)?;
        let full = matches!(request.op, Op::View { full: true });
        let outcome = self.engine.apply(request)?;
        Ok(render(&outcome, full))
    }
}

impl Tool for PlanTool {
    fn name(&self) -> &str {
        "plan"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn schema(&self) -> Value {
        schema()
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Ledger
    }

    fn irreversible(&self, _input: &Map<String, Value>) -> bool {
        false
    }

    fn validate(&self, input: &Map<String, Value>) -> Result<(), String> {
        request(&self.actor, input)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn execute(&self, input: Map<String, Value>, _context: &ToolContext) -> ToolOutput {
        match self.run(&input) {
            Ok(text) => text_output(text),
            Err(error) => error_output(error.to_string()),
        }
    }
}

/// Invariant: the request-prefix gate prices this tool through these two
/// items rather than a live [`PlanTool`], so what is measured is what ships.
pub const DESCRIPTION: &str = "The plan ledger. One op per call against the open plan: declare the whole cut before working it, then step todos through it. A todo is a unit of decision, not of iteration -- thirty probes inside one kernel cell are one todo. Batch ops with real work; never call it alone.";

/// Invariant: a flat object, because one provider rebuilds the schema from `properties` and
/// `required` alone, so a root `oneOf` would vanish there. No live state: cached prefix.
pub fn schema() -> Value {
    json!({
            "type": "object",
            "properties": {
                "op": {
                    "type": "string",
                    "enum": ALL_OPS.iter().map(|op| op_name(*op)).collect::<Vec<_>>(),
                    "description": "init goal+todos; append/drop/reorder/add_edge edit the cut; start/done/fail step a todo; block/unblock park one; retry a failed one; decompose a running one into a sub-plan; supersede replaces the whole cut; view echoes it"
                },
                "plan": {"type": "string", "description": "Sub-plan id; omit for the root plan"},
                "goal": {"type": "string", "description": "init: the whole deliverable in one line"},
                "todos": {
                    "type": "array",
                    "items": {"type": "object"},
                    "description": "init/append/decompose/supersede: [{label, after?: [label], delegation?: {spec: {role?, model?, effort?, isolation?}, accept: {command|stated}, context?: [url], output?: {schema: url}}}]. A delegation hands the todo to a child and its accept is mandatory",
                    "minItems": 1
                },
                "label": {"type": "string", "description": "drop/block/unblock/start/done/fail/retry/decompose: the todo"},
                "labels": {"type": "array", "items": {"type": "string"}, "description": "reorder: every label of the plan, in the new priority order"},
                "todo": {"type": "string", "description": "add_edge: the todo that waits"},
                "after": {"type": "string", "description": "add_edge: the sibling it waits on"},
                "on": {"type": "object", "description": "block: {\"child\": agent} | {\"user\": null} | {\"external\": {\"probe\": command}}"},
                "note": {"type": "string", "description": "block: what would unblock it"},
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

    #[test]
    fn every_fixture_op_argument_set_parses() -> Fallible {
        let mut seen = 0usize;
        for entry in std::fs::read_dir(fixture_dir())? {
            let fixture: Value = serde_json::from_str(&std::fs::read_to_string(entry?.path())?)?;
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
                input.insert("op".to_owned(), Value::String(name.to_owned()));
                if let Some(plan) = step.get("plan") {
                    input.insert("plan".to_owned(), plan.clone());
                }
                let parsed = request(&Actor::Owner, &input);
                assert!(parsed.is_ok(), "{name}: {:?}", parsed.err());
                seen = seen.saturating_add(1);
            }
        }
        assert!(seen > 30, "only {seen} fixture ops exercised");
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

    fn todo(label: &str, state: TodoState) -> Result<Todo, yi_types::plan::doc::DocError> {
        Ok(Todo {
            label: TodoLabel::new(label)?,
            after: Vec::new(),
            state,
            delegation: None,
            subplan: None,
            retries: yi_types::plan::doc::RetryCount::default(),
            extra: Map::new(),
        })
    }

    #[test]
    fn a_windowed_view_counts_what_it_hid() -> Fallible {
        let by = yi_types::plan::doc::AgentId::new("kid")?;
        let mut plan = Plan::opening(
            PlanId::new("ship-it")?,
            GoalText::new("ship it")?,
            PlanTier::Root,
            vec![
                todo("cut", TodoState::Done { output: None })?,
                todo("build", TodoState::Running { by })?,
                todo("ship", TodoState::Pending)?,
            ],
        );
        plan.touched = yi_types::plan::doc::TouchCount(4);
        let outcome = Outcome {
            plan,
            ready: Vec::new(),
            dispatched: Vec::new(),
            held: Vec::new(),
            spawned: Vec::new(),
            reaped: Vec::new(),
            subplan: None,
        };
        let windowed = render(&outcome, false);
        assert!(
            windowed.contains("3 todos: 1 pending, 1 running, 1 done"),
            "{windowed}"
        );
        assert!(windowed.contains("- running build by kid"), "{windowed}");
        assert!(windowed.contains("2 more todos not shown"), "{windowed}");
        let full = render(&outcome, true);
        assert!(full.contains("- pending ship"), "{full}");
        assert!(!full.contains("not shown"), "{full}");
        Ok(())
    }
}
