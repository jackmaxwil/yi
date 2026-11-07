use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::process::Stdio;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::{Child, ChildStdin};
use tokio::sync::mpsc;
use yi_runtime::post::Waiting;
use yi_types::acp::{DaemonLedger, DaemonLedgerEntry};
use yi_types::mail::Envelope;

use crate::negotiate;

mod mail;
use mail::SUPERVISOR;

pub struct DaemonOptions {
    pub socket: PathBuf,
    /// Arguments forwarded to every worker after `acp` (model, session-dir,
    /// yolo — the serve invocation's own surface).
    pub worker_args: Vec<String>,
    pub agent_version: String,
    /// Where the `config.json` the settings page reads and saves lives.
    pub home: PathBuf,
    /// Where the sessions and their `channels/` mailboxes live; the daemon polls the mailboxes.
    pub session_dir: PathBuf,
}

pub(crate) type ClientId = u64;

const CLIENT_QUEUE: usize = 4096;

pub(crate) enum Input {
    Client(ClientId, mpsc::Sender<String>),
    ClientLine(ClientId, Value),
    ClientClosed(ClientId),
    WorkerLine(String, u64, Value, String),
    WorkerReply(String, Value),
    WorkerClosed(String, u64),
    SwapTimeout(String, u64, crate::swap::Stage),
    Mail(Vec<Waiting>),
    Answer(ClientId, String),
    Shutdown,
}

/// Asks parked for a detached session; past this the worker is refused
/// rather than left waiting.
const PARKED_MAX: usize = 8;

struct ClientRequest {
    client: ClientId,
    original: Value,
    root: String,
    resumed: Option<String>,
}

pub(crate) struct Worker {
    pub(crate) child: Child,
    pub(crate) stdin: ChildStdin,
    pub(crate) generation: u64,
}

/// Per-session routing plus the unseen ledger (in-memory, survives detach not restart).
/// Attachment is a set, so one session can be watched from any number of terminals.
pub(crate) struct SessionEntry {
    pub(crate) root: String,
    pub(crate) attached: std::collections::HashSet<ClientId>,
    unseen: u64,
    last_state: Option<String>,
    last_event_ms: u64,
    name: Option<String>,
    pinned_ms: Option<u64>,
    /// Invariant: true only for a row the pre-attach path invented so a resume's replay
    /// could route. The worker's result confirms it; an unconfirmed row dies with its client.
    provisional: bool,
    /// Invariant: true only while the daemon started this session for a mailed request and no
    /// client has attached since, so its asks are refused instead of parked for nobody.
    headless: bool,
    owed: Vec<Envelope>,
    /// Invariant: true only while a live worker holds the session, never for a ledger row.
    loaded: bool,
}

impl SessionEntry {
    fn provisional(root: String) -> Self {
        Self {
            provisional: true,
            ..Self::new(root, None)
        }
    }

    fn new(root: String, attached: Option<ClientId>) -> Self {
        Self {
            root,
            attached: attached.into_iter().collect(),
            unseen: 0,
            last_state: None,
            last_event_ms: 0,
            name: None,
            pinned_ms: None,
            provisional: false,
            headless: false,
            owed: Vec::new(),
            loaded: false,
        }
    }

    /// Invariant: no worker survives the daemon, so a stored `running` or
    /// `requires_action` is a turn that died; it reloads as idle.
    fn from_stored(stored: DaemonLedgerEntry) -> Self {
        Self {
            last_state: stored.last_state.map(|_| "idle".to_owned()),
            unseen: stored.unseen,
            last_event_ms: stored.last_event_ms,
            name: stored.name,
            pinned_ms: stored.pinned_ms,
            ..Self::new(stored.cwd, None)
        }
    }

    fn to_stored(&self) -> DaemonLedgerEntry {
        DaemonLedgerEntry {
            cwd: self.root.clone(),
            unseen: self.unseen,
            last_state: self.last_state.clone(),
            last_event_ms: self.last_event_ms,
            name: self.name.clone(),
            pinned_ms: self.pinned_ms,
            extra: BTreeMap::new(),
        }
    }
}

fn ledger_path(socket: &std::path::Path) -> PathBuf {
    socket.with_extension("ledger.json")
}

/// Invariant: the ledger is written whole through a rename (D118), so a reader sees one
/// version or none; the read is bounded because the file is a peer's to grow.
const LEDGER_MAX_BYTES: u64 = 1 << 20;

pub fn read_ledger(socket: &std::path::Path) -> Option<DaemonLedger> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(ledger_path(socket))
        .and_then(|file| file.take(LEDGER_MAX_BYTES).read_to_string(&mut text))
        .ok()?;
    serde_json::from_str::<DaemonLedger>(&text).ok()
}

fn spawn_ledger_writer(
    path: PathBuf,
) -> (
    std::sync::mpsc::Sender<DaemonLedger>,
    std::thread::JoinHandle<()>,
) {
    let (sender, snapshots) = std::sync::mpsc::channel::<DaemonLedger>();
    let writer = std::thread::spawn(move || {
        let tmp = path.with_extension("json.tmp");
        while let Ok(mut ledger) = snapshots.recv() {
            while let Ok(newer) = snapshots.try_recv() {
                ledger = newer;
            }
            if let Ok(text) = serde_json::to_string(&ledger)
                && std::fs::write(&tmp, text).is_ok()
            {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    });
    (sender, writer)
}

fn load_ledger(socket: &std::path::Path) -> HashMap<String, SessionEntry> {
    read_ledger(socket)
        .map(|ledger| {
            ledger
                .sessions
                .into_iter()
                .map(|(id, entry)| (id, SessionEntry::from_stored(entry)))
                .collect()
        })
        .unwrap_or_default()
}

fn prompt_title(frame: &Value) -> Option<String> {
    let text = frame
        .pointer("/params/prompt")?
        .as_array()?
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join(" ");
    yi_runtime::session_store::session_title(&text)
}

#[expect(
    clippy::disallowed_methods,
    reason = "the ledger's timestamp is the job: when the last unattended update landed"
)]
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Routes ACP v2 between clients (unix socket) and one worker per root (`yi acp` over
/// stdio). Workers outlive client connections, so schedulers fire with nobody attached.
pub(crate) struct Supervisor {
    pub(crate) options: DaemonOptions,
    ledger: std::sync::mpsc::Sender<DaemonLedger>,
    pub(crate) workers: HashMap<String, Worker>,
    clients: HashMap<ClientId, mpsc::Sender<String>>,
    pub(crate) sessions: HashMap<String, SessionEntry>,
    woken: HashSet<(String, String)>,
    requests: HashMap<u64, ClientRequest>,
    worker_requests: HashMap<String, String>,
    /// Worker-originated requests that arrived with no attached client, flushed on the next
    /// attach; without this the worker's synchronous asker blocks on an unseen answer.
    parked: HashMap<String, Vec<Value>>,
    next_request: u64,
    pub(crate) swap: crate::swap::Swaps,
    pub(crate) input: mpsc::UnboundedSender<Input>,
    settings_tail: Option<tokio::task::JoinHandle<()>>,
}
pub(crate) fn error_frame(id: Value, code: i64, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

pub(crate) fn result_frame(id: Value, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

impl Supervisor {
    /// A client that cannot keep up is dropped, never queued without bound; it heals on reconnect.
    pub(crate) fn send_client(&self, client: ClientId, line: String) {
        let Some(sink) = self.clients.get(&client) else {
            return;
        };
        if let Err(mpsc::error::TrySendError::Full(_)) = sink.try_send(line) {
            let _ = self.input.send(Input::ClientClosed(client));
        }
    }

    fn persist(&self) {
        let ledger = DaemonLedger {
            sessions: self
                .sessions
                .iter()
                .filter(|(_, entry)| !entry.provisional)
                .map(|(id, entry)| (id.clone(), entry.to_stored()))
                .collect(),
        };
        let _ = self.ledger.send(ledger);
    }

    fn mark_seen(&mut self, client: ClientId, id: Option<Value>, frame: &Value) {
        let Some(id) = id else { return };
        let session_id = frame
            .pointer("/params/sessionId")
            .and_then(Value::as_str)
            .unwrap_or("");
        match self.sessions.get_mut(session_id) {
            Some(entry) => {
                entry.unseen = 0;
                self.persist();
                self.send_client(client, result_frame(id, json!({})));
            }
            None => self.send_client(client, error_frame(id, -32602, "unknown sessionId")),
        }
    }

    /// A session only the worker listed has no entry yet, so a pin's `cwd` makes one.
    fn pin(&mut self, client: ClientId, id: Option<Value>, frame: &Value) {
        let Some(id) = id else { return };
        let param = |key: &str| frame.pointer(&format!("/params/{key}"));
        let session_id = param("sessionId").and_then(Value::as_str);
        let (Some(session_id), Some(pinned)) =
            (session_id, param("pinned").and_then(Value::as_bool))
        else {
            let refused = error_frame(id, -32602, "_yi/pin takes a sessionId and a bool pinned");
            return self.send_client(client, refused);
        };
        if pinned && let Some(cwd) = param("cwd").and_then(Value::as_str) {
            self.sessions
                .entry(session_id.to_owned())
                .or_insert_with(|| SessionEntry::new(cwd.to_owned(), None));
        }
        let Some(entry) = self.sessions.get_mut(session_id) else {
            return self.send_client(client, error_frame(id, -32602, "unknown sessionId"));
        };
        entry.pinned_ms = pinned.then(|| entry.pinned_ms.unwrap_or_else(now_ms));
        let reply = json!({"pinnedMs": entry.pinned_ms});
        self.persist();
        self.send_client(client, result_frame(id, reply));
    }

    async fn worker_for(&mut self, root: &str) -> Result<(), String> {
        if let Some(refused) = self.swap_refusal(root) {
            return Err(refused);
        }
        if self.workers.contains_key(root) {
            return Ok(());
        }
        self.spawn_worker(root)
    }

    async fn forward_to_worker(
        &mut self,
        root: &str,
        client: ClientId,
        id: Option<Value>,
        mut frame: Value,
    ) {
        if let Some(refused) = self.swap_refusal(root) {
            if let Some(original) = id {
                self.send_client(client, error_frame(original, crate::swap::BUSY, &refused));
            }
            return;
        }
        if let Some(original) = id {
            self.next_request = self.next_request.wrapping_add(1);
            let rewritten = self.next_request;
            let resumed = (frame["method"] == "session/resume")
                .then(|| frame.pointer("/params/sessionId").and_then(Value::as_str))
                .flatten()
                .map(str::to_owned);
            let request = ClientRequest {
                client,
                original,
                root: root.to_owned(),
                resumed,
            };
            self.requests.insert(rewritten, request);
            if let Some(map) = frame.as_object_mut() {
                map.insert("id".to_owned(), json!(format!("sup_{rewritten}")));
            }
        }
        if !self.write_worker(root, frame.to_string()).await {
            self.handle_worker_closed(root.to_owned());
        }
    }

    async fn handle_client_line(&mut self, client: ClientId, frame: Value) {
        let id = frame.get("id").cloned();
        let method = frame
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        // A frame with an id and no method is a response to a
        // worker-originated request (the permission bridge).
        if method.is_empty() {
            if let Some(request_id) = id.as_ref().and_then(Value::as_str)
                && let Some(root) = self.worker_requests.remove(request_id)
            {
                self.forward_response_to_worker(&root, frame).await;
            }
            return;
        }
        match method.as_str() {
            "initialize" => {
                let Some(id) = id else { return };
                let requested = frame
                    .pointer("/params/protocolVersion")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let line = match negotiate(requested) {
                    Ok(agreed) => result_frame(
                        id,
                        json!({
                            "protocolVersion": agreed,
                            "info": {"name": "yi", "version": self.options.agent_version},
                            "capabilities": {"session": {"delete": {}}},
                            "authMethods": [],
                        }),
                    ),
                    Err(message) => error_frame(id, -32602, &message),
                };
                self.send_client(client, line);
            }
            "session/new" | "session/resume" => {
                let root = frame
                    .pointer("/params/cwd")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| self.root_of(&frame));
                let Some(root) = root else {
                    if let Some(id) = id {
                        self.send_client(
                            client,
                            error_frame(id, -32602, "missing cwd and unknown sessionId"),
                        );
                    }
                    return;
                };
                if let Err(error) = self.worker_for(&root).await {
                    if let Some(id) = id {
                        self.send_client(client, error_frame(id, -32603, &error));
                    }
                    return;
                }
                // Attach BEFORE forwarding: a resume's replay notifications stream ahead of
                // its response and must route here, not into the unseen ledger.
                if let Some(session_id) = frame
                    .pointer("/params/sessionId")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                {
                    self.sessions
                        .entry(session_id.clone())
                        .or_insert_with(|| SessionEntry::provisional(root.clone()))
                        .attached
                        .insert(client);
                    self.flush_parked(&session_id, client);
                }
                self.forward_to_worker(&root, client, id, frame).await;
            }
            "session/list" => {
                // With a cwd the list is the worker's persisted repo; without
                // one it is the daemon's live map plus the unseen ledger.
                if let Some(root) = frame
                    .pointer("/params/cwd")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                {
                    if let Err(error) = self.worker_for(&root).await {
                        if let Some(id) = id {
                            self.send_client(client, error_frame(id, -32603, &error));
                        }
                        return;
                    }
                    self.forward_to_worker(&root, client, id, frame).await;
                    return;
                }
                let Some(id) = id else { return };
                let sessions: Vec<Value> = self
                    .sessions
                    .iter()
                    .filter(|(_, entry)| !entry.provisional)
                    .map(|(session_id, entry)| {
                        json!({
                            "sessionId": session_id,
                            "cwd": entry.root,
                            "title": entry.name,
                            "_meta": {"yi": {
                                "attached": !entry.attached.is_empty(),
                                "unseen": entry.unseen,
                                "lastState": entry.last_state,
                                "lastEventMs": entry.last_event_ms,
                                "pinnedMs": entry.pinned_ms,
                            }},
                        })
                    })
                    .collect();
                self.send_client(client, result_frame(id, json!({"sessions": sessions})));
            }
            "_yi/shutdown" => {
                if let Some(id) = id {
                    self.send_client(client, result_frame(id, json!({})));
                }
                let _ = self.input.send(Input::Shutdown);
            }
            "_yi/seen" => self.mark_seen(client, id, &frame),
            "_yi/pin" => self.pin(client, id, &frame),
            "_yi/settings" | "_yi/set_default" => self.settings(client, id, &method, &frame),
            "_yi/open" => self.open(client, id, &frame),
            "_yi/worker_replace" => self.replace(client, id, &frame).await,
            _ => {
                let Some(root) = self.root_of(&frame) else {
                    if let Some(id) = id {
                        self.send_client(client, error_frame(id, -32602, "unknown sessionId"));
                    }
                    return;
                };
                if let Some(session_id) = frame
                    .pointer("/params/sessionId")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    && let Some(entry) = self.sessions.get_mut(&session_id)
                {
                    entry.attached.insert(client);
                    if entry.name.is_none() && method == "session/prompt" {
                        entry.name = prompt_title(&frame);
                        self.persist();
                    }
                    self.flush_parked(&session_id, client);
                }
                self.forward_to_worker(&root, client, id, frame).await;
            }
        }
    }

    fn root_of(&self, frame: &Value) -> Option<String> {
        let session_id = frame.pointer("/params/sessionId").and_then(Value::as_str)?;
        self.sessions
            .get(session_id)
            .map(|entry| entry.root.clone())
    }

    async fn forward_response_to_worker(&mut self, root: &str, frame: Value) {
        if self.swap_refusal(root).is_some() {
            return;
        }
        if !self.write_worker(root, frame.to_string()).await {
            self.handle_worker_closed(root.to_owned());
        }
    }

    fn handle_worker_line(&mut self, root: String, mut frame: Value, line: String) {
        let method = frame
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if method.is_empty() {
            let Some(rewritten) = frame
                .get("id")
                .and_then(Value::as_str)
                .and_then(|id| id.strip_prefix("sup_"))
                .and_then(|id| id.parse::<u64>().ok())
            else {
                return;
            };
            let Some(ClientRequest {
                client,
                original,
                root: request_root,
                resumed,
            }) = self.requests.remove(&rewritten)
            else {
                return;
            };
            let answered = frame.get("result").is_some();
            let viewer = (client != SUPERVISOR).then_some(client);
            if client == SUPERVISOR && !answered {
                return self.forget_failed_wake(resumed, &frame);
            }
            if let Some(session_id) = frame
                .pointer("/result/sessionId")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or(resumed.filter(|_| answered))
            {
                let name = frame
                    .pointer("/result/_meta/yi/name")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                match self.sessions.get_mut(&session_id) {
                    Some(entry) => {
                        entry.root = request_root;
                        entry.attached.extend(viewer);
                        entry.provisional = false;
                        entry.loaded = true;
                        entry.name = name.or(entry.name.take());
                    }
                    None => {
                        let mut entry = SessionEntry::new(request_root, viewer);
                        (entry.name, entry.loaded) = (name, true);
                        self.sessions.insert(session_id.clone(), entry);
                    }
                }
                self.persist();
                // Anything the worker said between attaching and answering waited for this.
                if client != SUPERVISOR {
                    self.flush_parked(&session_id, client);
                }
            }
            if let Some(map) = frame.as_object_mut() {
                map.insert("id".to_owned(), original);
            }
            self.send_client(client, frame.to_string());
            return;
        }
        // Worker-originated request (permission bridge): remember the id so
        // the client's response routes back.
        if let Some(id) = frame.get("id").and_then(Value::as_str) {
            self.worker_requests.insert(id.to_owned(), root.clone());
        }
        // With no client attached the frame is dropped but the ledger records that something
        // happened, so a reattaching console still sees the unseen work.
        let session_id = frame
            .pointer("/params/sessionId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let Some(session_id) = session_id else { return };
        let state = frame
            .pointer("/params/update")
            .filter(|update| {
                update.get("sessionUpdate").and_then(Value::as_str) == Some("state_update")
            })
            .and_then(|update| update.get("state"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let Some(entry) = self.sessions.get_mut(&session_id) else {
            // A worker speaks at attach, before its `session/new` response makes this entry.
            let queue = self.parked.entry(session_id).or_default();
            if queue.len() < PARKED_MAX {
                queue.push(frame);
            }
            return;
        };
        let transition = state.is_some();
        let owed = mail::owed_by_ask(entry, &method, &frame, state.as_deref());
        if let Some(state) = state {
            entry.last_event_ms = now_ms();
            entry.last_state = Some(state);
        }
        let named = frame
            .pointer("/params/update")
            .filter(|update| {
                update.get("sessionUpdate").and_then(Value::as_str) == Some("_yi/name")
            })
            .and_then(|update| update.get("name"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let renamed = named.is_some();
        if named.is_some() {
            entry.name = named;
        }
        let detached = entry.attached.is_empty();
        if transition && detached {
            entry.unseen = entry.unseen.saturating_add(1);
        }
        let watchers: Vec<ClientId> = entry.attached.iter().copied().collect();
        if transition || renamed {
            self.persist();
        }
        if let Some(owed) = owed {
            return self.refuse_ask(root, &session_id, &frame, &owed);
        }
        if detached {
            // Requests (frames with an id) park for the next attach; a
            // capped queue bounds a worker that asks in a loop.
            if let Some(request_id) = frame.get("id").cloned() {
                let queue = self.parked.entry(session_id).or_default();
                if queue.len() < PARKED_MAX {
                    queue.push(frame);
                } else {
                    // Incident: past the cap the ask was dropped, leaving the
                    // worker's synchronous asker on an answer nobody would send.
                    let _ = self.input.send(Input::WorkerReply(
                        root,
                        json!({
                            "jsonrpc": "2.0",
                            "id": request_id,
                            "error": {"code": -32603, "message": "no client attached; ask queue full"},
                        }),
                    ));
                }
            }
            return;
        }
        // Fan out to every watcher; for a worker-originated request the first answer wins
        // and the stale ids of the rest fall out of `worker_requests` with it.
        for client in watchers {
            self.send_client(client, line.clone());
        }
    }

    fn flush_parked(&mut self, session_id: &str, client: ClientId) {
        if let Some(entry) = self.sessions.get_mut(session_id) {
            (entry.headless, entry.owed) = (false, Vec::new());
        }
        if let Some(queue) = self.parked.remove(session_id) {
            for frame in queue {
                self.send_client(client, frame.to_string());
            }
        }
    }

    fn handle_worker_closed(&mut self, root: String) {
        if let Some(mut worker) = self.workers.remove(&root) {
            let _reap = worker.child.start_kill();
        }
        let dead: Vec<u64> = self
            .requests
            .iter()
            .filter(|(_, request)| request.root == root)
            .map(|(id, _)| *id)
            .collect();
        for id in dead {
            if let Some(request) = self.requests.remove(&id) {
                let exited = error_frame(request.original, -32603, "worker exited");
                self.send_client(request.client, exited);
            }
        }
        self.tell_exited(&root);
        // Invariant: a pin outlives its worker, or the next persist writes the ledger without it.
        self.sessions
            .retain(|_, entry| entry.root != root || entry.pinned_ms.is_some());
        for entry in self.sessions.values_mut().filter(|e| e.root == root) {
            entry.attached.clear();
            entry.loaded = false;
            entry.last_state = entry.last_state.take().map(|_| "idle".to_owned());
        }
        self.parked
            .retain(|id, _| self.sessions.get(id).is_some_and(|e| e.root != root));
        self.abandon_swap(&root, "the worker exited");
    }

    /// The daemon answers for the config, not a worker: a worker holds the copy it started on.
    /// The read and fsync run off the routing loop, which every client and worker waits on.
    fn settings(&mut self, client: ClientId, id: Option<Value>, method: &str, frame: &Value) {
        let Some(id) = id else { return };
        let home = self.options.home.clone();
        let save = method == "_yi/set_default";
        let setting = frame
            .pointer("/params/id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let value = frame.pointer("/params/value").cloned();
        let input = self.input.clone();
        // Incident: saves ran side by side, so a press could overwrite the newer one it raced.
        let before = self.settings_tail.take();
        let task = tokio::spawn(async move {
            if let Some(before) = before {
                let _ = before.await;
            }
            let _ = tokio::task::spawn_blocking(move || {
                let answer = match (save, setting, value) {
                    (false, _, _) => yi_runtime::settings::current(&home),
                    (true, Some(setting), Some(value)) => {
                        yi_runtime::settings::save(&home, &setting, value)
                    }
                    (true, _, _) => Err("a save names an id and a value; null unsets".to_owned()),
                };
                let line = match answer {
                    Ok(rows) => result_frame(id, json!({"rows": rows})),
                    Err(why) => error_frame(id, -32602, &why),
                };
                let _ = input.send(Input::Answer(client, line));
            })
            .await;
        });
        self.settings_tail = Some(task);
    }

    /// A message opened in an app the person listed: the daemon runs it, since the console never
    /// spawns, and only an app named in config.json can run.
    fn open(&mut self, client: ClientId, id: Option<Value>, frame: &Value) {
        let Some(id) = id else { return };
        let home = self.options.home.clone();
        let name = frame
            .pointer("/params/name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let text = frame
            .pointer("/params/text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let input = self.input.clone();
        tokio::task::spawn_blocking(move || {
            let opened = yi_runtime::settings::open(&home, &name, &text).and_then(|argv| {
                spawn_app(&argv).map_err(|error| format!("{name}: {error}"))?;
                Ok(argv)
            });
            let line = match opened {
                Ok(argv) => result_frame(id, json!({"argv": argv})),
                Err(why) => error_frame(id, -32602, &why),
            };
            let _ = input.send(Input::Answer(client, line));
        });
    }

    fn handle_client_closed(&mut self, client: ClientId) {
        self.clients.remove(&client);
        for entry in self.sessions.values_mut() {
            entry.attached.remove(&client);
        }
        self.sessions
            .retain(|_, entry| !entry.provisional || !entry.attached.is_empty());
        self.requests.retain(|_, request| request.client != client);
    }
}

fn spawn_client(stream: UnixStream, client: ClientId, input: mpsc::UnboundedSender<Input>) {
    let (read, mut write) = stream.into_split();
    let (sink, mut outgoing) = mpsc::channel::<String>(CLIENT_QUEUE);
    if input.send(Input::Client(client, sink)).is_err() {
        return;
    }
    tokio::spawn(async move {
        while let Some(line) = outgoing.recv().await {
            let mut payload = line.into_bytes();
            payload.push(b'\n');
            if write.write_all(&payload).await.is_err() {
                break;
            }
        }
    });
    tokio::spawn(async move {
        let mut lines = BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(&line) {
                Ok(value) => {
                    if input.send(Input::ClientLine(client, value)).is_err() {
                        break;
                    }
                }
                Err(_) => continue,
            }
        }
        let _ = input.send(Input::ClientClosed(client));
    });
}

#[expect(
    clippy::disallowed_methods,
    reason = "an Open in app is one the person named in config.json; nothing else can be run"
)]
fn spawn_app(argv: &[String]) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    let (program, args) = argv.split_first().ok_or(std::io::ErrorKind::InvalidInput)?;
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    // Invariant: a dropped Child is never reaped, so each launch would leave a zombie in the
    // long-lived daemon; a thread waits it out without holding the reply.
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[expect(
    clippy::disallowed_methods,
    reason = "the daemon is the one place the CLI forks itself; the console only connects"
)]
pub fn spawn_detached(socket: &std::path::Path, worker_args: &[String]) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe()?;
    std::process::Command::new(exe)
        .arg("serve")
        .arg("--socket")
        .arg(socket)
        .args(worker_args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map(drop)
}

pub fn run_daemon(options: DaemonOptions, runtime: tokio::runtime::Runtime) -> i32 {
    runtime.block_on(async move {
        if std::os::unix::net::UnixStream::connect(&options.socket).is_ok() {
            eprintln!("daemon already running at {}", options.socket.display());
            return 0;
        }
        let _stale = std::fs::remove_file(&options.socket);
        let listener = match UnixListener::bind(&options.socket) {
            Ok(listener) => listener,
            Err(error) => {
                eprintln!("error: cannot bind {}: {error}", options.socket.display());
                return 1;
            }
        };
        // The socket is a local trust boundary: owner-only.
        let _perms = std::fs::set_permissions(
            &options.socket,
            std::os::unix::fs::PermissionsExt::from_mode(0o600),
        );
        let (input_tx, mut input_rx) = mpsc::unbounded_channel::<Input>();
        let (ledger, ledger_writer) = spawn_ledger_writer(ledger_path(&options.socket));
        let mut supervisor = Supervisor {
            sessions: load_ledger(&options.socket),
            woken: HashSet::new(),
            options,
            ledger,
            workers: HashMap::new(),
            clients: HashMap::new(),
            requests: HashMap::new(),
            worker_requests: HashMap::new(),
            parked: HashMap::new(),
            settings_tail: None,
            next_request: 0,
            swap: crate::swap::Swaps::default(),
            input: input_tx.clone(),
        };
        mail::poll(&supervisor.options.session_dir, input_tx.clone());
        let accept_input = input_tx.clone();
        tokio::spawn(async move {
            let mut next_client: ClientId = 0;
            loop {
                let Ok((stream, _addr)) = listener.accept().await else {
                    break;
                };
                next_client = next_client.wrapping_add(1);
                spawn_client(stream, next_client, accept_input.clone());
            }
        });
        while let Some(event) = input_rx.recv().await {
            match event {
                Input::Client(client, sink) => {
                    supervisor.clients.insert(client, sink);
                }
                Input::ClientLine(client, frame) => {
                    let _span = yi_types::trace::span(format!(
                        "daemon client {}",
                        frame
                            .get("method")
                            .and_then(Value::as_str)
                            .unwrap_or("response")
                    ));
                    supervisor.handle_client_line(client, frame).await;
                }
                Input::ClientClosed(client) => supervisor.handle_client_closed(client),
                Input::WorkerLine(root, generation, frame, line) => {
                    if !supervisor.serves(&root, generation) {
                        continue;
                    }
                    let _span = yi_types::trace::span(format!(
                        "daemon worker {}",
                        frame
                            .get("method")
                            .and_then(Value::as_str)
                            .unwrap_or("response")
                    ));
                    if crate::swap::is_reply(&frame) {
                        supervisor.swap_reply(&root, &frame).await;
                    } else {
                        supervisor.handle_worker_line(root, frame, line);
                    }
                }
                Input::WorkerReply(root, frame) => {
                    supervisor.forward_response_to_worker(&root, frame).await;
                }
                Input::WorkerClosed(root, generation) => {
                    if supervisor.swap_closing(&root, generation) {
                        supervisor.swap_exited(&root).await;
                    } else if supervisor.serves(&root, generation) {
                        supervisor.handle_worker_closed(root);
                    }
                }
                Input::SwapTimeout(root, id, stage) => supervisor.swap_timeout(&root, id, stage),
                Input::Mail(waiting) => supervisor.wake(waiting).await,
                Input::Answer(client, line) => supervisor.send_client(client, line),
                Input::Shutdown => break,
            }
        }
        for worker in supervisor.workers.values_mut() {
            let _ = worker.child.kill().await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let socket = supervisor.options.socket.clone();
        drop(supervisor);
        let _flushed = ledger_writer.join();
        let _ = std::fs::remove_file(&socket);
        0
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    use yi_runtime::post::Waiting;

    #[test]
    fn the_ledger_writer_lands_the_newest_snapshot_and_flushes_on_close() -> Result<(), String> {
        let dir = std::env::temp_dir().join(format!("yi-ledger-writer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        let path = dir.join("daemon.ledger.json");
        let (ledger, writer) = spawn_ledger_writer(path.clone());
        for unseen in 0..200 {
            let entry = DaemonLedgerEntry {
                cwd: "/repo".to_owned(),
                unseen,
                last_state: None,
                last_event_ms: 0,
                name: None,
                pinned_ms: None,
                extra: BTreeMap::new(),
            };
            let sessions = std::iter::once(("s1".to_owned(), entry)).collect();
            ledger
                .send(DaemonLedger { sessions })
                .map_err(|error| error.to_string())?;
        }
        drop(ledger);
        writer.join().map_err(|_| "writer panicked".to_owned())?;
        let text = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
        let stored: DaemonLedger =
            serde_json::from_str(&text).map_err(|error| error.to_string())?;
        assert_eq!(
            stored.sessions.get("s1").map(|entry| entry.unseen),
            Some(199)
        );
        assert!(
            !path.with_extension("json.tmp").exists(),
            "the temp file was renamed"
        );
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    /// A supervisor with no worker, its pinned and unpinned `/repo` rows mid-turn for client 7.
    fn supervisor_with_rows() -> Supervisor {
        let (ledger, _stored) = std::sync::mpsc::channel();
        let (input, _inputs) = mpsc::unbounded_channel();
        let mut supervisor = Supervisor {
            options: DaemonOptions {
                socket: PathBuf::new(),
                worker_args: Vec::new(),
                agent_version: String::new(),
                home: PathBuf::new(),
                session_dir: PathBuf::new(),
            },
            ledger,
            workers: HashMap::new(),
            clients: HashMap::new(),
            sessions: HashMap::new(),
            woken: HashSet::new(),
            requests: HashMap::new(),
            worker_requests: HashMap::new(),
            parked: HashMap::new(),
            settings_tail: None,
            next_request: 0,
            swap: crate::swap::Swaps::default(),
            input,
        };
        for (id, pinned_ms) in [("pinned", Some(1)), ("loose", None)] {
            let mut entry = SessionEntry::new("/repo".to_owned(), Some(7));
            (entry.last_state, entry.pinned_ms) = (Some("running".to_owned()), pinned_ms);
            supervisor.sessions.insert(id.to_owned(), entry);
        }
        supervisor
    }
    async fn failure_letter(channels: &std::path::Path, request: &str) -> Result<Value, String> {
        let mailbox = yi_runtime::schedule::channel::Channel::at(channels.join("mail-asker.jsonl"));
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Some(letter) = mailbox
                    .entries()
                    .into_iter()
                    .map(|entry| entry.data)
                    .find(|data| data["inReplyTo"] == request)
                {
                    return letter;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| format!("no failure letter answered {request}"))
    }

    /// Dies with a dead worker's pinned row still attached or still running, which lists a
    /// session nothing serves as Working forever; the root's unpinned row goes with the worker.
    #[test]
    fn a_dead_workers_pinned_row_stays_listed_idle_and_unattached() -> Result<(), String> {
        let mut supervisor = supervisor_with_rows();
        supervisor.handle_worker_closed("/repo".to_owned());
        assert!(!supervisor.sessions.contains_key("loose"));
        let row = supervisor
            .sessions
            .get("pinned")
            .ok_or("the pinned row goes")?;
        assert!(row.attached.is_empty(), "{:?}", row.attached);
        assert_eq!(row.last_state.as_deref(), Some("idle"));
        Ok(())
    }

    /// Dies with a pin whose `pinned` is missing or not a bool read as an unpin.
    #[test]
    fn a_pin_without_a_bool_is_refused_and_keeps_the_pin() -> Result<(), String> {
        let mut supervisor = supervisor_with_rows();
        let (sink, mut replies) = mpsc::channel(4);
        supervisor.clients.insert(7, sink);
        for pinned in [json!(null), json!("false")] {
            let frame = json!({"params": {"sessionId": "pinned", "pinned": pinned}});
            supervisor.pin(7, Some(json!(1)), &frame);
            let reply = replies.try_recv().map_err(|error| error.to_string())?;
            assert!(reply.contains("-32602"), "{reply}");
        }
        let row = supervisor
            .sessions
            .get("pinned")
            .ok_or("the pinned row goes")?;
        assert_eq!(row.pinned_ms, Some(1));
        Ok(())
    }

    /// Dies with a misspelled root (`--cwd .`, a trailing slash) answered as a successful swap
    /// while the worker that serves the console keeps the old build.
    #[tokio::test]
    async fn replacing_a_root_no_worker_serves_refuses_and_spawns_nothing() -> Result<(), String> {
        let mut supervisor = supervisor_with_rows();
        let (sink, mut replies) = mpsc::channel(4);
        supervisor.clients.insert(7, sink);
        let binary = std::env::current_exe().map_err(|error| error.to_string())?;
        let frame = json!({"params": {"cwd": "/repo/", "binary": binary.display().to_string()}});
        supervisor.replace(7, Some(json!(1)), &frame).await;
        let reply = replies.try_recv().map_err(|error| error.to_string())?;
        assert!(reply.contains("no worker serves /repo/"), "{reply}");
        assert!(supervisor.workers.is_empty(), "a worker was spawned");
        assert!(
            supervisor.swap.binaries.is_empty(),
            "the binary was adopted"
        );
        Ok(())
    }

    /// Dies with a session the daemon started for a mailed request parking its ask for a client
    /// that never comes, or with a ledger row skipped as held because a sibling in its repo is.
    #[tokio::test]
    async fn an_ask_from_a_mail_started_session_is_refused_and_the_asker_told() -> Result<(), String>
    {
        let dir = std::env::temp_dir().join(format!("yi-mail-ask-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let channels = dir.join("channels");
        std::fs::create_dir_all(&channels).map_err(|error| error.to_string())?;
        std::fs::create_dir_all(dir.join("repo")).map_err(|error| error.to_string())?;
        let header = r#"{"kind":"header","version":4,"id":"sleeper","createdAt":1,"cwd":"/repo"}"#;
        std::fs::write(dir.join("repo/1_sleeper.jsonl"), format!("{header}\n"))
            .map_err(|error| error.to_string())?;
        let mut supervisor = supervisor_with_rows();
        let (input, mut inputs) = mpsc::unbounded_channel();
        (supervisor.input, supervisor.options.session_dir) = (input, dir.clone());
        let request: Envelope = serde_json::from_value(json!({
            "id": "asker-1", "from": "history://asker", "to": "history://sleeper",
            "kind": "request", "conversation": "asker-1", "seq": 1, "sentAt": 1, "body": "tidy",
        }))
        .map_err(|error| error.to_string())?;
        let mut stub = tokio::process::Command::new("cat");
        stub.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .kill_on_drop(true);
        let mut child = stub.spawn().map_err(|error| error.to_string())?;
        let stdin = child.stdin.take().ok_or("the stub worker has no stdin")?;
        supervisor.workers.insert(
            "/repo".to_owned(),
            Worker {
                child,
                stdin,
                generation: 0,
            },
        );
        let mut sibling = SessionEntry::new("/repo".to_owned(), None);
        sibling.loaded = true;
        supervisor.sessions.insert("sibling".to_owned(), sibling);
        let ledger_row = SessionEntry::new("/repo".to_owned(), None);
        supervisor.sessions.insert("sleeper".to_owned(), ledger_row);
        let session = "sleeper".to_owned();
        supervisor
            .wake(vec![Waiting {
                session,
                request,
                root: Some(PathBuf::from("/repo")),
            }])
            .await;
        let resumed = supervisor
            .requests
            .values()
            .any(|r| r.resumed.as_deref() == Some("sleeper"));
        assert!(resumed, "the stopped session was not resumed");
        let ask = json!({"jsonrpc": "2.0", "id": "perm_1", "method": "session/request_permission",
            "params": {"sessionId": "sleeper", "title": "Run rm -rf build"}});
        supervisor.handle_worker_line("/repo".to_owned(), ask.clone(), ask.to_string());
        let waited = tokio::time::timeout(std::time::Duration::from_secs(5), inputs.recv());
        let Ok(Some(Input::WorkerReply(_, reply))) = waited.await else {
            return Err("the worker was left waiting on its ask".to_owned());
        };
        assert_eq!(reply["id"], "perm_1");
        assert_eq!(reply["result"]["outcome"]["outcome"], "cancelled");
        assert!(supervisor.parked.is_empty(), "nothing waits for a client");
        let letter = failure_letter(&channels, "asker-1").await?;
        assert_eq!(letter["kind"], "failure");
        assert!(
            letter["body"]
                .as_str()
                .is_some_and(|body| body.contains("Run rm -rf build")),
            "{letter}"
        );
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    /// Dies with a worker exit leaving the requester waiting forever for a session that died.
    #[tokio::test]
    async fn a_dead_worker_tells_the_asker() -> Result<(), String> {
        let dir = std::env::temp_dir().join(format!("yi-mail-exit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let channels = dir.join("channels");
        std::fs::create_dir_all(&channels).map_err(|error| error.to_string())?;
        let mut supervisor = supervisor_with_rows();
        supervisor.options.session_dir = dir.clone();
        let request: Envelope = serde_json::from_value(json!({
            "id": "asker-exit", "from": "history://asker", "to": "history://sleeper",
            "kind": "request", "conversation": "asker-exit", "seq": 1, "sentAt": 1,
            "body": "tidy",
        }))
        .map_err(|error| error.to_string())?;
        let mut entry = SessionEntry::new("/repo".to_owned(), None);
        entry.owed.push(request);
        supervisor.sessions.insert("sleeper".to_owned(), entry);
        supervisor.handle_worker_closed("/repo".to_owned());
        let letter = failure_letter(&channels, "asker-exit").await?;
        assert_eq!(letter["kind"], "failure");
        assert!(
            letter["body"]
                .as_str()
                .is_some_and(|body| body.contains("exited before answering")),
            "{letter}"
        );
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    /// Dies with one stranded request spawning a worker or mailing its asker again on every poll.
    #[tokio::test]
    async fn a_missing_history_tells_the_asker_only_once() -> Result<(), String> {
        let dir = std::env::temp_dir().join(format!("yi-mail-retry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let channels = dir.join("channels");
        std::fs::create_dir_all(&channels).map_err(|error| error.to_string())?;
        let mut supervisor = supervisor_with_rows();
        supervisor.options.session_dir = dir.clone();
        let request: Envelope = serde_json::from_value(json!({
            "id": "asker-retry", "from": "history://asker", "to": "history://sleeper",
            "kind": "request", "conversation": "asker-retry", "seq": 1, "sentAt": 1,
            "body": "tidy",
        }))
        .map_err(|error| error.to_string())?;
        let waiting = || Waiting {
            session: "sleeper".to_owned(),
            request: request.clone(),
            root: None,
        };
        supervisor.wake(vec![waiting()]).await;
        let letter = failure_letter(&channels, "asker-retry").await?;
        assert!(
            letter["body"]
                .as_str()
                .is_some_and(|body| body.contains("has no history to start")),
            "{letter}"
        );
        supervisor.wake(vec![waiting()]).await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let mailbox = yi_runtime::schedule::channel::Channel::at(channels.join("mail-asker.jsonl"));
        let failures = mailbox
            .entries()
            .into_iter()
            .filter(|entry| entry.data["inReplyTo"] == "asker-retry")
            .count();
        assert_eq!(
            failures, 1,
            "one request produced {failures} failure letters"
        );
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[tokio::test]
    async fn a_client_that_stops_reading_is_dropped_at_the_queue_cap() -> Result<(), String> {
        let path =
            std::path::PathBuf::from(format!("/tmp/yi-daemon-cap-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = tokio::net::UnixListener::bind(&path).expect("bind");
        let mut peer = tokio::net::UnixStream::connect(&path)
            .await
            .expect("connect");
        let (server, _) = listener.accept().await.expect("accept");
        let (input_tx, mut input_rx) = mpsc::unbounded_channel::<Input>();
        spawn_client(server, 7, input_tx);
        let Some(Input::Client(_, sink)) = input_rx.recv().await else {
            return Err("the client must register its sink".to_owned());
        };
        let line = "x".repeat(1024);
        let mut queued = 0_usize;
        while sink.try_send(line.clone()).is_ok() {
            queued = queued.saturating_add(1);
            assert!(
                queued <= CLIENT_QUEUE.saturating_mul(4),
                "the queue must fill"
            );
            tokio::task::yield_now().await;
        }
        drop(sink);
        let mut buffer = vec![0_u8; 1 << 20];
        let eof = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                match peer.read(&mut buffer).await {
                    Ok(0) | Err(_) => break true,
                    Ok(_) => {}
                }
            }
        })
        .await
        .unwrap_or(false);
        assert!(eof, "dropping the sink must close the client's socket");
        let _ = std::fs::remove_file(&path);
        Ok(())
    }
}
