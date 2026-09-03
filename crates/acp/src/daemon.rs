use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::{Child, ChildStdin};
use tokio::sync::mpsc;

use crate::{PROTOCOL_VERSION, negotiate};

pub struct DaemonOptions {
    pub socket: PathBuf,
    /// Arguments forwarded to every worker after `acp` (model, session-dir,
    /// yolo — the serve invocation's own surface).
    pub worker_args: Vec<String>,
    pub agent_version: String,
}

type ClientId = u64;

enum Input {
    Client(ClientId, mpsc::UnboundedSender<String>),
    ClientLine(ClientId, Value),
    ClientClosed(ClientId),
    WorkerLine(String, Value),
    WorkerReply(String, Value),
    WorkerClosed(String),
}

/// Asks parked for a detached session; past this the worker is refused
/// rather than left waiting.
const PARKED_MAX: usize = 8;

struct Worker {
    child: Child,
    stdin: ChildStdin,
}

/// Per-session routing plus the unseen ledger (in-memory, survives detach not restart).
/// Attachment is a set, so one session can be watched from any number of terminals.
struct SessionEntry {
    root: String,
    attached: std::collections::HashSet<ClientId>,
    unseen: u64,
    last_state: Option<String>,
    last_event_ms: u64,
    /// Invariant: true only for a row the pre-attach path invented so a resume's replay
    /// could route. The worker's result confirms it; an unconfirmed row dies with its client.
    provisional: bool,
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
            provisional: false,
        }
    }
}

#[expect(
    clippy::disallowed_methods,
    reason = "the ledger's timestamp is the job: when the last unattended update landed"
)]
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Routes ACP v2 between clients (unix socket) and one worker per root (`yi acp` over
/// stdio). Workers outlive client connections, so schedulers fire with nobody attached.
struct Supervisor {
    options: DaemonOptions,
    workers: HashMap<String, Worker>,
    clients: HashMap<ClientId, mpsc::UnboundedSender<String>>,
    /// session_id → routing and ledger state.
    sessions: HashMap<String, SessionEntry>,
    /// rewritten request id → (client, original id, root).
    requests: HashMap<u64, (ClientId, Value, String)>,
    /// worker-originated request id → root (permission bridge round trip).
    worker_requests: HashMap<String, String>,
    /// Worker-originated requests that arrived with no attached client, flushed on the next
    /// attach; without this the worker's synchronous asker blocks on an unseen answer.
    parked: HashMap<String, Vec<Value>>,
    next_request: u64,
    input: mpsc::UnboundedSender<Input>,
}

fn error_frame(id: Value, code: i64, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

fn result_frame(id: Value, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

impl Supervisor {
    fn send_client(&self, client: ClientId, line: String) {
        if let Some(sink) = self.clients.get(&client) {
            let _client_gone_is_fine = sink.send(line);
        }
    }

    async fn worker_for(&mut self, root: &str) -> Result<(), String> {
        if self.workers.contains_key(root) {
            return Ok(());
        }
        let exe = std::env::current_exe().map_err(|error| error.to_string())?;
        let mut command = tokio::process::Command::new(exe);
        command
            .arg("acp")
            .arg("--cwd")
            .arg(root)
            .args(&self.options.worker_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().map_err(|error| error.to_string())?;
        let stdin = child.stdin.take().ok_or("worker has no stdin")?;
        let stdout = child.stdout.take().ok_or("worker has no stdout")?;
        let input = self.input.clone();
        let worker_root = root.to_owned();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(value) = serde_json::from_str::<Value>(&line)
                    && input
                        .send(Input::WorkerLine(worker_root.clone(), value))
                        .is_err()
                {
                    break;
                }
            }
            let _ = input.send(Input::WorkerClosed(worker_root.clone()));
        });
        self.workers
            .insert(root.to_owned(), Worker { child, stdin });
        Ok(())
    }

    async fn forward_to_worker(
        &mut self,
        root: &str,
        client: ClientId,
        id: Option<Value>,
        mut frame: Value,
    ) {
        if let Some(original) = id {
            self.next_request = self.next_request.wrapping_add(1);
            let rewritten = self.next_request;
            self.requests
                .insert(rewritten, (client, original, root.to_owned()));
            if let Some(map) = frame.as_object_mut() {
                map.insert("id".to_owned(), json!(format!("sup_{rewritten}")));
            }
        }
        let line = frame.to_string();
        let gone = match self.workers.get_mut(root) {
            Some(worker) => {
                let mut payload = line.into_bytes();
                payload.push(b'\n');
                worker.stdin.write_all(&payload).await.is_err()
            }
            None => true,
        };
        if gone {
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
                            "capabilities": {},
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
                            "attached": !entry.attached.is_empty(),
                            "unseen": entry.unseen,
                            "lastState": entry.last_state,
                            "lastEventMs": entry.last_event_ms,
                        })
                    })
                    .collect();
                self.send_client(client, result_frame(id, json!({"sessions": sessions})));
            }
            "_yi/seen" => {
                let Some(id) = id else { return };
                let session_id = frame
                    .pointer("/params/sessionId")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                match self.sessions.get_mut(session_id) {
                    Some(entry) => {
                        entry.unseen = 0;
                        self.send_client(client, result_frame(id, json!({})));
                    }
                    None => {
                        self.send_client(client, error_frame(id, -32602, "unknown sessionId"));
                    }
                }
            }
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
        let line = frame.to_string();
        let gone = match self.workers.get_mut(root) {
            Some(worker) => {
                let mut payload = line.into_bytes();
                payload.push(b'\n');
                worker.stdin.write_all(&payload).await.is_err()
            }
            None => true,
        };
        if gone {
            self.handle_worker_closed(root.to_owned());
        }
    }

    fn handle_worker_line(&mut self, root: String, mut frame: Value) {
        let method = frame
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if method.is_empty() {
            // Response to a client-originated request.
            let Some(rewritten) = frame
                .get("id")
                .and_then(Value::as_str)
                .and_then(|id| id.strip_prefix("sup_"))
                .and_then(|id| id.parse::<u64>().ok())
            else {
                return;
            };
            let Some((client, original, request_root)) = self.requests.remove(&rewritten) else {
                return;
            };
            if let Some(session_id) = frame
                .pointer("/result/sessionId")
                .and_then(Value::as_str)
                .map(str::to_owned)
            {
                // Keep an existing ledger across re-resume; only the
                // attachment and root refresh.
                match self.sessions.get_mut(&session_id) {
                    Some(entry) => {
                        entry.root = request_root;
                        entry.attached.insert(client);
                        entry.provisional = false;
                    }
                    None => {
                        self.sessions
                            .insert(session_id, SessionEntry::new(request_root, Some(client)));
                    }
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
            return;
        };
        entry.last_event_ms = now_ms();
        if let Some(state) = state {
            entry.last_state = Some(state);
        }
        if entry.attached.is_empty() {
            entry.unseen = entry.unseen.saturating_add(1);
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
        let watchers: Vec<ClientId> = entry.attached.iter().copied().collect();
        let line = frame.to_string();
        for client in watchers {
            self.send_client(client, line.clone());
        }
    }

    fn flush_parked(&mut self, session_id: &str, client: ClientId) {
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
            .filter(|(_, (_, _, request_root))| *request_root == root)
            .map(|(id, _)| *id)
            .collect();
        for id in dead {
            if let Some((client, original, _)) = self.requests.remove(&id) {
                self.send_client(client, error_frame(original, -32603, "worker exited"));
            }
        }
        self.sessions.retain(|_, entry| entry.root != root);
        self.parked
            .retain(|session_id, _| self.sessions.contains_key(session_id));
    }

    fn handle_client_closed(&mut self, client: ClientId) {
        self.clients.remove(&client);
        for entry in self.sessions.values_mut() {
            entry.attached.remove(&client);
        }
        self.sessions
            .retain(|_, entry| !entry.provisional || !entry.attached.is_empty());
        self.requests
            .retain(|_, (request_client, _, _)| *request_client != client);
    }
}

fn spawn_client(stream: UnixStream, client: ClientId, input: mpsc::UnboundedSender<Input>) {
    let (read, mut write) = stream.into_split();
    let (sink, mut outgoing) = mpsc::unbounded_channel::<String>();
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

pub fn run_daemon(options: DaemonOptions, runtime: tokio::runtime::Runtime) -> i32 {
    runtime.block_on(async move {
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
        let mut supervisor = Supervisor {
            options,
            workers: HashMap::new(),
            clients: HashMap::new(),
            sessions: HashMap::new(),
            requests: HashMap::new(),
            worker_requests: HashMap::new(),
            parked: HashMap::new(),
            next_request: 0,
            input: input_tx.clone(),
        };
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
                    supervisor.handle_client_line(client, frame).await;
                }
                Input::ClientClosed(client) => supervisor.handle_client_closed(client),
                Input::WorkerLine(root, frame) => supervisor.handle_worker_line(root, frame),
                Input::WorkerReply(root, frame) => {
                    supervisor.forward_response_to_worker(&root, frame).await;
                }
                Input::WorkerClosed(root) => supervisor.handle_worker_closed(root),
            }
        }
        0
    })
}

pub fn daemon_protocol_version() -> u16 {
    PROTOCOL_VERSION
}
