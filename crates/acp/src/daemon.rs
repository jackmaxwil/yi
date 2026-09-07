use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::Stdio;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::{Child, ChildStdin};
use tokio::sync::mpsc;
use yi_types::acp::{DaemonLedger, DaemonLedgerEntry};

use crate::{PROTOCOL_VERSION, negotiate};

pub struct DaemonOptions {
    pub socket: PathBuf,
    /// Arguments forwarded to every worker after `acp` (model, session-dir,
    /// yolo — the serve invocation's own surface).
    pub worker_args: Vec<String>,
    pub agent_version: String,
}

type ClientId = u64;

const CLIENT_QUEUE: usize = 4096;

enum Input {
    Client(ClientId, mpsc::Sender<String>),
    ClientLine(ClientId, Value),
    ClientClosed(ClientId),
    WorkerLine(String, Value),
    WorkerReply(String, Value),
    WorkerClosed(String),
    Shutdown,
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
    name: Option<String>,
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
            name: None,
            provisional: false,
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
    clients: HashMap<ClientId, mpsc::Sender<String>>,
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
    /// A client that cannot keep up is dropped, never queued without bound; it heals on reconnect.
    fn send_client(&self, client: ClientId, line: String) {
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
        let path = ledger_path(&self.options.socket);
        let tmp = path.with_extension("json.tmp");
        let Ok(text) = serde_json::to_string(&ledger) else {
            return;
        };
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
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
                            "name": entry.name,
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
                let name = frame
                    .pointer("/result/name")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                match self.sessions.get_mut(&session_id) {
                    Some(entry) => {
                        entry.root = request_root;
                        entry.attached.insert(client);
                        entry.provisional = false;
                        entry.name = name.or(entry.name.take());
                    }
                    None => {
                        let mut entry = SessionEntry::new(request_root, Some(client));
                        entry.name = name;
                        self.sessions.insert(session_id, entry);
                    }
                }
                self.persist();
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
        let transition = state.is_some();
        if let Some(state) = state {
            entry.last_event_ms = now_ms();
            entry.last_state = Some(state);
        }
        let detached = entry.attached.is_empty();
        if transition && detached {
            entry.unseen = entry.unseen.saturating_add(1);
        }
        let watchers: Vec<ClientId> = entry.attached.iter().copied().collect();
        if transition {
            self.persist();
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
        let mut supervisor = Supervisor {
            sessions: load_ledger(&options.socket),
            options,
            workers: HashMap::new(),
            clients: HashMap::new(),
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
                Input::Shutdown => break,
            }
        }
        for worker in supervisor.workers.values_mut() {
            let _ = worker.child.kill().await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let _ = std::fs::remove_file(&supervisor.options.socket);
        0
    })
}

pub fn daemon_protocol_version() -> u16 {
    PROTOCOL_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

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
