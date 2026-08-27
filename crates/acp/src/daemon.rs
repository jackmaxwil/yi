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
    WorkerClosed(String),
}

struct Worker {
    child: Child,
    stdin: ChildStdin,
}

/// Routes ACP v2 between clients (unix socket) and one worker per root (`yi
/// acp` over stdio). Workers outlive client connections, so schedulers keep
/// firing while nobody is attached.
struct Supervisor {
    options: DaemonOptions,
    workers: HashMap<String, Worker>,
    clients: HashMap<ClientId, mpsc::UnboundedSender<String>>,
    /// session_id → (root, last attached client).
    sessions: HashMap<String, (String, Option<ClientId>)>,
    /// rewritten request id → (client, original id, root).
    requests: HashMap<u64, (ClientId, Value, String)>,
    /// worker-originated request id → root (permission bridge round trip).
    worker_requests: HashMap<String, String>,
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
                self.forward_to_worker(&root, client, id, frame).await;
            }
            "session/list" => {
                let Some(id) = id else { return };
                let sessions: Vec<Value> = self
                    .sessions
                    .iter()
                    .map(|(session_id, (root, attached))| {
                        json!({
                            "sessionId": session_id,
                            "cwd": root,
                            "attached": attached.is_some(),
                        })
                    })
                    .collect();
                self.send_client(client, result_frame(id, json!({"sessions": sessions})));
            }
            _ => {
                let Some(root) = self.root_of(&frame) else {
                    if let Some(id) = id {
                        self.send_client(client, error_frame(id, -32602, "unknown sessionId"));
                    }
                    return;
                };
                if let Some(session_id) = frame.pointer("/params/sessionId").and_then(Value::as_str)
                    && let Some(entry) = self.sessions.get_mut(session_id)
                {
                    entry.1 = Some(client);
                }
                self.forward_to_worker(&root, client, id, frame).await;
            }
        }
    }

    fn root_of(&self, frame: &Value) -> Option<String> {
        let session_id = frame.pointer("/params/sessionId").and_then(Value::as_str)?;
        self.sessions.get(session_id).map(|(root, _)| root.clone())
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
                self.sessions
                    .insert(session_id, (request_root, Some(client)));
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
        // Notifications route to the session's attached client; with no
        // client attached the update is dropped and the worker keeps
        // running — that is the reconnect contract.
        let target = frame
            .pointer("/params/sessionId")
            .and_then(Value::as_str)
            .and_then(|session_id| self.sessions.get(session_id))
            .and_then(|(_, attached)| *attached);
        if let Some(client) = target {
            self.send_client(client, frame.to_string());
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
        self.sessions
            .retain(|_, (session_root, _)| *session_root != root);
    }

    fn handle_client_closed(&mut self, client: ClientId) {
        self.clients.remove(&client);
        for entry in self.sessions.values_mut() {
            if entry.1 == Some(client) {
                entry.1 = None;
            }
        }
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
                Input::WorkerClosed(root) => supervisor.handle_worker_closed(root),
            }
        }
        0
    })
}

pub fn daemon_protocol_version() -> u16 {
    PROTOCOL_VERSION
}
