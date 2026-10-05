#![forbid(unsafe_code)]
#![deny(clippy::string_slice)]

mod attach;
pub mod cells;
pub mod daemon;
mod forward;
pub mod review;
pub mod update;

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::task::{JoinHandle, JoinSet};
use yi_runtime::session_store::{
    CreateOptions, EntryOrder, EntryQuery, JsonlRepo, SessionRepo, SharedSession, lock_session,
};
use yi_runtime::{AgentSession, AskOutcome, Asker, SubagentHost, available_models, resolve_model};
use yi_types::acp::{
    AcpConfigChoice, AcpConfigKind, AcpConfigOption, AcpDiffChange, AcpDiffOperation, AcpDiffPatch,
    AcpErrorResponse, AcpErrorShape, AcpFrame, AcpImplementation, AcpInitializeResult, AcpMeta,
    AcpPatchFormat, AcpPermissionOption, AcpPermissionOptionKind, AcpPermissionOutcome,
    AcpPermissionSubject, AcpResponse, AcpSessionResult, AcpSessionUpdate, AcpToolCallUpdate,
    AcpToolContent,
};
use yi_types::event::AgentEvent;
use yi_types::permission::{choices, chosen};
use yi_types::subagent::ChildId;

use crate::forward::{Forward, Parent, forward_parent, session_name};
use crate::update::{
    IdMap, PendingPrompt, PendingPrompts, ReplayFrame, extension, replay_update, replay_updates,
    update_notification,
};

pub const PROTOCOL_VERSION: u16 = 2;
pub const VERSION_MISMATCH_ERROR: &str =
    "Unsupported protocol version: this agent speaks ACP v2 or later";

const INVALID_PARAMS: i64 = -32602;
const METHOD_NOT_FOUND: i64 = -32601;
const INTERNAL_ERROR: i64 = -32603;
const REQUEST_CANCELLED: i64 = -32800;

/// Builds a session for a store id, so a lane is claimed under the id that resumes it.
pub type SessionBuilder = Arc<
    dyn Fn(Option<Asker>, Option<&str>) -> Result<(AgentSession, Arc<SubagentHost>), String>
        + Send
        + Sync,
>;

const REPLAY_CHUNK: usize = 512;

/// Line sink for outgoing frames; injectable so the permission bridge and
/// mappers are testable without a real stdout.
pub type LineSink = Arc<dyn Fn(&Value) + Send + Sync>;

pub struct AcpOptions {
    pub session_dir: PathBuf,
    pub cwd: PathBuf,
    pub build: SessionBuilder,
    pub agent_version: String,
    pub defaults: Option<SessionDefaults>,
}

pub struct SessionDefaults {
    pub model: yi_types::model::Model,
    pub effort: yi_types::model::Effort,
    pub mode: yi_runtime::PermissionMode,
}

pub fn stdout_sink() -> LineSink {
    Arc::new(|value: &Value| {
        let _span = yi_types::trace::span("acp.write_frame");
        // One write per frame: stdout's line buffer turned a 1.4 MB frame into 1 KB syscalls.
        let Ok(mut line) = serde_json::to_vec(value) else {
            return;
        };
        line.push(b'\n');
        let mut stdout = std::io::stdout().lock();
        let _stdout_gone_means_exit = stdout.write_all(&line);
        let _flush = stdout.flush();
    })
}

/// Design §17.2: v2 or later is served as v2; v1 gets the exact mismatch error.
pub fn negotiate(protocol_version: u64) -> Result<u16, String> {
    if protocol_version >= u64::from(PROTOCOL_VERSION) {
        Ok(PROTOCOL_VERSION)
    } else {
        Err(VERSION_MISMATCH_ERROR.to_owned())
    }
}

/// `None` once stdin has ended: no answer can come, so an ask rejects instead of waiting.
type PendingAsks = Arc<Mutex<Option<HashMap<String, std::sync::mpsc::Sender<Value>>>>>;

fn permission_options(grants: &[yi_runtime::Grant]) -> Vec<AcpPermissionOption> {
    use AcpPermissionOptionKind::{AllowAlways, AllowOnce, RejectOnce};
    let labels = grants.iter().map(|grant| grant.label.as_str());
    choices(labels, AllowOnce, |_| AllowAlways, RejectOnce)
        .into_iter()
        .map(|(option_id, name, kind)| AcpPermissionOption {
            option_id,
            name,
            kind,
        })
        .collect()
}

/// Written synchronously BEFORE blocking, with the stdin reader thread routing the response
/// into the std channel, so a current-thread runtime cannot deadlock waiting on itself.
pub fn bridge_asker(session_id: String, sink: LineSink, pending: PendingAsks) -> Asker {
    let counter = Arc::new(Mutex::new(0_u64));
    Arc::new(move |ask: &yi_runtime::PermissionAsk<'_>| {
        let id = {
            let mut counter = counter
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *counter = counter.saturating_add(1);
            let n = *counter;
            format!("perm_{session_id}_{n}")
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        let open = (pending.lock().ok())
            .and_then(|mut map| map.as_mut().map(|map| map.insert(id.clone(), sender)))
            .is_some();
        if !open {
            return AskOutcome::Reject;
        }
        let params = yi_types::acp::AcpPermissionParams {
            session_id: session_id.clone(),
            title: ask.title.to_owned(),
            description: Some(ask.description.to_owned()),
            options: permission_options(ask.grants),
            subject: ask.patch.map(|patch| AcpPermissionSubject::ToolCall {
                tool_call: Box::new(AcpToolCallUpdate {
                    tool_call_id: ask.tool_call_id.unwrap_or_default().to_owned(),
                    content: Some(vec![AcpToolContent::Diff {
                        changes: ask.changes.iter().map(|path| diff_change(path)).collect(),
                        patch: Some(AcpDiffPatch {
                            format: AcpPatchFormat::GitPatch,
                            text: patch.to_owned(),
                        }),
                    }]),
                    ..AcpToolCallUpdate::default()
                }),
            }),
        };
        let request = AcpFrame {
            jsonrpc: "2.0".to_owned(),
            id: Some(Value::String(id.clone())),
            method: "session/request_permission".to_owned(),
            params: serde_json::to_value(&params).ok(),
        };
        sink(&json!(request));
        let response = receiver.recv();
        if let Some(map) = pending.lock().ok().as_mut().and_then(|map| map.as_mut()) {
            map.remove(&id);
        }
        let Ok(response) = response else {
            return AskOutcome::Reject;
        };
        let outcome = response
            .get("result")
            .and_then(|result| result.get("outcome"))
            .cloned()
            .unwrap_or(Value::Null);
        match serde_json::from_value::<AcpPermissionOutcome>(outcome) {
            Ok(AcpPermissionOutcome::Selected { option_id }) => chosen(
                &option_id,
                AskOutcome::AllowOnce,
                AskOutcome::AllowAlways,
                AskOutcome::Reject,
            ),
            _ => AskOutcome::Reject,
        }
    })
}

/// Invariant: an ask precedes the write it gates, so what is on disk is the pre-image.
fn diff_change(path: &std::path::Path) -> AcpDiffChange {
    let operation = if path.exists() {
        AcpDiffOperation::Modify
    } else {
        AcpDiffOperation::Add
    };
    AcpDiffChange {
        operation,
        path: path.to_string_lossy().into_owned(),
    }
}

struct SessionHandle {
    session: Arc<AgentSession>,
    host: Arc<SubagentHost>,
    forwarder: JoinHandle<()>,
    prompts: PendingPrompts,
}

impl Drop for SessionHandle {
    fn drop(&mut self) {
        self.forwarder.abort();
        // Each running child is revoked under its own `parent_close`, its work kept.
        let _revoked = self.host.close();
        self.session.retire();
    }
}

struct AcpState {
    user_cells: crate::cells::UserCells,
    repo: JsonlRepo,
    sessions: HashMap<String, SessionHandle>,
    building: HashMap<String, attach::Building>,
    failed: HashMap<String, String>,
    wake: tokio::sync::mpsc::WeakUnboundedSender<attach::Incoming>,
    defaults: Option<SessionDefaults>,
    build: SessionBuilder,
    sink: LineSink,
    pending: PendingAsks,
    agent_version: String,
    initialized: bool,
    cwd: PathBuf,
    prompt_serial: u64,
}

fn mode_value(mode: Option<yi_runtime::PermissionMode>) -> &'static str {
    mode.map_or("ask", yi_runtime::gate::mode_label)
}

fn config_options(session: &AgentSession) -> Vec<AcpConfigOption> {
    options_for(
        &session.model(),
        session.effort(),
        mode_value(session.permission_broker().map(|broker| broker.mode())),
    )
}

fn options_for(
    model: &yi_types::model::Model,
    effort: yi_types::model::Effort,
    mode: &str,
) -> Vec<AcpConfigOption> {
    let choice = |value: String, name: String| AcpConfigChoice { value, name };
    let models = available_models()
        .iter()
        .map(|candidate| {
            let value = format!("{}/{}", candidate.provider, candidate.id);
            choice(value, candidate.name.clone())
        })
        .collect();
    let modes = ["ask", "auto", "yolo"]
        .iter()
        .map(|mode| choice((*mode).to_owned(), (*mode).to_owned()))
        .collect();
    let levels = model
        .supported_efforts()
        .iter()
        .map(|level| choice(level.to_string(), level.to_string()))
        .collect();
    let select = |config_id: &str, name: &str, current_value: String, options| AcpConfigOption {
        config_id: config_id.to_owned(),
        name: name.to_owned(),
        kind: AcpConfigKind::Select,
        current_value,
        options,
    };
    vec![
        select(
            "model",
            "Model",
            format!("{}/{}", model.provider, model.id),
            models,
        ),
        select(
            "thought_level",
            "Thinking level",
            effort.to_string(),
            levels,
        ),
        select("mode", "Permission mode", mode.to_owned(), modes),
    ]
}

fn prompt_text(params: &Value) -> String {
    let blocks = params.get("prompt").and_then(Value::as_array);
    blocks
        .into_iter()
        .flatten()
        .fold(String::new(), |mut text, block| {
            let field = |key| block.get(key).and_then(Value::as_str);
            match (
                block.get("type").and_then(Value::as_str),
                field("text"),
                field("uri"),
            ) {
                (Some("text"), Some(part), _) => text.push_str(part),
                (Some("resource_link"), _, Some(uri)) => {
                    if !text.is_empty() && !text.ends_with(char::is_whitespace) {
                        text.push(' ');
                    }
                    text.push_str(uri);
                }
                _ => {}
            }
            text
        })
}

impl AcpState {
    fn attach(&mut self, store: &SharedSession) -> Result<String, String> {
        let session_id = lock_session(store).metadata().id.clone();
        let asker = bridge_asker(
            session_id.clone(),
            Arc::clone(&self.sink),
            Arc::clone(&self.pending),
        );
        let built = {
            let _span = yi_types::trace::span("acp.build_session");
            (self.build)(Some(asker), Some(&session_id))
        };
        self.adopt(store, built)
    }

    fn adopt(&mut self, store: &SharedSession, built: attach::Built) -> Result<String, String> {
        let session_id = lock_session(store).metadata().id.clone();
        let (session, host) = built?;
        let _attaching = yi_types::trace::span("acp.attach_store");
        session
            .attach_store(Arc::clone(store))
            .map_err(|error| error.to_string())?;
        if let Some(lane) = session.lane() {
            lane.reattach();
        }
        let session = Arc::new(session);
        if let Some(todos) = session.todos() {
            let sink = Arc::clone(&self.sink);
            let id = session_id.clone();
            todos.on_change(Arc::new(move |list| {
                let update = extension(
                    "_yi/todo",
                    [("list", serde_json::to_value(list).unwrap_or(Value::Null))],
                );
                (sink)(&update_notification(&id, update));
            }));
        }
        let events = session.subscribe();
        let context_window = session.model().context_window;
        let prompts = PendingPrompts::default();
        let mut parent = Parent {
            forward: Forward {
                session_id: session_id.clone(),
                child: None,
                seq: Arc::new(AtomicU64::new(0)),
                sink: Arc::clone(&self.sink),
            },
            session: Arc::clone(&session),
            host: Arc::clone(&host),
            ids: IdMap::with_prompts(context_window, Arc::clone(&prompts)),
            children: JoinSet::new(),
            seen: HashSet::new(),
            last_goal: Value::Null,
            last_workdir: Value::Null,
            last_claims: Value::Null,
            last_plan: Value::Null,
            titled: lock_session(store).name().is_some(),
            launch_cwd: self.cwd.clone(),
        };
        parent.watch_workdir();
        parent.watch_claims();
        parent.watch_plan();
        let forwarder = tokio::spawn(forward_parent(events, parent));
        self.sessions.insert(
            session_id.clone(),
            SessionHandle {
                session,
                host,
                forwarder,
                prompts,
            },
        );
        Ok(session_id)
    }

    fn spawn_user_cell(
        &mut self,
        session_id: String,
        service: Arc<yi_runtime::kernel::KernelService>,
        events: tokio::sync::broadcast::Sender<AgentEvent>,
        code: String,
    ) -> Result<Value, (i64, String)> {
        let call_id = self.user_cells.admit(&session_id)?;
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag: Arc<dyn Fn() -> bool + Send + Sync> = {
            let cancelled = Arc::clone(&cancelled);
            Arc::new(move || cancelled.load(std::sync::atomic::Ordering::SeqCst))
        };
        let start = AgentEvent::ToolExecutionStart {
            tool_call_id: call_id.clone(),
            tool_name: "ipython".to_owned(),
            args: json!({ "code": code }),
        };
        if events.send(start).is_err() {
            return Err((INTERNAL_ERROR, "the session is closed".to_owned()));
        }
        let id = call_id.clone();
        let task = tokio::spawn(async move {
            let output = service.execute_user_cell(&code, &flag).await;
            let end = AgentEvent::ToolExecutionEnd {
                tool_call_id: id.clone(),
                tool_name: "ipython".to_owned(),
                result: output.result,
                is_error: output.is_error,
            };
            if events.send(end).is_err() {
                eprintln!("user cell {id}: the session closed before its output");
            }
        });
        self.user_cells.track(
            &session_id,
            crate::cells::UserCell {
                call_id: call_id.clone(),
                cancelled,
                task,
            },
        );
        Ok(json!({ "callId": call_id }))
    }

    fn session(&self, params: &Value) -> Result<(&SessionHandle, String), (i64, String)> {
        let id = params
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or((INVALID_PARAMS, "missing sessionId".to_owned()))?;
        if let Some(error) = self.failed.get(id) {
            return Err((INTERNAL_ERROR, error.clone()));
        }
        self.sessions
            .get(id)
            .map(|handle| (handle, id.to_owned()))
            .ok_or((INVALID_PARAMS, format!("unknown session {id}")))
    }

    fn session_result(&self, session_id: &str) -> AcpSessionResult {
        let handle = self.sessions.get(session_id);
        let options = handle
            .map(|handle| config_options(&handle.session))
            .unwrap_or_default();
        let name = self
            .store_of(session_id)
            .and_then(|store| session_name(&store));
        let mut meta = AcpMeta::default();
        meta.yi.name = name;
        AcpSessionResult {
            session_id: Some(session_id.to_owned()),
            config_options: options,
            meta: Some(meta),
        }
    }

    /// Answered by the forwarder once the prompt is inserted; only a refusal answers here.
    fn prompt(&mut self, request: Value, params: &Value) -> Option<Result<Value, (i64, String)>> {
        if let Some(id) = params.get("sessionId").and_then(Value::as_str) {
            let id = id.to_owned();
            self.settle(&id);
        }
        self.prompt_serial = self.prompt_serial.saturating_add(1);
        let message_id = format!("msg_p{}", self.prompt_serial);
        let (handle, _) = match self.session(params) {
            Ok(found) => found,
            Err(refused) => return Some(Err(refused)),
        };
        let text = prompt_text(params);
        crate::update::queued(&handle.prompts).push(PendingPrompt {
            text: text.clone(),
            message_id,
            request,
        });
        let prompt = yi_runtime::session::user_input(&text);
        if handle.session.prompt_message(prompt.clone()).is_err() {
            handle.session.follow_up_message(prompt);
        }
        None
    }

    fn drop_session(&mut self, session_id: &str) {
        if let Some(handle) = self.sessions.remove(session_id) {
            // Incident: the turn and its queued follow-ups ran on after a close told the client
            // they were cancelled.
            handle.session.stop();
            let closed = "the session closed before this prompt was inserted";
            refuse(&self.sink, &handle.prompts, REQUEST_CANCELLED, closed);
        }
    }

    fn emit_config(&self, session_id: &str) {
        let Some(handle) = self.sessions.get(session_id) else {
            return;
        };
        let update = extension(
            "_yi/config",
            [
                ("configOptions", json!(config_options(&handle.session))),
                (
                    "contextWindow",
                    Value::from(handle.session.model().context_window),
                ),
            ],
        );
        (self.sink)(&update_notification(session_id, update));
    }

    fn emit_goal(&self, session_id: &str, goal: &Value) {
        let update = extension("_yi/goal", [("goal", goal.clone())]);
        (self.sink)(&update_notification(session_id, update));
    }

    fn set_config_option(&mut self, params: &Value) -> Result<Value, (i64, String)> {
        let (handle, id) = self.session(params)?;
        let config_id = params.get("configId").and_then(Value::as_str).unwrap_or("");
        if params.get("type").and_then(Value::as_str) != Some("id") {
            let refused = format!("{config_id} is a select option: send \"type\": \"id\"");
            return Err((INVALID_PARAMS, refused));
        }
        let value = params.get("value").and_then(Value::as_str).unwrap_or("");
        match config_id {
            "model" => {
                let (provider, model_id) = value
                    .split_once('/')
                    .ok_or((INVALID_PARAMS, format!("unknown model {value}")))?;
                let model = resolve_model(provider, model_id)
                    .ok_or((INVALID_PARAMS, format!("unknown model {value}")))?;
                handle.session.set_model(model);
            }
            "thought_level" => {
                let effort = value
                    .parse()
                    .map_err(|error: yi_types::model::UnknownEffort| {
                        (INVALID_PARAMS, error.to_string())
                    })?;
                handle.session.set_effort(effort);
            }
            "mode" => {
                let mode = yi_runtime::slash::parse_mode(value).map_err(|_| {
                    (
                        INVALID_PARAMS,
                        format!("unknown mode {value}; use ask|auto|yolo"),
                    )
                })?;
                handle
                    .session
                    .permission_broker()
                    .ok_or((
                        INTERNAL_ERROR,
                        "no permission broker is attached".to_owned(),
                    ))?
                    .set_mode_and_fragment(mode, &handle.session);
            }
            other => {
                return Err((
                    INVALID_PARAMS,
                    format!("unsupported config option in this build: {other}"),
                ));
            }
        }
        self.emit_config(&id);
        Ok(json!({"configOptions": self.session_result(&id).config_options}))
    }

    fn handle(&mut self, method: &str, params: &Value) -> Result<Value, (i64, String)> {
        if method != "session/resume"
            && let Some(id) = params.get("sessionId").and_then(Value::as_str)
        {
            let id = id.to_owned();
            self.settle(&id);
        }
        match method {
            "initialize" => {
                let requested = params
                    .get("protocolVersion")
                    .and_then(Value::as_u64)
                    .ok_or((INVALID_PARAMS, "missing protocolVersion".to_owned()))?;
                let agreed = negotiate(requested).map_err(|message| (INVALID_PARAMS, message))?;
                self.initialized = true;
                Ok(json!(AcpInitializeResult {
                    protocol_version: agreed,
                    info: AcpImplementation {
                        name: "yi".to_owned(),
                        version: self.agent_version.clone(),
                    },
                    capabilities: json!({"session": {"delete": {}}}),
                    auth_methods: Vec::new(),
                }))
            }
            "session/new" => {
                let creating = yi_types::trace::span("acp.repo_create");
                let store = self
                    .repo
                    .create(CreateOptions::default())
                    .map_err(|error| (INTERNAL_ERROR, error.to_string()))?;
                drop(creating);
                let Some(defaults) = &self.defaults else {
                    let session_id = self
                        .attach(&store)
                        .map_err(|error| (INTERNAL_ERROR, error))?;
                    return Ok(json!(self.session_result(&session_id)));
                };
                let options = options_for(
                    &defaults.model,
                    defaults.model.clamp_effort(defaults.effort),
                    mode_value(Some(defaults.mode)),
                );
                let session_id = self.attach_later(&store);
                let mut result = self.session_result(&session_id);
                result.config_options = options;
                Ok(json!(result))
            }
            "session/cancel" => {
                let (handle, _) = self.session(params)?;
                handle.session.abort();
                Ok(json!({}))
            }
            "session/list" => {
                let _listing = yi_types::trace::span("acp.repo_list");
                let sessions: Vec<Value> = self
                    .repo
                    .list()
                    .map_err(|error| (INTERNAL_ERROR, error.to_string()))?
                    .iter()
                    .map(|metadata| {
                        json!({
                            "sessionId": metadata.id,
                            "cwd": self.cwd,
                            "title": metadata.name,
                            "_meta": {"yi": {
                                "attached": self.sessions.contains_key(&metadata.id)
                                    || self.building.contains_key(&metadata.id),
                                "createdAt": metadata.created_at,
                            }},
                        })
                    })
                    .collect();
                Ok(json!({"sessions": sessions}))
            }
            "session/resume" => {
                let id = params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .ok_or((INVALID_PARAMS, "missing sessionId".to_owned()))?
                    .to_owned();
                let store = match self.store_of(&id) {
                    Some(store) => store,
                    None => {
                        let _span = yi_types::trace::span("acp.repo_open");
                        let store = self
                            .repo
                            .open(&id)
                            .map_err(|error| (INVALID_PARAMS, error.to_string()))?;
                        self.attach_later(&store);
                        store
                    }
                };
                let mut result = self.session_result(&id);
                result.session_id = None;
                // v2 clients send `{"type": "start"}`; a bare number is the
                // entry-offset extension. Anything non-null replays.
                if let Some(replay_from) = params.get("replayFrom").filter(|v| !v.is_null()) {
                    let from = replay_from.as_u64().unwrap_or(0);
                    let window = self.context_window(&id);
                    let standard = params
                        .pointer("/_meta/yi/replayUpdates")
                        .and_then(Value::as_bool)
                        != Some(false);
                    let replayed = if standard {
                        Some(self.replay(&id, &store, from, window)?)
                    } else {
                        None
                    };
                    let total = self.emit_replay_from(&id, &store, from, window, None)?;
                    let meta = result.meta.get_or_insert_with(AcpMeta::default);
                    meta.yi.replayed_to = Some(replayed.unwrap_or(total));
                }
                Ok(json!(result))
            }
            "session/close" => {
                let id = self.session(params)?.1;
                self.user_cells.close(&id);
                self.drop_session(&id);
                Ok(json!({}))
            }
            "session/delete" => {
                let id = params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .ok_or((INVALID_PARAMS, "missing sessionId".to_owned()))?
                    .to_owned();
                self.drop_session(&id);
                self.repo
                    .delete(&id)
                    .map_err(|error| (INVALID_PARAMS, error.to_string()))?;
                Ok(json!({}))
            }
            "session/set_config_option" => self.set_config_option(params),
            "_yi/heartbeat" | "_yi/goal" | "_yi/tracked" | "_yi/branch_diff" | "_yi/tape"
            | "_yi/kernel_execute" | "_yi/kernel_cancel" | "_yi/slash" => {
                self.handle_extension(method, params)
            }
            "_yi/steer" | "_yi/rewind" | "_yi/plan" | "_yi/todo" | "_yi/child_replay"
            | "_yi/child_abort" | "_yi/child_answer" => self.handle_control(method, params),
            other => Err((METHOD_NOT_FOUND, format!("unknown method {other}"))),
        }
    }

    fn handle_control(&mut self, method: &str, params: &Value) -> Result<Value, (i64, String)> {
        let (handle, session_id) = self.session(params)?;
        let text = |key: &str| params.get(key).and_then(Value::as_str).unwrap_or("");
        match method {
            "_yi/steer" => {
                let prompt = yi_runtime::session::user_input(text("text"));
                handle.session.steer_message(prompt);
                Ok(json!({}))
            }
            "_yi/rewind" => {
                let files = params.get("files").and_then(Value::as_bool) == Some(true);
                let (rewound, restored) = if files {
                    let (rewound, restored) =
                        crate::review::restore_before(&handle.session, text("entryId"), &self.cwd)?;
                    (rewound, Some(restored))
                } else {
                    let rewound = yi_runtime::rewind_to(&handle.session, text("entryId"))
                        .map_err(|error| (INVALID_PARAMS, error))?;
                    (rewound, None)
                };
                let summarizing = rewound.abandoned.is_some();
                if let Some(stub) = rewound.abandoned {
                    let session = Arc::clone(&handle.session);
                    tokio::spawn(async move {
                        yi_runtime::summarize_branch(&session, stub).await;
                    });
                }
                self.emit_replay(&session_id, 0, None)?;
                Ok(json!({
                    "leafId": rewound.leaf,
                    "unsent": rewound.unsent,
                    "summarizing": summarizing,
                    "restored": restored,
                }))
            }
            "_yi/todo" => {
                let list = handle.session.todos().map(|store| store.list());
                Ok(json!({"list": list}))
            }
            "_yi/plan" => {
                let service = handle
                    .session
                    .plan_service()
                    .ok_or((INTERNAL_ERROR, "no plan service is attached".to_owned()))?;
                if text("action") == "submit" {
                    return submit_plan(&service, params);
                }
                let plan = service
                    .read_plan()
                    .map_err(|error| (INVALID_PARAMS, error.to_string()))?;
                let subplans = yi_runtime::plan::subplans_of(&plan, service.plans_dir());
                Ok(json!({"plan": plan, "subplans": subplans}))
            }
            "_yi/child_answer" => {
                let told =
                    handle
                        .host
                        .answer_told(text("childId"), text("questionId"), text("text"));
                Ok(json!({ "text": told }))
            }
            "_yi/child_replay" | "_yi/child_abort" => {
                let child_id = ChildId(text("childId").to_owned());
                let child = handle
                    .host
                    .children_view()
                    .into_iter()
                    .find(|child| child.update.id == child_id)
                    .ok_or((INVALID_PARAMS, format!("unknown child {}", child_id.0)))?;
                if method == "_yi/child_abort" {
                    let _stopped = handle.host.interrupt(&child_id.0);
                    return Ok(json!({}));
                }
                let store = child
                    .session
                    .store()
                    .ok_or((INTERNAL_ERROR, "the child has no store".to_owned()))?;
                let context_window = child.session.model().context_window;
                self.emit_replay_from(&session_id, &store, 0, context_window, Some(&child_id))?;
                Ok(json!({}))
            }
            other => Err((METHOD_NOT_FOUND, format!("unknown method {other}"))),
        }
    }

    /// `_yi/*` extension methods (§17.2): the heartbeat and goal surfaces.
    fn handle_extension(&mut self, method: &str, params: &Value) -> Result<Value, (i64, String)> {
        let (handle, session_id) = self.session(params)?;
        let text = |key: &str| params.get(key).and_then(Value::as_str).unwrap_or("");
        match method {
            "_yi/kernel_execute" => {
                let code = crate::cells::cell_code(params)?;
                let service = handle
                    .session
                    .kernel_service()
                    .ok_or((INTERNAL_ERROR, "no kernel is attached".to_owned()))?;
                let events = handle.session.events_sender();
                self.spawn_user_cell(session_id, service, events, code)
            }
            "_yi/kernel_cancel" => {
                let call = text("callId");
                if self.user_cells.cancel(&session_id, call) {
                    return Ok(json!({}));
                }
                Err((INVALID_PARAMS, format!("no running user cell {call}")))
            }
            "_yi/tracked" => {
                let paths: Vec<String> = params
                    .get("paths")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                if paths.len() > 64 {
                    return Err((INVALID_PARAMS, "at most 64 paths per call".to_owned()));
                }
                let root = std::path::Path::new(text("root"));
                Ok(json!({"tracked": yi_runtime::environment::tracked(root, &paths)}))
            }
            "_yi/why" => Ok(crate::review::why(&handle.session, &self.cwd, params)),
            "_yi/tape" | "_yi/branch_diff" => {
                let value = if method == "_yi/tape" {
                    serde_json::to_value(yi_runtime::tape::session_tape(&handle.session))
                } else {
                    let root = crate::review::root(&handle.session, &self.cwd);
                    serde_json::to_value(yi_runtime::environment::branch_diff(&root))
                };
                Ok(value.unwrap_or(Value::Null))
            }
            "_yi/slash" => {
                let line = text("line").trim();
                let (command, args) = yi_runtime::slash::split(line);
                let before = (handle.session.model().id, handle.session.effort());
                let reply = match command {
                    "sessions" => self.sessions_text()?,
                    "undo" => yi_runtime::slash::undo(&handle.session, &self.cwd),
                    // Routed through `_yi/heartbeat`, not `slash::run`, so the client keeps `_yi/heartbeat_changed` (C9).
                    "heartbeat" => {
                        return self.handle_extension(
                            "_yi/heartbeat",
                            &json!({"sessionId": session_id, "command": args}),
                        );
                    }
                    other => yi_runtime::slash::run(&handle.session, other, args)
                        .ok_or((INVALID_PARAMS, format!("unknown command: /{other}")))?,
                };
                let after = self
                    .sessions
                    .get(&session_id)
                    .map(|handle| (handle.session.model().id, handle.session.effort()));
                if after.is_some_and(|after| after != before) {
                    self.emit_config(&session_id);
                }
                Ok(json!({"text": reply}))
            }
            "_yi/heartbeat" => {
                let service = handle
                    .session
                    .heartbeat_service()
                    .ok_or((INTERNAL_ERROR, "no scheduler is attached".to_owned()))?;
                match service.run(text("command")) {
                    Ok(reply) => {
                        self.emit_heartbeat_changed(&session_id, &reply);
                        Ok(json!({"text": reply}))
                    }
                    Err(error) => Err((INVALID_PARAMS, error)),
                }
            }
            "_yi/goal" => {
                let service = handle
                    .session
                    .goal_service()
                    .ok_or((INTERNAL_ERROR, "no goal service is attached".to_owned()))?;
                // Blocks dispatch up to the check timeout — same class as
                // the ACP permission bridge's synchronous wait.
                let outcome = service.act(params.as_object().unwrap_or(&serde_json::Map::new()));
                match outcome {
                    Ok(goal) => {
                        if text("action") != "get" {
                            self.emit_goal(&session_id, &goal);
                        }
                        Ok(json!({"goal": goal}))
                    }
                    Err(error) => Err((INVALID_PARAMS, error)),
                }
            }
            other => Err((METHOD_NOT_FOUND, format!("unknown method {other}"))),
        }
    }

    fn sessions_text(&mut self) -> Result<String, (i64, String)> {
        let listed = self
            .repo
            .list()
            .map_err(|error| (INTERNAL_ERROR, error.to_string()))?;
        Ok(yi_runtime::slash::sessions_listing(&listed))
    }

    /// A schedule mutation is a fact the client cannot infer from the
    /// method reply alone, since a heartbeat also fires without one.
    fn emit_heartbeat_changed(&self, session_id: &str, reply: &str) {
        let mut fields = std::collections::BTreeMap::new();
        fields.insert("text".to_owned(), Value::String(reply.to_owned()));
        let update = AcpSessionUpdate::Extension(yi_types::acp::AcpExtensionUpdate {
            session_update: "_yi/heartbeat_changed".to_owned(),
            fields,
        });
        (self.sink)(&update_notification(session_id, update));
    }

    fn context_window(&self, session_id: &str) -> u64 {
        self.sessions
            .get(session_id)
            .map(|handle| handle.session.model().context_window)
            .or_else(|| {
                self.defaults
                    .as_ref()
                    .map(|defaults| defaults.model.context_window)
            })
            .unwrap_or(0)
    }

    fn emit_replay(
        &self,
        session_id: &str,
        from: u64,
        child: Option<&ChildId>,
    ) -> Result<(), (i64, String)> {
        let Some(handle) = self.sessions.get(session_id) else {
            return Ok(());
        };
        let Some(store) = handle.session.store() else {
            return Ok(());
        };
        let context_window = handle.session.model().context_window;
        self.emit_replay_from(session_id, &store, from, context_window, child)
            .map(|_| ())
    }

    fn emit_replay_from(
        &self,
        session_id: &str,
        store: &SharedSession,
        from: u64,
        context_window: u64,
        child: Option<&ChildId>,
    ) -> Result<u64, (i64, String)> {
        let mut span = yi_types::trace::span("acp.emit_replay");
        let (entries, leaf, goal) = {
            let session = lock_session(store);
            let entries = session
                .find_entries(&EntryQuery {
                    order: EntryOrder::OldestFirst,
                    ..EntryQuery::default()
                })
                .map_err(|error| (INTERNAL_ERROR, error.to_string()))?;
            let leaf = session.leaf_id("main").ok().flatten();
            (entries, leaf, session.goal())
        };
        let todos = yi_runtime::todo::latest_record(store).map(|record| record.list);
        let name = session_name(store);
        let total = u64::try_from(entries.len()).unwrap_or(u64::MAX);
        let skip = usize::try_from(from.min(total)).unwrap_or(usize::MAX);
        let tail = entries.get(skip..).unwrap_or_default();
        let mut sent = from.min(total);
        span.set("entries", tail.len());
        let mut chunks = tail.chunks(REPLAY_CHUNK).peekable();
        while let Some(chunk) = chunks.next() {
            let chunk_from = sent;
            sent = sent.saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
            let last = chunks.peek().is_none();
            let frame = ReplayFrame {
                entries: chunk,
                from: chunk_from,
                replayed_to: sent,
                leaf: if last { leaf.as_deref() } else { None },
                name: name.as_deref(),
                goal: goal.as_ref(),
                todos: todos.as_ref(),
                context_window,
                child,
            };
            (self.sink)(&update_notification(session_id, replay_update(&frame)));
        }
        if tail.is_empty() {
            let frame = ReplayFrame {
                entries: &[],
                from: sent,
                replayed_to: total,
                leaf: leaf.as_deref(),
                name: name.as_deref(),
                goal: goal.as_ref(),
                todos: todos.as_ref(),
                context_window,
                child,
            };
            (self.sink)(&update_notification(session_id, replay_update(&frame)));
        }
        Ok(total)
    }

    /// Design §17.2: the stored branch replayed as `session/update`s. `from` skips entries the
    /// client holds (valid only if it saw nothing since); returns the next `replayedTo`.
    fn replay(
        &self,
        session_id: &str,
        store: &SharedSession,
        from: u64,
        context_window: u64,
    ) -> Result<u64, (i64, String)> {
        let mut span = yi_types::trace::span("acp.replay_updates");
        let entries = lock_session(store)
            .find_entries(&EntryQuery {
                order: EntryOrder::OldestFirst,
                ..EntryQuery::default()
            })
            .map_err(|error| (INTERNAL_ERROR, error.to_string()))?;
        let total = u64::try_from(entries.len()).unwrap_or(u64::MAX);
        let skip = usize::try_from(from.min(total)).unwrap_or(usize::MAX);
        let tail = entries.get(skip..).unwrap_or_default();
        let mut ids = IdMap::new(context_window);
        let mut frames = 0_u64;
        for updated in replay_updates(tail, &mut ids) {
            frames = frames.saturating_add(1);
            (self.sink)(&update_notification(session_id, updated));
        }
        span.set("entries", tail.len());
        span.set("frames", frames);
        Ok(total)
    }
}

/// Invariant: the daemon fans this worker's asks out to every attached client, so a submit
/// carries no principal and is never confirmed here: an administrative op is `NoConfirmer`.
fn submit_plan(
    service: &yi_runtime::plan::PlanService,
    params: &Value,
) -> Result<Value, (i64, String)> {
    let payload = params
        .as_object()
        .ok_or((INVALID_PARAMS, "params must be an object".to_owned()))?;
    yi_runtime::plan::authority::submit_request(service, payload, None)
        .map_err(|error| (INVALID_PARAMS, error))
}

fn refuse(sink: &LineSink, prompts: &PendingPrompts, code: i64, why: &str) {
    let waiting = std::mem::take(&mut *crate::update::queued(prompts));
    for prompt in waiting {
        respond(sink, prompt.request, Err((code, why.to_owned())));
    }
}

fn respond(sink: &LineSink, id: Value, outcome: Result<Value, (i64, String)>) {
    let frame = match outcome {
        Ok(result) => json!(AcpResponse {
            jsonrpc: "2.0".to_owned(),
            id,
            result,
        }),
        Err((code, message)) => json!(AcpErrorResponse {
            jsonrpc: "2.0".to_owned(),
            id,
            error: AcpErrorShape { code, message },
        }),
    };
    sink(&frame);
}

pub fn run_acp(options: AcpOptions, runtime: tokio::runtime::Runtime) -> i32 {
    let repo = JsonlRepo::new(options.session_dir, options.cwd.display().to_string());
    let sink = stdout_sink();
    let pending: PendingAsks = Arc::new(Mutex::new(Some(HashMap::new())));
    let (line_tx, mut line_rx) = tokio::sync::mpsc::unbounded_channel::<attach::Incoming>();
    let mut state = AcpState {
        user_cells: crate::cells::UserCells::default(),
        repo,
        sessions: HashMap::new(),
        building: HashMap::new(),
        failed: HashMap::new(),
        wake: line_tx.downgrade(),
        defaults: options.defaults,
        build: options.build,
        sink: Arc::clone(&sink),
        pending: Arc::clone(&pending),
        agent_version: options.agent_version,
        initialized: false,
        cwd: options.cwd,
        prompt_serial: 0,
    };
    runtime.block_on(async move {
        std::thread::spawn(move || {
            use std::io::BufRead;
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                let Ok(line) = line else { break };
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                // Permission responses are routed here, off the executor:
                // the asker blocks the current-thread runtime while waiting.
                if let Ok(value) = serde_json::from_str::<Value>(trimmed)
                    && value.get("method").is_none()
                    && let Some(id) = value.get("id").and_then(Value::as_str)
                {
                    let sender =
                        (pending.lock().ok()).and_then(|map| map.as_ref()?.get(id).cloned());
                    if let Some(sender) = sender {
                        let _ = sender.send(value);
                        continue;
                    }
                }
                if line_tx
                    .send(attach::Incoming::Line(trimmed.to_owned()))
                    .is_err()
                {
                    break;
                }
            }
            // Incident: a bash ask in flight when stdin closed waited forever on a reply, and the
            // runtime's drop waited on that blocking task, so `yi acp` never exited (Linux asks).
            if let Ok(mut map) = pending.lock() {
                *map = None;
            }
        });
        while let Some(incoming) = line_rx.recv().await {
            let line = match incoming {
                attach::Incoming::Line(line) => line,
                attach::Incoming::Built(session_id) => {
                    state.settle(&session_id);
                    continue;
                }
            };
            let Ok(incoming) = serde_json::from_str::<AcpFrame>(&line) else {
                respond(
                    &sink,
                    Value::Null,
                    Err((-32700, "parse error: not a JSON-RPC frame".to_owned())),
                );
                continue;
            };
            let params = incoming.params.unwrap_or(Value::Null);
            match incoming.id {
                Some(id) => {
                    let _span = yi_types::trace::span(format!("acp {}", incoming.method));
                    let outcome = if incoming.method == "session/prompt" {
                        state.prompt(id.clone(), &params)
                    } else {
                        Some(state.handle(&incoming.method, &params))
                    };
                    if let Some(outcome) = outcome {
                        respond(&sink, id, outcome);
                    }
                }
                None => {
                    if incoming.method == "session/cancel" {
                        let _ = state.handle("session/cancel", &params);
                    }
                }
            }
        }
        for handle in state.sessions.values() {
            handle.session.abort();
        }
        0
    })
}
