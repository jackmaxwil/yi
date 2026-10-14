use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::{Map, Value, json};
use yi_loop::{AgentTool, ExecutionMode};
use yi_types::event::ToolErrorKind;
use yi_types::kernel::{NESTED_READS_KEPT, NestedCall};

use crate::args::Args;
use crate::kernel::{HostRegistry, KernelService};

/// The session's tool table, read at each call: the registry is sealed before the tools land.
pub type ToolsFn = Arc<dyn Fn() -> Vec<Arc<dyn AgentTool>> + Send + Sync>;

#[derive(Default)]
struct Rows {
    request: Option<String>,
    cell: u64,
    kept: Vec<NestedCall>,
    read_keys: std::collections::BTreeSet<(String, Vec<String>)>,
    dropped: u64,
}

/// What one session's cells called through `tool.call`, until its `ipython` result lists them.
pub struct Calls {
    rows: Mutex<Rows>,
    minted: AtomicU64,
    /// Invariant: call ids repeat across attaches unless they carry when this one began.
    attached_ms: u64,
    /// Invariant: the loop runs a write alone and reads beside reads; a cell's gathered calls
    /// keep that order only through this lock, reads shared and everything else exclusive.
    order: tokio::sync::RwLock<()>,
}

impl Default for Calls {
    fn default() -> Self {
        Self {
            rows: Mutex::default(),
            minted: AtomicU64::new(0),
            attached_ms: yi_session::now_ms(),
            order: tokio::sync::RwLock::new(()),
        }
    }
}

impl Calls {
    fn rows(&self) -> MutexGuard<'_, Rows> {
        self.rows.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn admit(&self, token: Option<&str>) -> Option<u64> {
        let rows = self.rows();
        (token.is_some() && rows.request.as_deref() == token).then_some(rows.cell)
    }

    fn push(&self, mut row: NestedCall, read: bool, cell: u64) {
        let mut rows = self.rows();
        row.outlived = rows.request.is_none() || rows.cell != cell;
        if read {
            let key = (row.tool.clone(), row.paths.clone());
            if rows.read_keys.contains(&key) {
                return;
            }
            if rows.read_keys.len() >= NESTED_READS_KEPT {
                rows.dropped = rows.dropped.saturating_add(1);
                return;
            }
            rows.read_keys.insert(key);
        }
        rows.kept.push(row);
    }
}

impl yi_tools::CellCalls for Calls {
    fn open(&self, request: &str) {
        let mut rows = self.rows();
        rows.request = Some(request.to_owned());
        rows.cell = rows.cell.wrapping_add(1);
    }

    fn close(&self) -> (Vec<NestedCall>, u64) {
        let mut rows = self.rows();
        rows.request = None;
        rows.read_keys.clear();
        (
            std::mem::take(&mut rows.kept),
            std::mem::take(&mut rows.dropped),
        )
    }
}

/// Why a tool the session holds is not callable from a cell, or `None` when it is.
fn excluded(name: &str, root: bool) -> Option<&'static str> {
    match name {
        "ipython" => {
            Some("a cell holds the kernel for its whole run, so a nested cell waits on itself")
        }
        "answer" => {
            Some("an answer ends the model's turn, and a cell's result is the model's to state")
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

/// The names that contain `name` or that it contains first, then the rest.
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

/// The refusal a call earns before its tool is looked at, or `None`.
fn refused_before(name: &str, args: &Map<String, Value>, root: bool) -> Option<Map<String, Value>> {
    if let Some(why) = excluded(name, root) {
        let message = format!("{name} is not callable from a cell: {why}");
        return Some(failure("excluded_tool", message, Vec::new()));
    }
    // Incident: a cell's own `kernel://` read queues behind the cell it runs in, and times out.
    let kernel = args
        .str_of("path")
        .is_some_and(|path| path.starts_with("kernel://"));
    (name == "read" && kernel).then(|| {
        let message = "a cell reads kernel:// with `await rlm.fetch(url)`; its own variables are already in scope";
        failure("invalid_input", message.to_owned(), Vec::new())
    })
}

async fn call(
    table: &[Arc<dyn AgentTool>],
    calls: &Calls,
    root: bool,
    payload: &Map<String, Value>,
) -> Map<String, Value> {
    let Some(name) = payload.str_of("name") else {
        let message = "tool.call needs a \"name\"".to_owned();
        return failure("invalid_input", message, Vec::new());
    };
    let args = match payload.get("args") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(args)) => args.clone(),
        Some(_) => {
            let message = format!("{name}'s arguments must be keyword arguments");
            return failure("invalid_input", message, Vec::new());
        }
    };
    // Invariant: a call carries the request its cell ran as, which tasks inherit, so a console
    // cell or a task an earlier cell left never names the agent's open one.
    let Some(cell) = calls.admit(payload.str_of("cell")) else {
        let message = "not from the agent's running cell: a call from a task that outlived its cell, even while a later cell runs, one made after the cell was interrupted, one naming no cell (a thread's), or one from a cell typed into the console is refused; call it inside an ipython call";
        return failure("cancelled", message.to_owned(), Vec::new());
    };
    if let Some(refusal) = refused_before(name, &args, root) {
        return refusal;
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
    let read = tool.execution_mode(&args) == ExecutionMode::Parallel;
    let (_shared, _alone) = match read {
        true => (Some(calls.order.read().await), None),
        false => (None, Some(calls.order.write().await)),
    };
    let minted = calls.minted.fetch_add(1, Ordering::Relaxed);
    let id = format!("cell-{}-{minted}", calls.attached_ms);
    let started = std::time::Instant::now();
    let signal = yi_loop::interrupt::InterruptSignal::default();
    let (outcome, value) = tool.execute_program(&id, args.clone(), &signal).await;
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
    calls.push(
        NestedCall {
            tool: name.to_owned(),
            ok: !outcome.is_error,
            error_kind: kind.map(str::to_owned),
            paths: yi_context::details::call_paths(name, &args),
            command: (args.str_of("command").map(str::to_owned)).filter(|_| !read),
            answer: asked.then(|| text.clone()),
            duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            outlived: false,
        },
        read && !asked,
        cell,
    );
    let mut reply = Map::new();
    reply.insert("ok".to_owned(), (!outcome.is_error).into());
    reply.insert("kind".to_owned(), kind.into());
    reply.insert("text".to_owned(), text.into());
    reply.insert("details".to_owned(), outcome.result.details);
    reply.insert("value".to_owned(), value.into());
    reply
}

impl yi_tools::KernelBridge for KernelService {
    fn execute_cell(
        &self,
        code: &str,
        cancelled: &yi_tools::CancelFlag,
        recovery_dir: Option<&std::path::Path>,
    ) -> Result<yi_tools::KernelCellOutcome, String> {
        let handle = tokio::runtime::Handle::try_current()
            .map_err(|_| "ipython requires a tokio runtime".to_owned())?;
        let calls = self.cell_calls.get().cloned();
        handle.block_on(self.execute_async(code, cancelled, recovery_dir, calls))
    }
}

/// The `ipython` tool, its result listing the calls its cell made when `calls` is given.
pub fn ipython_tool(
    service: Arc<KernelService>,
    calls: Option<Arc<Calls>>,
) -> Arc<dyn yi_tools::Tool> {
    let calls = calls.map(|calls| calls as Arc<dyn yi_tools::CellCalls>);
    if let Some(calls) = &calls {
        service.cell_calls.get_or_init(|| Arc::clone(calls));
    }
    Arc::new(yi_tools::IpythonTool {
        bridge: service,
        calls,
    })
}
