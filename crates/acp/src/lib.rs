#![forbid(unsafe_code)]
#![deny(clippy::string_slice)]

pub mod cells;
pub mod daemon;
mod forward;
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
    AcpConfigOption, AcpErrorResponse, AcpErrorShape, AcpFrame, AcpImplementation,
    AcpInitializeResult, AcpNotification, AcpPermissionOption, AcpPermissionOptionKind,
    AcpPermissionOutcome, AcpResponse, AcpSessionResult, AcpSessionUpdate, AcpUpdateParams,
};
use yi_types::event::AgentEvent;
use yi_types::subagent::ChildId;

use crate::forward::{Forward, Parent, forward_parent, session_name};
use crate::update::{IdMap, ReplayFrame, extension, replay_update, replay_updates};

pub const PROTOCOL_VERSION: u16 = 2;
pub const VERSION_MISMATCH_ERROR: &str =
    "Unsupported protocol version: this agent speaks ACP v2 or later";

const INVALID_PARAMS: i64 = -32602;
const METHOD_NOT_FOUND: i64 = -32601;
const INTERNAL_ERROR: i64 = -32603;

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
}

pub fn stdout_sink() -> LineSink {
    Arc::new(|value: &Value| {
        let mut stdout = std::io::stdout().lock();
        if serde_json::to_writer(&mut stdout, value).is_ok() {
            let _stdout_gone_means_exit = stdout.write_all(b"\n");
            let _flush = stdout.flush();
        }
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

type PendingAsks = Arc<Mutex<HashMap<String, std::sync::mpsc::Sender<Value>>>>;

/// Grant 0 keeps the `allow_always` id, so a client that knows one always-option still works.
fn permission_options(grants: &[yi_runtime::Grant]) -> Vec<AcpPermissionOption> {
    let always = grants
        .iter()
        .enumerate()
        .map(|(index, grant)| AcpPermissionOption {
            option_id: match index {
                0 => "allow_always".to_owned(),
                _ => format!("allow_always_{index}"),
            },
            name: format!("Always allow {}", grant.label),
            kind: AcpPermissionOptionKind::AllowAlways,
        });
    let bare = grants.is_empty().then(|| AcpPermissionOption {
        option_id: "allow_always".to_owned(),
        name: "Always allow".to_owned(),
        kind: AcpPermissionOptionKind::AllowAlways,
    });
    std::iter::once(AcpPermissionOption {
        option_id: "allow_once".to_owned(),
        name: "Allow once".to_owned(),
        kind: AcpPermissionOptionKind::AllowOnce,
    })
    .chain(always)
    .chain(bare)
    .chain(std::iter::once(AcpPermissionOption {
        option_id: "reject_once".to_owned(),
        name: "Reject".to_owned(),
        kind: AcpPermissionOptionKind::RejectOnce,
    }))
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
        if let Ok(mut map) = pending.lock() {
            map.insert(id.clone(), sender);
        }
        let params = yi_types::acp::AcpPermissionParams {
            session_id: session_id.clone(),
            title: ask.title.to_owned(),
            description: Some(ask.description.to_owned()),
            options: permission_options(ask.grants),
            content: ask.patch.map(|patch| {
                vec![yi_types::acp::AcpToolContent::Diff {
                    changes: ask
                        .changes
                        .iter()
                        .map(|path| path.to_string_lossy().into_owned())
                        .collect(),
                    patch: patch.to_owned(),
                }]
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
        if let Ok(mut map) = pending.lock() {
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
            Ok(AcpPermissionOutcome::Selected { option_id }) => match option_id.as_str() {
                "allow_once" => AskOutcome::AllowOnce,
                "allow_always" => AskOutcome::AllowAlways(0),
                other => other
                    .strip_prefix("allow_always_")
                    .and_then(|index| index.parse().ok())
                    .map_or(AskOutcome::Reject, AskOutcome::AllowAlways),
            },
            _ => AskOutcome::Reject,
        }
    })
}

struct SessionHandle {
    session: Arc<AgentSession>,
    host: Arc<SubagentHost>,
    forwarder: JoinHandle<()>,
}

impl Drop for SessionHandle {
    fn drop(&mut self) {
        self.forwarder.abort();
        // Each running child is revoked under its own `parent_close`, its work kept.
        let _revoked = self.host.close();
        self.session.dispose_kernel();
    }
}

struct AcpState {
    user_cells: crate::cells::UserCells,
    repo: JsonlRepo,
    sessions: HashMap<String, SessionHandle>,
    build: SessionBuilder,
    sink: LineSink,
    pending: PendingAsks,
    agent_version: String,
    initialized: bool,
    cwd: PathBuf,
}

fn undo_text(session: &AgentSession, cwd: &std::path::Path) -> String {
    if session.status() == yi_runtime::Status::Running {
        return "/undo: the current turn is still running (esc stops it)".to_owned();
    }
    let Some(store) = session.store() else {
        return "/undo: this session has no store to read checkpoints from".to_owned();
    };
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    match yi_runtime::undo(&store, cwd, &home) {
        yi_runtime::UndoOutcome::Restored { changes, scoped } => {
            format!("/undo: {}", yi_runtime::describe_undo(&changes, scoped))
        }
        yi_runtime::UndoOutcome::NoCheckpoint => "/undo: no checkpoint to restore".to_owned(),
        yi_runtime::UndoOutcome::Failed(error) => format!("/undo: {error}"),
    }
}

pub(crate) fn update_notification(session_id: &str, update: AcpSessionUpdate) -> Value {
    json!(AcpNotification {
        jsonrpc: "2.0".to_owned(),
        method: "session/update".to_owned(),
        params: json!(AcpUpdateParams {
            session_id: session_id.to_owned(),
            update,
        }),
    })
}

fn mode_value(session: &AgentSession) -> &'static str {
    match session.permission_broker().map(|broker| broker.mode()) {
        Some(yi_runtime::PermissionMode::Yolo) => "yolo",
        Some(yi_runtime::PermissionMode::Auto) => "auto",
        _ => "ask",
    }
}

fn config_options(session: &AgentSession) -> Vec<AcpConfigOption> {
    let model = session.model();
    let models: Vec<Value> = available_models()
        .iter()
        .map(|candidate| {
            let value = format!("{}/{}", candidate.provider, candidate.id);
            json!({"value": value, "name": candidate.name})
        })
        .collect();
    let modes: Vec<Value> = ["ask", "auto", "yolo"]
        .iter()
        .map(|mode| json!({"value": mode, "name": mode}))
        .collect();
    let levels: Vec<Value> = model
        .supported_efforts()
        .iter()
        .map(|level| json!({"value": level.to_string(), "name": level.to_string()}))
        .collect();
    vec![
        AcpConfigOption {
            config_id: "model".to_owned(),
            name: "Model".to_owned(),
            kind: json!({
                "type": "select",
                "value": format!("{}/{}", model.provider, model.id),
                "options": models,
            }),
        },
        AcpConfigOption {
            config_id: "thought_level".to_owned(),
            name: "Thinking level".to_owned(),
            kind: json!({
                "type": "select",
                "value": session.effort().to_string(),
                "options": levels,
            }),
        },
        AcpConfigOption {
            config_id: "mode".to_owned(),
            name: "Permission mode".to_owned(),
            kind: json!({
                "type": "select",
                "value": mode_value(session),
                "options": modes,
            }),
        },
    ]
}

fn prompt_text(params: &Value) -> String {
    params
        .get("prompt")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| {
                    (block.get("type").and_then(Value::as_str) == Some("text"))
                        .then(|| block.get("text").and_then(Value::as_str))
                        .flatten()
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

impl AcpState {
    fn attach(&mut self, store: &SharedSession) -> Result<String, String> {
        let session_id = lock_session(store).metadata().id.clone();
        let asker = bridge_asker(
            session_id.clone(),
            Arc::clone(&self.sink),
            Arc::clone(&self.pending),
        );
        let (session, host) = (self.build)(Some(asker), Some(&session_id))?;
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
        let mut parent = Parent {
            forward: Forward {
                session_id: session_id.clone(),
                child: None,
                seq: Arc::new(AtomicU64::new(0)),
                sink: Arc::clone(&self.sink),
            },
            session: Arc::clone(&session),
            host: Arc::clone(&host),
            ids: IdMap::new(context_window),
            children: JoinSet::new(),
            seen: HashSet::new(),
            last_goal: Value::Null,
            last_workdir: Value::Null,
            launch_cwd: self.cwd.clone(),
        };
        parent.watch_workdir();
        let forwarder = tokio::spawn(forward_parent(events, parent));
        self.sessions.insert(
            session_id.clone(),
            SessionHandle {
                session,
                host,
                forwarder,
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
        self.sessions
            .get(id)
            .map(|handle| (handle, id.to_owned()))
            .ok_or((INVALID_PARAMS, format!("unknown session {id}")))
    }

    fn session_result(&self, session_id: &str) -> Value {
        let handle = self.sessions.get(session_id);
        let options = handle
            .map(|handle| config_options(&handle.session))
            .unwrap_or_default();
        let name = handle
            .and_then(|handle| handle.session.store())
            .and_then(|store| session_name(&store));
        json!(AcpSessionResult {
            session_id: session_id.to_owned(),
            config_options: options,
            name,
        })
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
                let mode = match value {
                    "ask" => yi_runtime::PermissionMode::Ask,
                    "auto" => yi_runtime::PermissionMode::Auto,
                    "yolo" => yi_runtime::PermissionMode::Yolo,
                    other => {
                        return Err((
                            INVALID_PARAMS,
                            format!("unknown mode {other}; use ask|auto|yolo"),
                        ));
                    }
                };
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
        let result = self.session_result(&id);
        let options = result.get("configOptions").cloned().unwrap_or(Value::Null);
        Ok(json!({"configOptions": options}))
    }

    fn handle(&mut self, method: &str, params: &Value) -> Result<Value, (i64, String)> {
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
                    capabilities: json!({}),
                    auth_methods: Vec::new(),
                }))
            }
            "session/new" => {
                let store = self
                    .repo
                    .create(CreateOptions::default())
                    .map_err(|error| (INTERNAL_ERROR, error.to_string()))?;
                let session_id = self
                    .attach(&store)
                    .map_err(|error| (INTERNAL_ERROR, error))?;
                Ok(self.session_result(&session_id))
            }
            "session/prompt" => {
                let (handle, _) = self.session(params)?;
                let text = prompt_text(params);
                let prompt = yi_runtime::session::user_input(&text);
                if handle.session.prompt_message(prompt.clone()).is_err() {
                    handle.session.follow_up_message(prompt);
                }
                Ok(json!({}))
            }
            "session/cancel" => {
                let (handle, _) = self.session(params)?;
                handle.session.abort();
                Ok(json!({}))
            }
            "session/list" => {
                let sessions: Vec<Value> = self
                    .repo
                    .list()
                    .map_err(|error| (INTERNAL_ERROR, error.to_string()))?
                    .iter()
                    .map(|metadata| {
                        json!({
                            "sessionId": metadata.id,
                            "attached": self.sessions.contains_key(&metadata.id),
                            "createdAt": metadata.created_at,
                            "name": metadata.name,
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
                if !self.sessions.contains_key(&id) {
                    let store = self
                        .repo
                        .open(&id)
                        .map_err(|error| (INVALID_PARAMS, error.to_string()))?;
                    self.attach(&store)
                        .map_err(|error| (INTERNAL_ERROR, error))?;
                }
                let mut result = self.session_result(&id);
                // v2 clients send `{"type": "start"}`; a bare number is the
                // entry-offset extension. Anything non-null replays.
                if let Some(replay_from) = params.get("replayFrom").filter(|v| !v.is_null()) {
                    let from = replay_from.as_u64().unwrap_or(0);
                    let replayed_to = self.replay(&id, from)?;
                    self.emit_replay(&id, from, None)?;
                    if let Some(map) = result.as_object_mut() {
                        map.insert("replayedTo".to_owned(), json!(replayed_to));
                    }
                }
                Ok(result)
            }
            "session/close" => {
                let id = self.session(params)?.1;
                self.user_cells.close(&id);
                self.sessions.remove(&id);
                Ok(json!({}))
            }
            "session/delete" => {
                let id = params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .ok_or((INVALID_PARAMS, "missing sessionId".to_owned()))?
                    .to_owned();
                self.sessions.remove(&id);
                self.repo
                    .delete(&id)
                    .map_err(|error| (INVALID_PARAMS, error.to_string()))?;
                Ok(json!({}))
            }
            "session/set_config_option" => self.set_config_option(params),
            "_yi/heartbeat" | "_yi/goal" | "_yi/tracked" | "_yi/kernel_execute"
            | "_yi/kernel_cancel" | "_yi/slash" => self.handle_extension(method, params),
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
                let rewound = yi_runtime::rewind_to(&handle.session, text("entryId"))
                    .map_err(|error| (INVALID_PARAMS, error))?;
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
            "_yi/slash" => {
                let line = text("line").trim();
                let (command, args) = line
                    .split_once(char::is_whitespace)
                    .map_or((line, ""), |(head, rest)| (head, rest.trim()));
                let before = (handle.session.model().id, handle.session.effort());
                let reply = match command {
                    "sessions" => self.sessions_text()?,
                    "undo" => undo_text(&handle.session, &self.cwd),
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
                let outcome = yi_runtime::schedule::parse_heartbeat_command(text("command"))
                    .and_then(|parsed| service.apply(&parsed, yi_runtime::session_store::now_ms()));
                match outcome {
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
                let outcome = match text("action") {
                    "get" => service.get(),
                    "create" => service.create(
                        text("objective"),
                        params.get("tokenBudget").and_then(Value::as_u64),
                        params
                            .get("check")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        params.get("checkTimeoutMs").and_then(Value::as_u64),
                    ),
                    // Blocks dispatch up to the check timeout — same class as
                    // the ACP permission bridge's synchronous wait.
                    "update" => service.update(text("status")),
                    "objective" => service.set_objective(
                        text("objective"),
                        params.get("citation").and_then(Value::as_str),
                    ),
                    other => Err(format!(
                        "unknown goal action {other}; use get|create|update|objective"
                    )),
                };
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
        if listed.is_empty() {
            return Ok("no sessions for this directory".to_owned());
        }
        let now = yi_runtime::session_store::now_ms();
        Ok(listed
            .iter()
            .map(|metadata| {
                format!(
                    "{}  {:>8}  {}",
                    metadata.id,
                    yi_runtime::session_store::age_label(now.saturating_sub(metadata.created_at)),
                    metadata.name.as_deref().unwrap_or("")
                )
            })
            .collect::<Vec<_>>()
            .join("\n"))
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
    }

    fn emit_replay_from(
        &self,
        session_id: &str,
        store: &SharedSession,
        from: u64,
        context_window: u64,
        child: Option<&ChildId>,
    ) -> Result<(), (i64, String)> {
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
        Ok(())
    }

    /// Design §17.2: the stored branch replayed as `session/update`s. `from` skips entries the
    /// client holds (valid only if it saw nothing since); returns the next `replayedTo`.
    fn replay(&mut self, session_id: &str, from: u64) -> Result<u64, (i64, String)> {
        let Some(handle) = self.sessions.get(session_id) else {
            return Ok(0);
        };
        let Some(store) = handle.session.store() else {
            return Ok(0);
        };
        let entries = lock_session(&store)
            .find_entries(&EntryQuery {
                order: EntryOrder::OldestFirst,
                ..EntryQuery::default()
            })
            .map_err(|error| (INTERNAL_ERROR, error.to_string()))?;
        let total = u64::try_from(entries.len()).unwrap_or(u64::MAX);
        let skip = usize::try_from(from.min(total)).unwrap_or(usize::MAX);
        let tail = entries.get(skip..).unwrap_or_default();
        let mut ids = IdMap::new(handle.session.model().context_window);
        for updated in replay_updates(tail, &mut ids) {
            (self.sink)(&update_notification(session_id, updated));
        }
        Ok(total)
    }
}

/// Invariant: the daemon fans this worker's asks out to every attached client, so a submit
/// carries no principal and is never confirmed here: an administrative op is `NoConfirmer`.
fn submit_plan(
    service: &yi_runtime::plan::PlanService,
    params: &Value,
) -> Result<Value, (i64, String)> {
    use yi_runtime::plan::authority::{submission_of, submit};
    let payload = params
        .as_object()
        .ok_or((INVALID_PARAMS, "params must be an object".to_owned()))?;
    if payload.contains_key("actor") {
        return Err((
            INVALID_PARAMS,
            yi_runtime::plan::tool::ArgError::ActorArg.to_string(),
        ));
    }
    let (engine, actor) = service
        .engine()
        .ok_or((INTERNAL_ERROR, "no plan engine is attached".to_owned()))?;
    let submission = submission_of(payload).map_err(|error| (INVALID_PARAMS, error))?;
    let applied = submit(&engine, &actor, None, submission)
        .map_err(|error| (INVALID_PARAMS, error.to_string()))?;
    Ok(json!({
        "plan": applied.outcome.plan.id.as_str(),
        "revision": applied.outcome.plan.touched.0,
        "text": applied.text(),
        "notices": applied.outcome.notices,
    }))
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
    let pending: PendingAsks = Arc::new(Mutex::new(HashMap::new()));
    let mut state = AcpState {
        user_cells: crate::cells::UserCells::default(),
        repo,
        sessions: HashMap::new(),
        build: options.build,
        sink: Arc::clone(&sink),
        pending: Arc::clone(&pending),
        agent_version: options.agent_version,
        initialized: false,
        cwd: options.cwd,
    };
    runtime.block_on(async move {
        let (line_tx, mut line_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
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
                    let sender = pending.lock().ok().and_then(|map| map.get(id).cloned());
                    if let Some(sender) = sender {
                        let _ = sender.send(value);
                        continue;
                    }
                }
                if line_tx.send(trimmed.to_owned()).is_err() {
                    break;
                }
            }
        });
        while let Some(line) = line_rx.recv().await {
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
                    let outcome = state.handle(&incoming.method, &params);
                    respond(&sink, id, outcome);
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
