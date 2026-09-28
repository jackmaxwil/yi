use std::borrow::Cow;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use yi_tools::{Tool, ToolContext, ToolKind, ToolOutput, error_output, text_output};
use yi_types::plan::doc::{BlockedOn, Todo, TodoLabel};
use yi_types::todo::PhaseName;

use super::{Op, Target, TodoError, TodoStore, mirror, text};

pub const NAME: &str = "todo";

pub const DESCRIPTION: &str = "Your task list; the user sees every change live. Create it before multi-step work (init with phases, or set with a checklist: `## Phase`, `- [ ] label`, `[>]` running, `[x]` done, `[-]` dropped, `[!]` blocked, two spaces nest one level). While a plan is open the plan is the list: its todos move by the plan tool, and these ops move only your own items. Every item is in one state and one op moves it: start (pending→running, one at a time), done with evidence (running→done, only after its check passed), block on user|external|child with a note on what unblocks it, unblock, drop with a reason, append (optionally under a parent), rm, view. Labels are verbatim and unique. A todo call rides with real work in the same message. Every result ends with next: lines you can copy.";

const OPS: [&str; 10] = [
    "set", "init", "append", "start", "done", "drop", "block", "unblock", "rm", "view",
];

pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "op": {"type": "string", "enum": OPS, "description": "done/drop/rm also take a phase, or nothing for all"},
            "list": {"type": "string", "description": "set: the checklist; trailing `user://<n>` tokens on a row cite the user messages it serves"},
            "phases": {"type": "array", "items": {"type": "object"}, "description": "init: [{name, items: [label]}]"},
            "items": {"type": "array", "items": {"type": "string"}, "description": "init (flat, one phase) or append: labels to add"},
            "phase": {"type": "string", "description": "append: the phase (created if missing); done/drop/rm: every item in it"},
            "under": {"type": "string", "description": "append: the parent label the items nest under"},
            "id": {"type": "string", "description": "start/done/drop/block/unblock/rm: the item's id, e.g. t3; or use label"},
            "label": {"type": "string", "description": "start/done/drop/block/unblock/rm: the item, verbatim"},
            "evidence": {"type": "string", "description": "done: the command in backticks and the output line that proves it"},
            "reason": {"type": "string", "description": "drop: why the item no longer applies"},
            "on": {"type": "string", "description": "block: who it waits on: user, external, child, or an address: `clock://at <ISO time>`, `exec://<command>?every=30s`, `file://<path>`, `channel://<name>`; the first message matching `filter` unblocks it"},
            "filter": {"type": "string", "description": "block: `key=value&…` matched exactly, or a substring; `ok=true` waits for an exec to pass"},
            "note": {"type": "string", "description": "block: what would unblock it"},
            "options": {"type": "array", "items": {"type": "object"}, "description": "block on user: 3 to 5 answers [{id, label, preview?}] the user picks one of by number, id or label; a preview is light, at most 2048 bytes"},
            "touched": {"type": "integer", "description": "optional: the touched counter you last saw; a stale value is refused so a user edit is never overwritten"}
        }
    })
}

#[derive(Debug, thiserror::Error)]
pub enum ArgError {
    #[error(
        "op is required; legal ops are {}; a full call looks like {}",
        OPS.join(", "),
        example("done")
    )]
    NoOp,
    #[error(
        "unknown op {got:?}; legal ops are {}; a full call looks like {}",
        OPS.join(", "),
        example("done")
    )]
    UnknownOp { got: String },
    #[error("{op} requires {field:?}; a full {op} call looks like {}", example(op))]
    Missing {
        op: &'static str,
        field: &'static str,
    },
    #[error(
        "{op} argument {field:?} is malformed: {cause}; a full {op} call looks like {}",
        example(op)
    )]
    Malformed {
        op: &'static str,
        field: &'static str,
        cause: String,
    },
}

fn example(op: &str) -> &'static str {
    match op {
        "set" => r###"{"op": "set", "list": "## Phase\n- [ ] first task\n- [ ] second task"}"###,
        "init" => r###"{"op": "init", "phases": [{"name": "Phase", "items": ["first task"]}]}"###,
        "append" => r###"{"op": "append", "items": ["another task"]}"###,
        "start" => r###"{"op": "start", "id": "t1"}"###,
        "drop" => r###"{"op": "drop", "id": "t1", "reason": "out of scope"}"###,
        "block" => r###"{"op": "block", "id": "t1", "on": "user", "note": "which file?"}"###,
        "unblock" => r###"{"op": "unblock", "id": "t1"}"###,
        "rm" => r###"{"op": "rm", "id": "t1"}"###,
        // Done, and the op a call lost: all 17 `op is required` in the v4 sweep were done calls.
        _ => r###"{"op": "done", "id": "t1", "evidence": "`make check` all targets ok"}"###,
    }
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
    Todo::from_text(&needle)
        .map(|item| item.label)
        .map_err(|cause| ArgError::Malformed {
            op,
            field: "label",
            cause: cause.to_string(),
        })
}

/// `items`, which the v4 sweep's models also sent as `todos`.
fn item_list(args: &Map<String, Value>) -> Option<&Vec<Value>> {
    args.get("items")
        .or_else(|| args.get("todos"))
        .and_then(Value::as_array)
}

fn items(args: &Map<String, Value>, op: &'static str) -> Result<Vec<Todo>, ArgError> {
    let field = "items";
    let raw = item_list(args).ok_or(ArgError::Missing { op, field })?;
    raw.iter()
        .map(|value| {
            value
                .as_str()
                .or_else(|| value.get("label").and_then(Value::as_str))
                .ok_or_else(|| ArgError::Malformed {
                    op,
                    field,
                    cause: "every item is a string".to_owned(),
                })
                .and_then(|text| {
                    Todo::from_text(text).map_err(|cause| ArgError::Malformed {
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

/// Incident: 17 v4 `done` calls leaked GLM's call markup, `{"done<arg_key>evidence": …}` or
/// `{"done": "evidence</arg_key><arg_value>…"}`; the field reads back and `infer_op` finds the op.
pub fn unleak(args: &Map<String, Value>) -> Cow<'_, Map<String, Value>> {
    let mut fixed = Cow::Borrowed(args);
    for (key, value) in args {
        let leaked = match key.rsplit_once("<arg_key>") {
            Some((_, field)) => Some((field, value.clone())),
            None => value
                .as_str()
                .and_then(|text| text.split_once("</arg_key><arg_value>"))
                .filter(|(field, _)| field.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
                .map(|(field, rest)| (field, Value::String(rest.to_owned()))),
        };
        if let Some((field, value)) = leaked
            && !field.is_empty()
            && !fixed.contains_key(field)
        {
            let map = fixed.to_mut();
            map.remove(key);
            map.insert(field.to_owned(), value);
        }
    }
    fixed
}

/// Incident: four F0e `done` calls wrote `{"done": "t3", …}`; for an op whose one argument is
/// the item that has a single reading, so it becomes `op` plus `id` (#474).
pub fn unkey_op(args: &Map<String, Value>) -> Cow<'_, Map<String, Value>> {
    const ID_OPS: [&str; 6] = ["start", "done", "drop", "block", "unblock", "rm"];
    if args.contains_key("op") || named(args).is_some() {
        return Cow::Borrowed(args);
    }
    let found = args.iter().find_map(|(key, value)| {
        let op = ID_OPS.iter().find(|op| **op == key.as_str())?;
        Some((*op, value.as_str()?.to_owned()))
    });
    let Some((op, id)) = found else {
        return Cow::Borrowed(args);
    };
    let mut fixed = args.clone();
    fixed.remove(op);
    fixed.insert("op".to_owned(), Value::String(op.to_owned()));
    fixed.insert("id".to_owned(), Value::String(id));
    Cow::Owned(fixed)
}

pub fn parse_op(args: &Map<String, Value>) -> Result<Op, ArgError> {
    let unleaked = unleak(args);
    let args = &unkey_op(&unleaked);
    let op = string(args, "op")
        .or_else(|| infer_op(args).map(str::to_owned))
        .ok_or(ArgError::NoOp)?;
    let list = string(args, "list").is_some();
    let op = match op.as_str() {
        // An init carrying set's checklist, or a set carrying init's items, meant the other.
        "init" if list && item_list(args).is_none() && !args.contains_key("phases") => "set",
        "set" if !list && item_list(args).is_some() => "init",
        given => given,
    };
    Ok(match op {
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
                            Ok((name, items(object, "init")?))
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
                    items(args, "init")?,
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
            items: items(args, "append")?,
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
            on: match BlockedOn::from_word(string(args, "on").as_deref()) {
                BlockedOn::Channel { address, .. } => {
                    crate::schedule::channel::check_address(&address).map_err(|cause| {
                        ArgError::Malformed {
                            op: "block",
                            field: "on",
                            cause,
                        }
                    })?;
                    BlockedOn::Channel {
                        address,
                        filter: string(args, "filter"),
                    }
                }
                on => on,
            },
            note: need_string(args, "block", "note")?,
            ask: crate::plan::ask::parse(
                args.get("options"),
                string(args, "on").is_none_or(|on| on == "user"),
            )
            .map_err(|cause| ArgError::Malformed {
                op: "block",
                field: "options",
                cause,
            })?
            .map(Box::new),
        },
        "unblock" => Op::Unblock {
            label: label(args, "unblock")?,
            answer: None,
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
    #[error("{0}")]
    Plan(String),
}

pub struct TodoTool {
    store: Arc<TodoStore>,
}

impl TodoTool {
    pub fn new(store: Arc<TodoStore>) -> Self {
        Self { store }
    }

    fn run(&self, args: &Map<String, Value>) -> Result<String, TodoToolError> {
        let op = self.store.aim(parse_op(args)?)?;
        let inferred = if string(args, "op").as_deref() == Some(op.name()) {
            String::new()
        } else {
            format!("(op inferred: {})\n", op.name())
        };
        if let Some(carried) = self.store.carry(&op) {
            let text = carried.map_err(TodoToolError::Plan)?;
            return Ok(format!(
                "{inferred}(the plan tool's {} on the plan's list)\n{text}",
                op.name()
            ));
        }
        let expected = args.get("touched").and_then(Value::as_u64);
        let whole = matches!(op, Op::Set { .. } | Op::Init { .. });
        let applied = self.store.apply(op, expected)?;
        let body = if applied.changed && !whole {
            text::render_change(&applied.before, &applied.list)
        } else {
            text::render(&applied.list)
        };
        let gone: Vec<&Todo> = applied
            .before
            .items()
            .filter(|item| !applied.list.items().any(|kept| kept.label == item.label))
            .filter(|item| item.extra.contains_key(mirror::PLAN_KEY))
            .collect();
        let replaced = match gone
            .first()
            .and_then(|item| item.extra.get(mirror::PLAN_KEY))
        {
            Some(plan) => format!(
                "(replaced plan {}'s {} rows; that plan is no longer open)\n",
                plan.as_str().unwrap_or_default(),
                gone.len()
            ),
            None => String::new(),
        };
        Ok(format!(
            "{inferred}{replaced}{body}\ntouched: {}",
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
