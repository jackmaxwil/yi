use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Map, Value, json};
use yi_loop::{AgentTool, ExecutionMode};
use yi_types::event::ToolErrorKind;
use yi_types::kernel::NestedCall;

use crate::args::Args;
use crate::kernel::{HostRegistry, KernelService};

/// The session's tool table, read at each call: the registry is sealed before the tools land.
pub type ToolsFn = Arc<dyn Fn() -> Vec<Arc<dyn AgentTool>> + Send + Sync>;

/// What one session's cells called through `tool.call`, until its `ipython` result lists them.
#[derive(Default)]
pub struct Calls {
    rows: Mutex<Vec<NestedCall>>,
    minted: AtomicU64,
    /// Invariant: the loop orders writes only by awaiting one batch at a time, so a cell that
    /// gathers two edits would race them; this lock is the only thing that orders them.
    writes: tokio::sync::Mutex<()>,
}

impl Calls {
    fn push(&self, row: NestedCall) {
        self.rows
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(row);
    }

    fn drain(&self) -> Vec<NestedCall> {
        std::mem::take(&mut *self.rows.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// Why a tool the session holds is not callable from a cell, or `None` when it is.
fn excluded(name: &str, root: bool) -> Option<&'static str> {
    match name {
        "ipython" => {
            Some("a cell holds the kernel for its whole run, so a nested cell waits on itself")
        }
        "ask_user" if root => Some(
            "a root's question ends the turn and its answer is the user's next message, which a cell cannot wait for",
        ),
        _ => None,
    }
}

fn failure(kind: &str, message: String, suggestions: Vec<String>) -> Map<String, Value> {
    let reply = json!({"ok": false, "kind": kind, "message": message, "suggestions": suggestions});
    reply.as_object().cloned().unwrap_or_default()
}

/// The names that share a word with `name` first, then the rest: a typo still finds its tool.
fn suggestions(name: &str, names: &[String]) -> Vec<String> {
    let near = |other: &String| other.contains(name) || name.contains(other.as_str());
    let (mut close, far): (Vec<_>, Vec<_>) = names.iter().cloned().partition(near);
    close.extend(far);
    close
}

/// `tool.list` and `tool.call` (design §9.2): a cell calls the session's own adapters, so
/// rules, wall and broker judge each call on its own arguments, as they judge a model call.
pub fn register(registry: &mut HostRegistry, tools: ToolsFn, calls: Arc<Calls>, root: bool) {
    let listed = Arc::clone(&tools);
    registry.register("tool.list", move |_payload| {
        let (mut callable, mut left_out) = (Vec::new(), Vec::new());
        for tool in listed() {
            let definition = tool.definition();
            match excluded(&definition.name, root) {
                Some(why) => left_out.push(json!({"name": definition.name, "why": why})),
                None => callable.push(json!({
                    "name": definition.name,
                    "description": definition.description,
                    "parameters": definition.parameters,
                })),
            }
        }
        let reply = json!({"tools": callable, "excluded": left_out});
        Box::pin(async move { Ok(reply.as_object().cloned().unwrap_or_default()) })
    });
    registry.register("tool.call", move |payload| {
        let (table, calls) = (tools(), Arc::clone(&calls));
        Box::pin(async move { Ok(call(&table, &calls, root, &payload).await) })
    });
}

async fn call(
    table: &[Arc<dyn AgentTool>],
    calls: &Calls,
    root: bool,
    payload: &Map<String, Value>,
) -> Map<String, Value> {
    let Some(name) = payload.str_of("name") else {
        return failure(
            "invalid_input",
            "tool.call needs a \"name\"".to_owned(),
            Vec::new(),
        );
    };
    let args = match payload.get("args") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(args)) => args.clone(),
        Some(_) => {
            let message = format!("{name}'s arguments must be keyword arguments");
            return failure("invalid_input", message, Vec::new());
        }
    };
    if let Some(why) = excluded(name, root) {
        let message = format!("{name} is not callable from a cell: {why}");
        return failure("excluded_tool", message, Vec::new());
    }
    let Some(tool) = table.iter().find(|tool| tool.definition().name == name) else {
        let names: Vec<String> = (table.iter().map(|tool| tool.definition().name))
            .filter(|other| excluded(other, root).is_none())
            .collect();
        let message = format!("this session has no tool {name}");
        return failure("unknown_tool", message, suggestions(name, &names));
    };
    if let Err(reason) = tool.validate(&args) {
        return failure("invalid_input", reason, Vec::new());
    }
    let mode = tool.execution_mode(&args);
    let _ordered = match mode {
        ExecutionMode::Sequential => Some(calls.writes.lock().await),
        ExecutionMode::Parallel => None,
    };
    let id = format!("cell-call-{}", calls.minted.fetch_add(1, Ordering::Relaxed));
    let started = std::time::Instant::now();
    let signal = yi_loop::interrupt::InterruptSignal::default();
    let outcome = tool.execute(&id, args.clone(), &signal).await;
    let text = yi_types::message::join_text(&outcome.result.content, "\n");
    let denied = (outcome
        .result
        .details
        .get("errorKind")
        .and_then(Value::as_str))
        == Some(ToolErrorKind::Denied.as_str());
    let kind = match (outcome.is_error, denied) {
        (false, _) => None,
        (true, true) => Some("refused"),
        (true, false) => Some("tool_failure"),
    };
    let asked = name == "ask_user";
    calls.push(NestedCall {
        tool: name.to_owned(),
        ok: !outcome.is_error,
        error_kind: kind.map(str::to_owned),
        args: (mode == ExecutionMode::Sequential || asked).then_some(args),
        answer: asked.then(|| text.clone()),
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    });
    let reply = json!({
        "ok": !outcome.is_error,
        "kind": kind,
        "text": text,
        "details": outcome.result.details,
    });
    reply.as_object().cloned().unwrap_or_default()
}

/// The `ipython` tool, its result listing the calls its cell made when `calls` is given.
pub fn ipython_tool(
    service: Arc<KernelService>,
    calls: Option<Arc<Calls>>,
) -> Arc<dyn yi_tools::Tool> {
    let drain = calls.map(|calls| Arc::new(move || calls.drain()) as Arc<yi_tools::NestedCallsFn>);
    Arc::new(yi_tools::IpythonTool {
        bridge: service,
        calls: drain,
    })
}
