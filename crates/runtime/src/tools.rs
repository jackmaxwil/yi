use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value};
use yi_loop::interrupt::InterruptSignal;
use yi_loop::tool::{ToolFuture, error_tool_result};
use yi_loop::{AgentTool, ToolOutcome};
use yi_tools::{CancelFlag, Tool, ToolContext};
use yi_types::model::ToolDef;

use crate::permission::{Containment, PermissionBroker};

pub struct ToolAdapter {
    tool: Arc<dyn Tool>,
    cwd: PathBuf,
    cancelled: CancelFlag,
    permission: Option<Arc<PermissionBroker>>,
    spill_root: Option<PathBuf>,
    spill_key: Option<Arc<dyn Fn() -> Option<String> + Send + Sync>>,
    transcript: Option<crate::goal::StoreHandle>,
    auto_background: Option<std::time::Duration>,
    rules: Option<Arc<crate::rules::RuleEngine>>,
    wall: crate::wall::Wall,
    ext: Option<crate::session::ExtHook>,
    check: Option<Arc<crate::plan::covers::WriteCheck>>,
    job_owner: Option<yi_tools::jobs::JobOwner>,
    rejections: std::sync::Mutex<std::collections::BTreeMap<String, u32>>,
}

fn files_matched(name: &str, result: &yi_types::event::ToolResult) -> u32 {
    match name {
        "grep" => result
            .details
            .get("files")
            .and_then(Value::as_u64)
            .map_or(0, |files| u32::try_from(files).unwrap_or(u32::MAX)),
        _ => 0,
    }
}

/// Invariant: an empty value in a property the schema does not require is a key not sent; models
/// fill optional fields with `""` or `[]` they do not mean, and a required one is never touched.
fn unsent_empties<'a>(
    schema: &Value,
    args: Cow<'a, Map<String, Value>>,
) -> Cow<'a, Map<String, Value>> {
    let required = schema.get("required").and_then(Value::as_array);
    let required = |key: &str| required.is_some_and(|keys| keys.iter().any(|name| name == key));
    let kept = |key: &String, value: &Value| {
        let empty = match value {
            Value::String(text) => text.is_empty(),
            Value::Array(items) => items.is_empty(),
            Value::Object(fields) => fields.is_empty(),
            Value::Null | Value::Bool(_) | Value::Number(_) => false,
        };
        !empty || required(key)
    };
    if args.iter().all(|(key, value)| kept(key, value)) {
        return args;
    }
    let mut args = args.into_owned();
    args.retain(|key, value| kept(key, value));
    Cow::Owned(args)
}

/// Invariant: a verdict is the tool's answer, not its failure: the call ran and a rule said no,
/// so it reaches the model unflagged and keeps its `details.errorKind`.
fn verdict_is_a_result(output: &mut yi_tools::ToolOutput) {
    let verdict = Some(yi_types::event::ToolErrorKind::Verdict.as_str());
    if output
        .result
        .details
        .get("errorKind")
        .and_then(Value::as_str)
        == verdict
    {
        output.is_error = false;
    }
}

fn result_text(result: &yi_types::event::ToolResult) -> String {
    yi_types::message::join_text(&result.content, "\n")
}

/// The predicates that hold after this call; the needles are the states the old
/// producers branched on, and one ipython needle wins, in the order they were tried.
fn facts_of(tool: &str, output: &yi_tools::ToolOutput) -> Vec<String> {
    let text = result_text(&output.result);
    let kind = output
        .result
        .details
        .get("errorKind")
        .and_then(Value::as_str);
    let kind = kind.unwrap_or(yi_types::event::ToolErrorKind::ToolError.as_str());
    let mut holds = vec![match output.is_error {
        true => format!("{}({kind})", yi_types::graph::RESULT_ERROR),
        false => yi_types::graph::RESULT_OK.to_owned(),
    }];
    // ponytail: CPython 3.11-3.13 wording; add a second needle if a venv rewords it.
    let needle = if text.contains("<coroutine object ") {
        Some(yi_types::graph::COROUTINE_UNAWAITED)
    } else if text.contains("can't be used in 'await' expression") {
        Some(yi_types::graph::METHOD_AWAITED)
    } else if text.contains("AttributeError")
        && text.contains("RLMSubagent")
        && text.contains("'name'")
    {
        Some(yi_types::graph::LISTING_NAME_MISSED)
    } else {
        None
    };
    if let Some(needle) = needle.filter(|_| tool == "ipython") {
        holds.push(needle.to_owned());
    }
    holds
}

/// Invariant: a command a call arms runs later on the host with no sandbox and no container,
/// so it passes every gate bash does, and a containment the sandbox would enforce is refused.
async fn gate_armed(
    command: &str,
    (rules, wall, broker): Gates<'_>,
    walled: (&[PathBuf], &[PathBuf]),
    context: &ToolContext,
) -> Option<String> {
    let refused = host_wall(command, rules.as_deref(), wall, walled, &context.cwd);
    if refused.is_some() {
        return refused;
    }
    let (broker, id, owned) = (broker.cloned(), context.call_id.clone(), command.to_owned());
    let container = context.container.is_some();
    tokio::task::spawn_blocking(move || refuse_armed(&owned, container, broker.as_deref(), &id))
        .await
        .unwrap_or_else(|join_error| Some(format!("permission check failed: {join_error}")))
}

/// The gate of a heartbeat's `exec://` source: it runs on the host, so it meets what an armed
/// todo command does. The spares are read per call: a child's store attaches after its wiring.
pub(crate) fn heartbeat_gate(
    wiring: &crate::wiring::RuntimeWiring,
    rules: Arc<dyn Fn() -> Option<Arc<crate::rules::RuleEngine>> + Send + Sync>,
    own: Option<crate::wiring::OwnPathsFn>,
) -> Arc<crate::schedule::GateFn> {
    let (broker, contained) = (wiring.broker.clone(), wiring.wall.container.is_some());
    let (wall, cwd) = (wiring.wall.clone(), wiring.cwd.clone());
    let roots = walled_roots(&wall, broker.as_deref());
    Arc::new(move |command: &str| {
        let spared: Vec<PathBuf> = own.iter().flat_map(|own| own(None)).collect();
        let rules = rules();
        host_wall(command, rules.as_deref(), &wall, (&roots, &spared), &cwd)
            .or_else(|| refuse_armed(command, contained, broker.as_deref(), ""))
    })
}

/// Invariant: a command run on the host meets the user's gate rules, then the wall by its text
/// alone: the wall's own lists, and its walled roots save a spare (#1001).
pub(crate) fn host_wall(
    command: &str,
    rules: Option<&crate::rules::RuleEngine>,
    wall: &crate::wall::Wall,
    (roots, spared): (&[PathBuf], &[PathBuf]),
    cwd: &std::path::Path,
) -> Option<String> {
    let args = bash(command);
    let json = serde_json::to_string(&args).unwrap_or_default();
    let walled = (rules.and_then(|rules| rules.check_tool("bash", &json)))
        .or_else(|| wall.check("bash", yi_tools::ToolKind::Exec, &args, cwd));
    walled.or_else(|| walled_root_refusal(command, roots, spared, cwd))
}

/// Incident: each relative `cd` resolved against every dir so far, doubling them, so a chain of
/// k cds cost 2^k resolves per word; past this the check refuses rather than skip a target.
const CD_DIRS_MAX: usize = 64;

/// Refuses a command naming a root or a dir above one, but a spare, in most spellings (`~`, a glob,
/// `..` after a `cd`, glued to a flag). Text only: `$(…)`, a variable or a built path pass it.
fn walled_root_refusal(
    command: &str,
    roots: &[PathBuf],
    spared: &[PathBuf],
    cwd: &std::path::Path,
) -> Option<String> {
    let text = command.replace(['"', '\'', '\\'], "");
    let text = text.replace("${HOME}", "~").replace("$HOME", "~");
    let words = text.split(|c: char| c.is_whitespace() || "=;|&()<>`:@".contains(c));
    // Every `cd` target joins the dirs a word is resolved from, scoped or not: fail closed.
    let (mut dirs, mut after_cd) = (vec![cwd.to_path_buf()], false);
    for word in words.filter(|word| !word.is_empty() && !roots.is_empty()) {
        let glued = word.find('/').filter(|at| *at > 0).map(|at| &word[at..]);
        for spelled in std::iter::once(word).chain(glued) {
            let head: String = (spelled.split_inclusive('/'))
                .take_while(|part| !part.contains(['*', '?', '[']))
                .collect();
            let paths: Vec<PathBuf> = (dirs.iter())
                .map(|dir| yi_permission::resolve_target(&head, dir))
                .collect();
            let hit = roots.iter().find(|root| {
                paths.iter().any(|path| {
                    yi_tools::walled(std::slice::from_ref(path), root)
                        || yi_tools::walled(std::slice::from_ref(*root), path)
                            && !yi_tools::walled(spared, path)
                })
            });
            if let Some(root) = hit {
                return Some(crate::gate::outside_sandbox_refusal(root));
            }
            for path in paths.into_iter().filter(|_| after_cd) {
                if !dirs.contains(&path) {
                    dirs.push(path);
                }
            }
            if dirs.len() > CD_DIRS_MAX {
                let why = format!(
                    "its cd targets give {} directories to read its paths from, past the check's cap of {CD_DIRS_MAX}; run the cds as separate calls",
                    dirs.len()
                );
                return Some(crate::gate::walled_host_refusal(&why));
            }
        }
        after_cd = matches!(word, "cd" | "pushd");
    }
    None
}

fn bash(command: &str) -> Map<String, Value> {
    [("command".to_owned(), Value::from(command))]
        .into_iter()
        .collect()
}

/// Blocking, since an ask waits for its answer; the kernel's `rlm_heartbeat.create` shares it.
pub fn refuse_armed(
    command: &str,
    in_container: bool,
    broker: Option<&PermissionBroker>,
    call_id: &str,
) -> Option<String> {
    if in_container {
        return Some(format!(
            "an exec:// source runs `{command}` on the host, and this session's commands run in a container"
        ));
    }
    if broker.is_some_and(PermissionBroker::confines_walled) {
        return Some(crate::gate::walled_host_refusal(
            "an exec:// source runs on the host",
        ));
    }
    let outcome = broker?.decide_call(
        "bash",
        yi_tools::ToolKind::Exec,
        true,
        call_id,
        &bash(command),
        Some(command),
    );
    match outcome.containment {
        _ if !outcome.allowed => Some(format!("Permission denied: {}", outcome.reason)),
        Containment::Contained {
            gate_allowed: false,
            ..
        } => Some(format!(
            "an exec:// source runs `{command}` outside the sandbox, which allows it only contained: run it with bash in a turn, or allow it by a permission rule"
        )),
        Containment::Contained { .. } | Containment::Uncontained => None,
    }
}

type Gates<'a> = (
    &'a Option<Arc<crate::rules::RuleEngine>>,
    &'a crate::wall::Wall,
    Option<&'a Arc<PermissionBroker>>,
);

/// The §7.3 tee target: the home root, never the user's working tree; one dir per session.
pub(crate) fn default_spill_root() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".yi").join(yi_tools::SPILLS))
}

/// A session's own spill dir under `root`, when its key is a valid session id.
pub(crate) fn own_spill_dir(
    root: Option<&std::path::Path>,
    key: Option<String>,
) -> Option<PathBuf> {
    let key = key.filter(|key| yi_session::validate_session_id(key).is_ok())?;
    Some(root?.join(key))
}

/// Every session store: `~/.yi/sessions` and the `--session-dir` in use, which the broker holds
/// as host-owned (D335).
pub(crate) fn session_stores(broker: Option<&PermissionBroker>) -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".yi/sessions"));
    let held = broker
        .map(PermissionBroker::session_stores)
        .unwrap_or_default();
    let mut stores: Vec<PathBuf> = home.into_iter().chain(held.iter().cloned()).collect();
    stores.sort();
    stores.dedup();
    stores
}

/// The spill roots and every session store: what a walled session's tools and kernel read only
/// where a later rule spares it.
pub(crate) fn spill_roots_and_stores(
    spill_root: Option<&std::path::Path>,
    broker: Option<&PermissionBroker>,
) -> Vec<PathBuf> {
    let flat =
        (spill_root.and_then(std::path::Path::parent)).map(|yi| yi.join(yi_tools::FLAT_SPILLS));
    (spill_root.map(std::path::Path::to_path_buf).into_iter())
        .chain(flat)
        .chain(session_stores(broker))
        .collect()
}

/// Invariant: a walled session reads only its own spills and transcript: the spill roots and
/// session stores are walled, its own spill dir and transcript spared (D340, D345).
pub(crate) fn walled_roots(
    wall: &crate::wall::Wall,
    broker: Option<&PermissionBroker>,
) -> Vec<PathBuf> {
    if wall.is_empty() {
        return Vec::new();
    }
    spill_roots_and_stores(default_spill_root().as_deref(), broker)
}

/// Invariant: a spare, a kernel's or a tool's, never reopens a path the wall's own `deny_read`
/// covers (#1000, #1001).
pub(crate) fn unwalled(wall: &crate::wall::Wall, dir: &std::path::Path) -> bool {
    !yi_tools::walled(&wall.deny_read, dir)
}

impl ToolAdapter {
    pub fn new(
        tool: Arc<dyn Tool>,
        cwd: PathBuf,
        cancelled: CancelFlag,
        permission: Option<Arc<PermissionBroker>>,
    ) -> Self {
        Self {
            tool,
            cwd,
            cancelled,
            permission,
            spill_root: default_spill_root(),
            spill_key: None,
            transcript: None,
            auto_background: None,
            rules: None,
            wall: crate::wall::Wall::default(),
            ext: None,
            check: None,
            job_owner: None,
            rejections: std::sync::Mutex::new(std::collections::BTreeMap::new()),
        }
    }

    pub fn with_job_owner(mut self, owner: yi_tools::jobs::JobOwner) -> Self {
        self.job_owner = Some(owner);
        self
    }

    /// The session's key names its spill dir, read per call: the store may attach after wiring.
    pub fn with_spill_key(mut self, key: Arc<dyn Fn() -> Option<String> + Send + Sync>) -> Self {
        self.spill_key = Some(key);
        self
    }

    /// The session's own spill dir, when its key is a valid session id.
    fn session_spills(&self) -> Option<PathBuf> {
        own_spill_dir(
            self.spill_root.as_deref(),
            self.spill_key.as_ref().and_then(|key| key()),
        )
    }

    /// The store whose file is the session's own transcript, read per call like its spill key.
    pub fn with_transcript(mut self, store: crate::goal::StoreHandle) -> Self {
        self.transcript = Some(store);
        self
    }

    pub fn with_extensions(mut self, ext: Option<crate::session::ExtHook>) -> Self {
        self.ext = ext;
        self
    }

    /// D13: default off — self-detaching is a surprise unless asked for.
    pub fn with_auto_background(mut self, limit: Option<std::time::Duration>) -> Self {
        self.auto_background = limit;
        self
    }

    pub fn with_wall(mut self, wall: crate::wall::Wall) -> Self {
        self.wall = wall;
        self
    }

    pub fn with_check(mut self, check: Option<Arc<crate::plan::covers::WriteCheck>>) -> Self {
        self.check = check;
        self
    }

    pub fn with_rules(mut self, rules: Option<Arc<crate::rules::RuleEngine>>) -> Self {
        self.rules = rules;
        self
    }
}

impl AgentTool for ToolAdapter {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.tool.name().to_owned(),
            description: self.tool.description().to_owned(),
            parameters: self.tool.schema(),
            freeform: self.tool.freeform(),
        }
    }

    /// Only read-kind calls overlap; a mutation keeps the transcript's order.
    fn execution_mode(&self, args: &Map<String, Value>) -> yi_loop::config::ExecutionMode {
        match self.tool.kind_for(args) {
            yi_tools::ToolKind::Read => yi_loop::config::ExecutionMode::Parallel,
            yi_tools::ToolKind::Write | yi_tools::ToolKind::Exec | yi_tools::ToolKind::Ledger => {
                yi_loop::config::ExecutionMode::Sequential
            }
        }
    }

    fn validate(&self, args: &Map<String, Value>) -> Result<(), String> {
        let args = &*unsent_empties(&self.tool.schema(), Cow::Borrowed(args));
        let Err(reason) = self.tool.validate(args) else {
            return Ok(());
        };
        let repeated = self
            .rejections
            .lock()
            .map(|mut seen| {
                let count = seen.entry(reason.clone()).or_insert(0);
                *count = count.saturating_add(1);
                *count > 1
            })
            .unwrap_or(false);
        if !repeated {
            return Err(reason);
        }
        Err(format!(
            "{reason}\n{}",
            crate::affordance::call_template(self.tool.name(), &self.tool.schema(), args)
        ))
    }

    fn execute<'a>(
        &'a self,
        tool_call_id: &'a str,
        args: Map<String, Value>,
        _signal: &'a InterruptSignal,
    ) -> ToolFuture<'a> {
        let tool = Arc::clone(&self.tool);
        let args = unsent_empties(&tool.schema(), Cow::Owned(args)).into_owned();
        let walled_roots = walled_roots(&self.wall, self.permission.as_deref());
        let spills = self.session_spills();
        let store = (self.transcript.as_ref()).and_then(|store| store());
        let transcript =
            store.and_then(|store| yi_session::lock_session(&store).file_path().cloned());
        let spared: Vec<PathBuf> = (spills.iter().chain(&transcript))
            .filter(|dir| !walled_roots.is_empty() && unwalled(&self.wall, dir))
            .cloned()
            .collect();
        let mut context = ToolContext {
            cwd: self.cwd.clone(),
            cancelled: Arc::clone(&self.cancelled),
            recovery_dir: spills,
            transcript,
            auto_background: self.auto_background,
            sandbox: None,
            deny_read: [self.wall.deny_read.as_slice(), &walled_roots].concat(),
            deny_write: self.wall.deny_write.clone(),
            container: self.wall.container.clone(),
            call_id: tool_call_id.to_owned(),
            job_owner: self.job_owner,
        };
        let permission = self.permission.clone();
        let rules = self.rules.clone();
        let wall = self.wall.clone();
        let ext = self.ext.clone();
        let check = self.check.clone();
        let call_id = tool_call_id.to_owned();
        Box::pin(async move {
            if let Some(ext) = &ext {
                ext(crate::ext::Event::ToolCall {
                    name: tool.name().to_owned(),
                    target: args
                        .get("path")
                        .and_then(Value::as_str)
                        .map(|path| yi_permission::resolve_target(path, &context.cwd)),
                });
            }
            // User rules gate before permission: a matching gate rule denies with its body.
            let gate_span = yi_types::trace::span("tool.gate").arg("tool", tool.name());
            let args_json = serde_json::to_string(&args).unwrap_or_default();
            if let Some(rules) = &rules
                && let Some(denial) = rules.check_tool(tool.name(), &args_json)
            {
                return ToolOutcome {
                    result: yi_loop::tool::error_tool_result_kind(
                        &denial,
                        yi_types::event::ToolErrorKind::Denied,
                    ),
                    is_error: true,
                };
            }
            let kind = match tool.kind_for(&args) {
                yi_tools::ToolKind::Write => yi_tools::ToolKind::Write,
                _ => tool.kind(),
            };
            if let Some(denial) = wall.check(tool.name(), kind, &args, &context.cwd) {
                return ToolOutcome {
                    result: yi_loop::tool::error_tool_result_kind(
                        &denial,
                        yi_types::event::ToolErrorKind::Denied,
                    ),
                    is_error: true,
                };
            }
            drop(gate_span);
            for command in tool.arms(&args) {
                let walled = (walled_roots.as_slice(), spared.as_slice());
                let gates = (&rules, &wall, permission.as_ref());
                let refused = gate_armed(&command, gates, walled, &context).await;
                if let Some(denial) = refused {
                    return ToolOutcome {
                        result: yi_loop::tool::error_tool_result_kind(
                            &denial,
                            yi_types::event::ToolErrorKind::Denied,
                        ),
                        is_error: true,
                    };
                }
            }
            let (mut contained, mut outside): (Option<Arc<PermissionBroker>>, _) = (None, None);
            if let Some(broker) = permission {
                let reporter = Arc::clone(&broker);
                let gate_tool = Arc::clone(&tool);
                let gate_args = args.clone();
                let gate_cwd = context.cwd.clone();
                let outcome = tokio::task::spawn_blocking(move || {
                    let span = yi_types::trace::span("tool.preview").arg("tool", gate_tool.name());
                    let preview = gate_tool.preview(&gate_args, &gate_cwd);
                    drop(span);
                    let _span = yi_types::trace::span("tool.decide").arg("tool", gate_tool.name());
                    broker.decide_call(
                        gate_tool.name(),
                        gate_tool.kind(),
                        gate_tool.irreversible(&gate_args),
                        &call_id,
                        &gate_args,
                        preview.as_deref(),
                    )
                })
                .await;
                match outcome {
                    Ok(outcome) if outcome.allowed => {
                        if let Containment::Contained { widen, .. } = &outcome.containment {
                            context.sandbox = (reporter.sandbox_for(&context.cwd, &wall, widen))
                                .map(|mut sandbox| {
                                    sandbox.deny_read.extend_from_slice(&walled_roots);
                                    sandbox.spared.clone_from(&spared);
                                    sandbox
                                });
                            contained = context.sandbox.as_ref().map(|_| reporter);
                        } else if wall.container.is_none() {
                            outside = reporter.outside_notice(tool.name(), &args, &outcome);
                        }
                    }
                    Ok(outcome) => {
                        return ToolOutcome {
                            result: yi_loop::tool::error_tool_result_kind(
                                &format!("Permission denied: {}", outcome.reason),
                                yi_types::event::ToolErrorKind::Denied,
                            ),
                            is_error: true,
                        };
                    }
                    Err(join_error) => {
                        return ToolOutcome {
                            result: error_tool_result(&format!(
                                "permission check failed: {join_error}"
                            )),
                            is_error: true,
                        };
                    }
                }
            }
            let name = tool.name().to_owned();
            let command = args
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            // Outside the sandbox the command text is the only wall left (#1001).
            let on_host = context.sandbox.is_none() && context.container.is_none();
            if let Some(denial) = on_host
                .then(|| walled_root_refusal(&command, &walled_roots, &spared, &context.cwd))
                .flatten()
            {
                return ToolOutcome {
                    result: yi_loop::tool::error_tool_result_kind(
                        &denial,
                        yi_types::event::ToolErrorKind::Denied,
                    ),
                    is_error: true,
                };
            }
            let written = (tool.kind_for(&args) == yi_tools::ToolKind::Write)
                .then(|| crate::permission::extract_targets(&name, &args, &context.cwd));
            let (cwd, cancel) = (context.cwd.clone(), Arc::clone(&context.cancelled));
            let output = tokio::task::spawn_blocking(move || {
                let _span = yi_types::trace::span("tool.execute").arg("tool", tool.name());
                tool.execute(args, &context)
            })
            .await;
            match output {
                Ok(mut output) => {
                    let _after = yi_types::trace::span("tool.after").arg("tool", name.as_str());
                    // A contained command the sandbox refused asks the next time, rather than failing the same way forever.
                    // One at or naming a path under the own walls is never kept: Seatbelt judges
                    // each run, so a retry neither asks to leave nor claims the own transcript.
                    let own = |path: &std::path::Path| yi_tools::walled(&walled_roots, path);
                    let named = (command.split_whitespace())
                        .any(|word| own(&yi_permission::resolve_target(word, &cwd)));
                    if let Some(broker) = &contained
                        && let Some(refusal) = (output.result.details.get("sandboxRefusal"))
                            .and_then(yi_tools::SandboxRefusal::from_json)
                        && !named
                        && !matches!(&refusal, yi_tools::SandboxRefusal::Path(path) if own(path))
                    {
                        broker.note_containment_failure(refusal);
                    }
                    verdict_is_a_result(&mut output);
                    let holds = facts_of(&name, &output);
                    let facts = crate::affordance::Facts {
                        holds: &holds.iter().map(String::as_str).collect::<Vec<_>>(),
                        name: "",
                        cap: crate::affordance::TOOL_LINES,
                    };
                    for line in
                        crate::affordance::render(crate::affordance::shipped(), &name, &facts)
                            .into_iter()
                            .chain(outside)
                    {
                        crate::affordance::append(&mut output.result, &line);
                    }
                    if let (Some(check), Some(written), false) = (check, written, output.is_error) {
                        let verdict =
                            tokio::task::spawn_blocking(move || check(&written, &cwd, &cancel));
                        if let Ok(Some(line)) = verdict.await {
                            crate::affordance::append(&mut output.result, &line);
                        }
                    }
                    let text = result_text(&output.result);
                    if let Some(rules) = &rules {
                        let failed =
                            yi_types::event::shown_failed(output.is_error, &output.result.details);
                        rules.check_result(&name, &args_json, &text, failed);
                    }
                    if let Some(ext) = &ext {
                        ext(crate::ext::Event::ToolResult {
                            files_matched: files_matched(&name, &output.result),
                            exit: output
                                .result
                                .details
                                .get("exitCode")
                                .and_then(Value::as_i64)
                                .and_then(|code| i32::try_from(code).ok())
                                .or(Some(i32::from(output.is_error))),
                            check: name == "bash" && crate::ext::orchestrate::is_check(&command),
                            name,
                        });
                    }
                    ToolOutcome {
                        result: output.result,
                        is_error: output.is_error,
                    }
                }
                Err(join_error) => ToolOutcome {
                    result: error_tool_result(&format!("tool execution failed: {join_error}")),
                    is_error: true,
                },
            }
        })
    }
}
