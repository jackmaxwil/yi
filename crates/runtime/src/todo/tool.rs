use std::sync::Arc;

use serde_json::{Map, Value, json};
use yi_tools::{Tool, ToolContext, ToolKind, ToolOutput, error_output, text_output};
use yi_types::plan::doc::TodoLabel;
use yi_types::todo::{BlockedOn, PhaseName, TodoItem};

use super::{Op, Target, TodoError, TodoStore, text};

pub const NAME: &str = "todo";

pub const DESCRIPTION: &str = "Your task list; the user sees every change live. Create it before multi-step work (init with phases, or set with a checklist: `## Phase`, `- [ ] label`, `[>]` running, `[x]` done, `[-]` dropped, `[!]` blocked, two spaces nest one level). Every item is in one state and one op moves it: start (pending→running, one at a time), done with evidence (running→done, only after its check passed), block on user|external|child with a note saying what would unblock it, unblock, drop with a reason, append (optionally under a parent), rm, view. Labels are verbatim and unique; if you lost the text, view. A todo call rides with real work in the same message. Every result ends with next: lines you can copy.";

const OPS: [&str; 10] = [
    "set", "init", "append", "start", "done", "drop", "block", "unblock", "rm", "view",
];

pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "op": {"type": "string", "enum": OPS, "description": "set/init replace the list; append adds; start/done/drop/block/unblock move one item (done/drop/rm also take a phase, or nothing for all); rm removes; view echoes"},
            "list": {"type": "string", "description": "set: the checklist"},
            "phases": {"type": "array", "items": {"type": "object"}, "description": "init: [{name, items: [label]}]"},
            "items": {"type": "array", "items": {"type": "string"}, "description": "init (flat, one phase) or append: labels to add"},
            "phase": {"type": "string", "description": "append: the phase (created if missing); done/drop/rm: every item in it"},
            "under": {"type": "string", "description": "append: the parent label the items nest under"},
            "id": {"type": "string", "description": "start/done/drop/block/unblock/rm: the item's id, e.g. t3; or use label"},
            "label": {"type": "string", "description": "start/done/drop/block/unblock/rm: the item, verbatim"},
            "evidence": {"type": "string", "description": "done: the check that passed, quoted"},
            "reason": {"type": "string", "description": "drop: why the item no longer applies"},
            "on": {"type": "string", "enum": ["user", "external", "child"], "description": "block: who it waits on"},
            "note": {"type": "string", "description": "block: what would unblock it"},
            "touched": {"type": "integer", "description": "optional: the touched counter you last saw; a stale value is refused so a user edit is never overwritten"}
        }
    })
}

#[derive(Debug, thiserror::Error)]
pub enum ArgError {
    #[error("op is required; legal ops are {}", OPS.join(", "))]
    NoOp,
    #[error("unknown op {got:?}; legal ops are {}", OPS.join(", "))]
    UnknownOp { got: String },
    #[error("{op} requires {field:?}")]
    Missing {
        op: &'static str,
        field: &'static str,
    },
    #[error("{op} argument {field:?} is malformed: {cause}")]
    Malformed {
        op: &'static str,
        field: &'static str,
        cause: String,
    },
}

fn string(args: &Map<String, Value>, field: &'static str) -> Option<String> {
    args.get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn need_string(
    args: &Map<String, Value>,
    op: &'static str,
    field: &'static str,
) -> Result<String, ArgError> {
    string(args, field).ok_or(ArgError::Missing { op, field })
}

fn named(args: &Map<String, Value>) -> Option<String> {
    string(args, "id").or_else(|| string(args, "label"))
}

fn label(args: &Map<String, Value>, op: &'static str) -> Result<TodoLabel, ArgError> {
    let needle = named(args).ok_or(ArgError::Missing { op, field: "label" })?;
    TodoItem::from_text(&needle)
        .map(|item| item.label)
        .map_err(|cause| ArgError::Malformed {
            op,
            field: "label",
            cause: cause.to_string(),
        })
}

fn items(
    args: &Map<String, Value>,
    op: &'static str,
    field: &'static str,
) -> Result<Vec<TodoItem>, ArgError> {
    let raw = args
        .get(field)
        .and_then(Value::as_array)
        .ok_or(ArgError::Missing { op, field })?;
    raw.iter()
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| ArgError::Malformed {
                    op,
                    field,
                    cause: "every item is a string".to_owned(),
                })
                .and_then(|text| {
                    TodoItem::from_text(text).map_err(|cause| ArgError::Malformed {
                        op,
                        field,
                        cause: cause.to_string(),
                    })
                })
        })
        .collect()
}

fn phase(args: &Map<String, Value>, op: &'static str) -> Result<Option<PhaseName>, ArgError> {
    string(args, "phase")
        .map(|name| {
            PhaseName::new(name).map_err(|cause| ArgError::Malformed {
                op,
                field: "phase",
                cause: cause.to_string(),
            })
        })
        .transpose()
}

fn target(args: &Map<String, Value>, op: &'static str) -> Result<Target, ArgError> {
    if named(args).is_some() {
        return Ok(Target::Label(label(args, op)?));
    }
    Ok(phase(args, op)?.map_or(Target::All, Target::Phase))
}

/// The op the fields name on their own: `list` is set, `items` is append, a named item
/// with `evidence`/`reason`/`on` is done/drop/block; anything else stays a refusal.
pub fn infer_op(args: &Map<String, Value>) -> Option<&'static str> {
    let list = string(args, "list").is_some();
    let items = args
        .get("items")
        .and_then(Value::as_array)
        .is_some_and(|items| !items.is_empty());
    match (list, items, named(args).is_some()) {
        (true, false, false) => Some("set"),
        (false, true, false) => Some("append"),
        (false, false, true) if string(args, "evidence").is_some() => Some("done"),
        (false, false, true) if string(args, "reason").is_some() => Some("drop"),
        (false, false, true) if string(args, "on").is_some() => Some("block"),
        _ => None,
    }
}

pub fn parse_op(args: &Map<String, Value>) -> Result<Op, ArgError> {
    let op = string(args, "op")
        .or_else(|| infer_op(args).map(str::to_owned))
        .ok_or(ArgError::NoOp)?;
    Ok(match op.as_str() {
        "set" => Op::Set {
            list: need_string(args, "set", "list")?,
        },
        "init" => {
            let phases = match args.get("phases").and_then(Value::as_array) {
                Some(raw) => {
                    raw.iter()
                        .map(|value| {
                            let object = value.as_object().ok_or(ArgError::Malformed {
                                op: "init",
                                field: "phases",
                                cause: "every phase is {name, items}".to_owned(),
                            })?;
                            let name = PhaseName::new(need_string(object, "init", "name")?)
                                .map_err(|cause| ArgError::Malformed {
                                    op: "init",
                                    field: "name",
                                    cause: cause.to_string(),
                                })?;
                            Ok((name, items(object, "init", "items")?))
                        })
                        .collect::<Result<Vec<_>, ArgError>>()?
                }
                None => vec![(
                    phase(args, "init")?.unwrap_or(PhaseName::new(super::DEFAULT_PHASE).map_err(
                        |cause| ArgError::Malformed {
                            op: "init",
                            field: "phase",
                            cause: cause.to_string(),
                        },
                    )?),
                    items(args, "init", "items")?,
                )],
            };
            Op::Init { phases }
        }
        "append" => Op::Append {
            phase: phase(args, "append")?,
            under: string(args, "under")
                .map(|text| {
                    TodoLabel::new(text).map_err(|cause| ArgError::Malformed {
                        op: "append",
                        field: "under",
                        cause: cause.to_string(),
                    })
                })
                .transpose()?,
            items: items(args, "append", "items")?,
        },
        "start" => Op::Start {
            label: label(args, "start")?,
        },
        "done" => Op::Done {
            target: target(args, "done")?,
            evidence: string(args, "evidence"),
        },
        "drop" => Op::Drop {
            target: target(args, "drop")?,
            reason: need_string(args, "drop", "reason")?,
        },
        "block" => Op::Block {
            label: label(args, "block")?,
            on: match string(args, "on").as_deref() {
                None | Some("user") => BlockedOn::User,
                Some("external") => BlockedOn::External,
                Some("child") => BlockedOn::Child,
                Some(other) => BlockedOn::Other(other.to_owned()),
            },
            note: need_string(args, "block", "note")?,
        },
        "unblock" => Op::Unblock {
            label: label(args, "unblock")?,
        },
        "rm" => Op::Rm {
            target: target(args, "rm")?,
        },
        "view" => Op::View,
        other => {
            return Err(ArgError::UnknownOp {
                got: other.to_owned(),
            });
        }
    })
}

#[derive(Debug, thiserror::Error)]
pub enum TodoToolError {
    #[error(transparent)]
    Arg(#[from] ArgError),
    #[error(transparent)]
    Todo(#[from] TodoError),
}

pub struct TodoTool {
    store: Arc<TodoStore>,
}

impl TodoTool {
    pub fn new(store: Arc<TodoStore>) -> Self {
        Self { store }
    }

    fn run(&self, args: &Map<String, Value>) -> Result<String, TodoToolError> {
        let op = parse_op(args)?;
        let inferred = if string(args, "op").is_none() {
            format!("(op inferred: {})\n", op.name())
        } else {
            String::new()
        };
        let expected = args.get("touched").and_then(Value::as_u64);
        let applied = self.store.apply(op, expected)?;
        Ok(format!(
            "{inferred}{}\ntouched: {}",
            text::render(&applied.list),
            applied.touched
        ))
    }
}

impl Tool for TodoTool {
    fn name(&self) -> &str {
        NAME
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
        parse_op(input)
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
