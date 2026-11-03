//! The `spec` tool: `freeze` (requirements, artifacts, check; runs the baseline), `run`
//! (the harness runs the check now), `add` (a requirement found after the freeze), `view`.

use std::sync::Arc;

use serde_json::{Map, Value, json};
use yi_tools::{Tool, ToolContext, ToolKind, ToolOutput, error_output, text_output};
use yi_types::spec::{FreezeFact, FreezeRefusal, Kind, Outcome, Run, RunKind, Spec, Status};

use super::{FreezeError, SpecStore, ids_text};

pub const NAME: &str = "spec";

pub const DESCRIPTION: &str = "The requirement list and its check, owned by the harness. Before the first write: write a check script that prints one TAP line per requirement, `ok N - R7 …` or `not ok N - R7 …` (the R id first in the description, one id per requirement, reading the files the task names; `# metric name=value` for a metric), then `freeze` with the requirements in order (R1..Rn, kind f2p by default; p2p for what must stay green; metric for a number to improve), the artifact paths the task names, and the command. Freeze runs the check on the untouched tree and refuses an f2p requirement already green there, a p2p one red, an uncovered id, no f2p at all, or fewer requirements than the request enumerates. After every write, `run`: the harness runs the check and reports each id green, red with its first diagnostic line, or missing. `add` a requirement found later; `view` shows the spec and the last run. Example check:\n\n    # check.py\n    import json, os\n    def ok(n, r, cond, msg=\"\"): print((\"ok\" if cond else \"not ok\"), n, \"-\", r, msg)\n    p = \"/app/output.json\"\n    ok(1, \"R1\", os.path.exists(p), \"output written\")\n    d = json.load(open(p)) if os.path.exists(p) else None\n    ok(2, \"R2\", isinstance(d, dict) and \"legs\" in d, \"has legs\")\n";

const OPS: [&str; 4] = ["freeze", "run", "add", "view"];

pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "op": {"type": "string", "enum": OPS, "description": "freeze: the requirements, artifacts and check, then the baseline run; run: the harness runs the check now; add: one requirement after the freeze; view: the spec and the last run"},
            "requirements": {"type": "array", "items": {"type": "object", "properties": {"text": {"type": "string"}, "kind": {"type": "string", "enum": ["f2p", "p2p", "metric"]}}, "required": ["text"]}, "description": "freeze: in order; R1 is the first"},
            "artifacts": {"type": "array", "items": {"type": "string"}, "description": "freeze: the paths the task names as products, absolute or relative to the cwd"},
            "check": {"type": "string", "description": "freeze: the command, run with sh -c in the cwd, e.g. python3 check.py"},
            "text": {"type": "string", "description": "add: the requirement, quoted from the request"},
            "kind": {"type": "string", "enum": ["f2p", "p2p", "metric"], "description": "add: f2p by default"}
        },
        "required": ["op"]
    })
}

#[derive(Debug, thiserror::Error)]
pub enum ArgError {
    #[error("op is required; legal ops are {legal}", legal = OPS.join(", "))]
    NoOp,
    #[error("unknown op {0:?}; legal ops are {legal}", legal = OPS.join(", "))]
    UnknownOp(String),
    #[error("{0} requires {1:?}")]
    Missing(&'static str, &'static str),
    #[error("{0} argument {1:?} is malformed: {2}")]
    Malformed(&'static str, &'static str, String),
}

enum Op {
    Freeze {
        requirements: Vec<(String, Kind)>,
        artifacts: Vec<String>,
        check: String,
    },
    Run,
    Add {
        text: String,
        kind: Kind,
    },
    View,
}

fn string(args: &Map<String, Value>, field: &str) -> Option<String> {
    args.get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn kind(args: &Map<String, Value>, op: &'static str) -> Result<Kind, ArgError> {
    Kind::try_from(string(args, "kind").as_deref().unwrap_or(""))
        .map_err(|cause| ArgError::Malformed(op, "kind", cause.to_string()))
}

fn parse_op(args: &Map<String, Value>) -> Result<Op, ArgError> {
    let Some(op) = string(args, "op") else {
        return Err(ArgError::NoOp);
    };
    match op.as_str() {
        "freeze" => {
            let raw = args
                .get("requirements")
                .and_then(Value::as_array)
                .ok_or(ArgError::Missing("freeze", "requirements"))?;
            let mut requirements = Vec::new();
            for value in raw {
                match value {
                    Value::String(text) => requirements.push((text.clone(), Kind::F2p)),
                    Value::Object(item) => {
                        let text = string(item, "text").ok_or(ArgError::Malformed(
                            "freeze",
                            "requirements",
                            "every requirement carries `text`".to_owned(),
                        ))?;
                        requirements.push((text, kind(item, "freeze")?));
                    }
                    _ => {
                        return Err(ArgError::Malformed(
                            "freeze",
                            "requirements",
                            "every requirement is a string or {text, kind}".to_owned(),
                        ));
                    }
                }
            }
            if requirements.is_empty() {
                return Err(ArgError::Missing("freeze", "requirements"));
            }
            let artifacts: Vec<String> = args
                .get("artifacts")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            let check = string(args, "check").ok_or(ArgError::Missing("freeze", "check"))?;
            Ok(Op::Freeze {
                requirements,
                artifacts,
                check,
            })
        }
        "run" => Ok(Op::Run),
        "add" => Ok(Op::Add {
            text: string(args, "text").ok_or(ArgError::Missing("add", "text"))?,
            kind: kind(args, "add")?,
        }),
        "view" => Ok(Op::View),
        other => Err(ArgError::UnknownOp(other.to_owned())),
    }
}

pub struct SpecTool {
    store: Arc<SpecStore>,
}

impl SpecTool {
    pub fn new(store: Arc<SpecStore>) -> Self {
        Self { store }
    }

    fn run(&self, input: &Map<String, Value>) -> Result<String, String> {
        match parse_op(input).map_err(|error| error.to_string())? {
            Op::Freeze {
                requirements,
                artifacts,
                check,
            } => match self.store.freeze(requirements, &artifacts, &check) {
                Ok(frozen) => Ok(format!(
                    "frozen: {} requirements, {} artifacts, check `{}`, timeout {}s\n{}\nnext: write the artifacts, then `spec run` after every write",
                    frozen.draft.requirements.len(),
                    frozen.draft.artifacts.len(),
                    frozen.draft.check.as_str(),
                    frozen.timeout.as_secs(),
                    render(
                        &Spec::Frozen(Box::new(frozen.clone())),
                        Some(&frozen.baseline)
                    )
                )),
                Err(FreezeError::Refused(refusal)) => Err(refusal_text(&refusal)),
                Err(FreezeError::Failed(error)) => Err(error.to_string()),
            },
            Op::Run => {
                let run = self
                    .store
                    .run(RunKind::Run)
                    .map_err(|error| error.to_string())?;
                let spec = self.store.spec().ok_or("no spec")?;
                Ok(render(&spec, Some(&run)))
            }
            Op::Add { text, kind } => {
                let id = self
                    .store
                    .add(&text, kind)
                    .map_err(|error| error.to_string())?;
                Ok(format!(
                    "added {id} ({}): {text}\nnext: add its TAP line to the check, then `spec run`",
                    kind.as_str()
                ))
            }
            Op::View => {
                let Some(spec) = self.store.spec() else {
                    return Ok("no spec yet; `spec freeze` creates it".to_owned());
                };
                Ok(render(&spec, self.store.last_run().as_ref()))
            }
        }
    }
}

pub fn fact_text(fact: &FreezeFact) -> String {
    match fact {
        FreezeFact::GreenAtBaseline { ids } => format!(
            "{} already green on the untouched tree: it proves nothing about your work; strengthen the assertion or label it p2p",
            ids_text(ids)
        ),
        FreezeFact::RedAtBaseline { ids } => format!(
            "{} labelled p2p but red on the untouched tree: either it is f2p or the assertion is wrong",
            ids_text(ids)
        ),
        FreezeFact::Uncovered { ids } => format!("no TAP line names {}", ids_text(ids)),
        FreezeFact::NoF2p => "no f2p requirement: the spec proves no change".to_owned(),
        FreezeFact::MetricWithoutValue { ids } => format!(
            "{} is a metric but the check printed no `# metric name=value` line",
            ids_text(ids)
        ),
        FreezeFact::Fewer { held, enumerated } => {
            format!("the request lists {enumerated} items; the spec holds {held}")
        }
        FreezeFact::DidNotRun { cause } => {
            format!("the check did not run: {}", not_run_text(cause))
        }
        FreezeFact::Artifact { raw, why } => format!("artifact {raw:?} refused: {}", why_text(why)),
    }
}

fn not_run_text(cause: &yi_types::spec::NotRun) -> String {
    match cause {
        yi_types::spec::NotRun::Spawn { error } => format!("could not start ({error})"),
        yi_types::spec::NotRun::Timeout { after } => {
            format!("timed out after {}s", after.as_secs())
        }
        yi_types::spec::NotRun::NoTap => {
            "it printed no TAP line (`ok N - R1 …` / `not ok N - R1 …`)".to_owned()
        }
    }
}

fn why_text(why: &yi_types::spec::ArtifactRefused) -> String {
    match why {
        yi_types::spec::ArtifactRefused::Empty => "empty path".to_owned(),
        yi_types::spec::ArtifactRefused::Root => "the filesystem root".to_owned(),
        yi_types::spec::ArtifactRefused::CwdOrAncestor => {
            "the working directory or an ancestor of it".to_owned()
        }
        yi_types::spec::ArtifactRefused::Home => "a home directory".to_owned(),
        yi_types::spec::ArtifactRefused::Outside => {
            "outside the workspace; the refute moves it, so it must be a path under the cwd"
                .to_owned()
        }
        yi_types::spec::ArtifactRefused::TooLarge { bytes, max } => {
            format!("{bytes} bytes, over the {max} byte cap; declare the files, not the tree")
        }
    }
}

pub fn refusal_text(refusal: &FreezeRefusal) -> String {
    let mut lines = vec!["freeze refused:".to_owned()];
    lines.extend(
        refusal
            .facts
            .iter()
            .map(|fact| format!("- {}", fact_text(fact))),
    );
    lines.push("next: fix the check or the list, then `spec freeze` again".to_owned());
    lines.join("\n")
}

/// The per-id table: status, id, kind, text, and the diagnostic line for a red one.
pub fn render(spec: &Spec, run: Option<&Run>) -> String {
    let draft = spec.draft();
    let mut lines = Vec::new();
    let (mut green, mut total) = (0_usize, 0_usize);
    let mut red = Vec::new();
    for requirement in &draft.requirements {
        let status = run.map(|run| run.status(&requirement.id));
        let mark = match status {
            Some(Status::Green) => "green",
            Some(Status::Red) => "red",
            Some(Status::Missing) => "missing",
            None => "unrun",
        };
        if requirement.kind != Kind::Metric {
            total = total.saturating_add(1);
            if status == Some(Status::Green) {
                green = green.saturating_add(1);
            } else {
                red.push(requirement.id.clone());
            }
        }
        let diag = run
            .and_then(|run| run.diag(&requirement.id))
            .map(|diag| format!(" — {diag}"))
            .unwrap_or_default();
        lines.push(format!(
            "{mark:<7} {} [{}] {}{diag}",
            requirement.id,
            requirement.kind.as_str(),
            requirement.text
        ));
    }
    let mut head = match spec {
        Spec::Frozen(_) => format!("{green}/{total} green"),
        Spec::Draft(_) => "unfrozen".to_owned(),
    };
    if let Some(run) = run {
        match &run.outcome {
            Outcome::Ran {
                unbound, metrics, ..
            } => {
                if *unbound > 0 {
                    head.push_str(&format!(" · {unbound} TAP lines name no R id"));
                }
                for (name, value) in metrics {
                    head.push_str(&format!(" · {name} {}", value.get()));
                }
            }
            Outcome::NotRun(cause) => {
                head.push_str(&format!(
                    " · the check did not run: {}",
                    not_run_text(cause)
                ));
            }
        }
    }
    lines.insert(0, head);
    if let Some(run) = run
        && !run.tail.trim().is_empty()
        && !run.ran()
    {
        lines.push(format!("output tail:\n{}", run.tail.trim_end()));
    }
    if let Spec::Frozen(_) = spec {
        if red.is_empty() && total > 0 {
            lines.push(
                "next: every requirement is green; stop cleanly when the work is done".to_owned(),
            );
        } else if run.is_some() {
            lines.push(format!("next: fix {} and `spec run` again", ids_text(&red)));
        }
    }
    lines.join("\n")
}

impl Tool for SpecTool {
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
        ToolKind::Exec
    }

    fn kind_for(&self, input: &Map<String, Value>) -> ToolKind {
        match string(input, "op").as_deref() {
            Some("freeze" | "run") => ToolKind::Exec,
            _ => ToolKind::Ledger,
        }
    }

    fn irreversible(&self, input: &Map<String, Value>) -> bool {
        matches!(string(input, "op").as_deref(), Some("freeze" | "run"))
    }

    fn validate(&self, input: &Map<String, Value>) -> Result<(), String> {
        parse_op(input)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn execute(&self, input: Map<String, Value>, _context: &ToolContext) -> ToolOutput {
        match self.run(&input) {
            Ok(text) => text_output(text),
            Err(error) => error_output(error),
        }
    }
}
