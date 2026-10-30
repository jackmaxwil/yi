//! Worker replacement: a root's `yi acp` worker is swapped for a freshly built binary while the
//! supervisor, its clients and every session's ledger stay where they are.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

use crate::AcpState;
use crate::daemon::{ClientId, Input, Supervisor, error_frame, result_frame};

pub(crate) const BUSY: i64 = -32000;
const INVALID_PARAMS: i64 = -32602;
const PREPARE_TIMEOUT: Duration = Duration::from_secs(30);
const RESTORE_TIMEOUT: Duration = Duration::from_secs(60);
const SWAP_ID: &str = "swap_";
const RESTORE_ID: &str = "restore_";

/// Per-root replacement state: the binary a root was last replaced with is what later spawns of
/// that root use, so a dead worker cannot bring the superseded build back.
#[derive(Default)]
pub(crate) struct Swaps {
    pub(crate) binaries: HashMap<String, PathBuf>,
    pub(crate) next_generation: u64,
    next_id: u64,
    active: HashMap<String, Swap>,
}

struct Swap {
    id: u64,
    generation: u64,
    binary: PathBuf,
    client: ClientId,
    request: Value,
    stage: Stage,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Preparing,
    Exiting,
    Restoring,
}

/// The old worker's inventory answer or the new worker's restore answer, by id prefix.
pub(crate) fn is_reply(frame: &Value) -> bool {
    frame.get("method").is_none()
        && frame
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| id.starts_with(SWAP_ID) || id.starts_with(RESTORE_ID))
}

fn executable(path: &str) -> Result<PathBuf, String> {
    use std::os::unix::fs::PermissionsExt;
    let invalid = |why: String| format!("invalid --binary {path}: {why}");
    let canonical = std::fs::canonicalize(path).map_err(|error| invalid(error.to_string()))?;
    let meta = std::fs::metadata(&canonical).map_err(|error| invalid(error.to_string()))?;
    if !meta.is_file() {
        return Err(invalid("not a regular file".to_owned()));
    }
    if meta.permissions().mode() & 0o111 == 0 {
        return Err(invalid("not executable".to_owned()));
    }
    Ok(canonical)
}

impl Supervisor {
    /// Whether `generation` is still the worker this root serves: a superseded worker's close
    /// must not tear down the build that replaced it.
    pub(crate) fn serves(&self, root: &str, generation: u64) -> bool {
        self.workers
            .get(root)
            .is_some_and(|worker| worker.generation == generation)
    }

    pub(crate) fn swap_refusal(&self, root: &str) -> Option<String> {
        let swap = self.swap.active.get(root)?;
        Some(format!(
            "worker replacement {} is in progress for {root} (generation {} -> {}); nothing was sent; retry after _yi/worker_replaced",
            swap.id,
            swap.generation,
            swap.generation.wrapping_add(1)
        ))
    }

    /// The swap waiting on this worker's close, so its close completes the swap instead of
    /// being read as an unexpected death.
    pub(crate) fn swap_closing(&self, root: &str, generation: u64) -> bool {
        self.swap
            .active
            .get(root)
            .is_some_and(|swap| swap.stage == Stage::Exiting && swap.generation == generation)
    }

    /// `_yi/worker_replace {cwd, binary}`: the developer verb's one request.
    pub(crate) async fn replace(&mut self, client: ClientId, id: Option<Value>, frame: &Value) {
        let Some(id) = id else { return };
        let param = |key: &str| {
            frame
                .pointer(&format!("/params/{key}"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        let (Some(root), Some(binary)) = (param("cwd"), param("binary")) else {
            let refused = "_yi/worker_replace takes a cwd and a binary";
            return self.send_client(client, error_frame(id, INVALID_PARAMS, refused));
        };
        if let Some(swap) = self.swap.active.get(&root) {
            let busy = format!(
                "worker replacement {} is already in progress for {root}",
                swap.id
            );
            return self.send_client(client, error_frame(id, BUSY, &busy));
        }
        let binary = match executable(&binary) {
            Ok(binary) => binary,
            Err(refused) => {
                return self.send_client(client, error_frame(id, INVALID_PARAMS, &refused));
            }
        };
        let Some(generation) = self.workers.get(&root).map(|worker| worker.generation) else {
            // Nothing to retire: the requested binary is what this root serves from now on.
            self.swap.binaries.insert(root.clone(), binary.clone());
            let spawned = self.spawn_worker(&root);
            if let Err(error) = spawned {
                self.swap.binaries.remove(&root);
                let why = format!("worker replacement failed for {root}: {error}");
                return self.send_client(client, error_frame(id, BUSY, &why));
            }
            let to = self
                .workers
                .get(&root)
                .map_or(0, |worker| worker.generation);
            let result = json!({
                "replacementId": 0,
                "from": 0,
                "to": to,
                "binary": binary.display().to_string(),
                "sessions": [],
            });
            return self.send_client(client, result_frame(id, result));
        };
        self.swap.next_id = self.swap.next_id.wrapping_add(1);
        let replacement = self.swap.next_id;
        self.swap.active.insert(
            root.clone(),
            Swap {
                id: replacement,
                generation,
                binary,
                client,
                request: id,
                stage: Stage::Preparing,
            },
        );
        let prepare = json!({
            "jsonrpc": "2.0",
            "id": format!("{SWAP_ID}{replacement}"),
            "method": "_yi/prepare_replace",
            "params": {"replacementId": replacement, "generation": generation},
        });
        if !self.write_worker(&root, prepare.to_string()).await {
            self.refuse_swap(&root, "the worker exited before it answered");
            return;
        }
        self.schedule_timeout(&root, replacement, PREPARE_TIMEOUT);
    }

    fn schedule_timeout(&self, root: &str, id: u64, after: Duration) {
        let input = self.input.clone();
        let root = root.to_owned();
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            let _ = input.send(Input::SwapTimeout(root, id));
        });
    }

    /// The old worker's inventory: blockers leave it serving, a prepared answer ends it.
    pub(crate) async fn swap_reply(&mut self, root: &str, frame: &Value) {
        let Some(swap) = self.swap.active.get(root) else {
            return;
        };
        let (id, stage) = (swap.id, swap.stage);
        let reply = frame.get("id").and_then(Value::as_str).unwrap_or("");
        match stage {
            Stage::Preparing if reply == format!("{SWAP_ID}{id}") => {
                if frame.pointer("/result/prepared").and_then(Value::as_bool) != Some(true) {
                    let mut blockers: Vec<String> = frame
                        .pointer("/result/blockers")
                        .and_then(Value::as_array)
                        .map(|rows| {
                            rows.iter()
                                .filter_map(Value::as_str)
                                .map(str::to_owned)
                                .collect()
                        })
                        .unwrap_or_default();
                    if blockers.is_empty() {
                        blockers.push(
                            frame
                                .pointer("/error/message")
                                .and_then(Value::as_str)
                                .unwrap_or("the old worker refused")
                                .to_owned(),
                        );
                    }
                    self.refuse_swap(root, &blockers.join("; "));
                    return;
                }
                if let Some(swap) = self.swap.active.get_mut(root) {
                    swap.stage = Stage::Exiting;
                }
                // The prepared worker exits on its own; a worker that does not is killed at
                // the timeout rather than left serving a build the caller already replaced.
                self.schedule_timeout(root, id, PREPARE_TIMEOUT);
            }
            Stage::Restoring if reply == format!("{RESTORE_ID}{id}") => {
                let failed: Vec<String> = frame
                    .pointer("/result/failed")
                    .and_then(Value::as_array)
                    .map(|rows| {
                        rows.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_else(|| {
                        vec![
                            frame
                                .pointer("/error/message")
                                .and_then(Value::as_str)
                                .unwrap_or("the new worker did not answer the restore")
                                .to_owned(),
                        ]
                    });
                let why = (!failed.is_empty()).then(|| failed.join("; "));
                self.finish_swap(root, why);
            }
            Stage::Preparing | Stage::Exiting | Stage::Restoring => {}
        }
    }

    /// The prepared worker closed: the requested binary takes its place, then the sessions it
    /// owned are loaded again and the attached clients are told to resume.
    pub(crate) async fn swap_exited(&mut self, root: &str) {
        let Some((id, binary, sessions)) = self.swap.active.get(root).map(|swap| {
            (
                swap.id,
                swap.binary.clone(),
                self.sessions
                    .iter()
                    .filter(|(_, entry)| entry.root == root)
                    .map(|(session_id, _)| session_id.clone())
                    .collect::<Vec<String>>(),
            )
        }) else {
            return;
        };
        if let Some(swap) = self.swap.active.get_mut(root) {
            swap.stage = Stage::Restoring;
        }
        if let Some(mut worker) = self.workers.remove(root) {
            let _reaped = worker.child.start_kill();
        }
        self.swap.binaries.insert(root.to_owned(), binary);
        if let Err(error) = self.spawn_worker(root) {
            let why = format!("the requested binary could not serve {root}: {error}");
            self.finish_swap(root, Some(why));
            return;
        }
        if sessions.is_empty() {
            self.finish_swap(root, None);
            return;
        }
        let restore = json!({
            "jsonrpc": "2.0",
            "id": format!("{RESTORE_ID}{id}"),
            "method": "_yi/restore",
            "params": {"sessions": sessions},
        });
        if !self.write_worker(root, restore.to_string()).await {
            self.finish_swap(
                root,
                Some("the new worker exited before restoring".to_owned()),
            );
            return;
        }
        self.schedule_timeout(root, id, RESTORE_TIMEOUT);
    }

    pub(crate) fn swap_timeout(&mut self, root: &str, id: u64) {
        let Some((stage, generation)) = self
            .swap
            .active
            .get(root)
            .filter(|swap| swap.id == id)
            .map(|swap| (swap.stage, swap.generation))
        else {
            return;
        };
        match stage {
            Stage::Preparing => {
                let why = format!(
                    "the old worker did not answer within {} s",
                    PREPARE_TIMEOUT.as_secs()
                );
                self.refuse_swap(root, &why);
            }
            Stage::Exiting => {
                if let Some(worker) = self.workers.get_mut(root)
                    && worker.generation == generation
                {
                    let _killed = worker.child.start_kill();
                }
            }
            Stage::Restoring => {
                let why = format!(
                    "the session restore did not answer within {} s",
                    RESTORE_TIMEOUT.as_secs()
                );
                self.finish_swap(root, Some(why));
            }
        }
    }

    /// A replacement whose worker died under it: the caller is told nothing replaced it.
    pub(crate) fn abandon_swap(&mut self, root: &str, why: &str) {
        let Some(swap) = self.swap.active.remove(root) else {
            return;
        };
        let failed = format!("worker replacement failed for {root}: {why}");
        self.send_client(swap.client, error_frame(swap.request, BUSY, &failed));
    }

    /// A replacement that did not happen: the old worker keeps serving and its caller is told.
    pub(crate) fn refuse_swap(&mut self, root: &str, why: &str) {
        let Some(swap) = self.swap.active.remove(root) else {
            return;
        };
        let refused = format!("worker replacement refused for {root}: {why}; nothing was replaced");
        self.send_client(swap.client, error_frame(swap.request, BUSY, &refused));
    }

    /// A replacement that happened, with or without a restore failure: success is answered
    /// only when every acknowledged session loaded again.
    fn finish_swap(&mut self, root: &str, failed: Option<String>) {
        let Some(swap) = self.swap.active.remove(root) else {
            return;
        };
        let to = self
            .workers
            .get(root)
            .map_or(swap.generation, |worker| worker.generation);
        let sessions = self.notify_replaced(root, to, &swap.binary);
        if let Some(why) = failed {
            let refused =
                format!("worker replacement failed for {root}: {why}; generation {to} is serving");
            return self.send_client(swap.client, error_frame(swap.request, BUSY, &refused));
        }
        let result = json!({
            "replacementId": swap.id,
            "from": swap.generation,
            "to": to,
            "binary": swap.binary.display().to_string(),
            "sessions": sessions,
        });
        self.send_client(swap.client, result_frame(swap.request, result));
    }

    /// Tells every attached client its session must resume from the ledger, and answers the
    /// session ids the new generation holds.
    fn notify_replaced(
        &mut self,
        root: &str,
        generation: u64,
        binary: &std::path::Path,
    ) -> Vec<String> {
        let mut sessions: Vec<String> = Vec::new();
        let mut watchers: Vec<(ClientId, String)> = Vec::new();
        for (session_id, entry) in &self.sessions {
            if entry.root != root {
                continue;
            }
            sessions.push(session_id.clone());
            for client in &entry.attached {
                watchers.push((*client, session_id.clone()));
            }
        }
        let update = crate::update::extension(
            "_yi/worker_replaced",
            [
                ("binary", json!(binary.display().to_string())),
                ("generation", json!(generation)),
            ],
        );
        for (client, session_id) in watchers {
            let frame = crate::update::update_notification(&session_id, update.clone());
            self.send_client(client, frame.to_string());
        }
        sessions
    }
}

impl AcpState {
    /// Every reason this worker may not be replaced, named; empty means it may.
    pub(crate) fn replace_blockers(&self) -> Vec<String> {
        let mut blockers: Vec<String> = self
            .building
            .keys()
            .map(|id| format!("session {id} is still starting"))
            .collect();
        for (id, handle) in &self.sessions {
            let session = &handle.session;
            if session.status() == yi_runtime::Status::Running {
                blockers.push(format!("session {id} is running a turn"));
            }
            if !crate::update::queued(&handle.prompts).is_empty() {
                blockers.push(format!("session {id} has a queued prompt"));
            }
            if session.pending_count() > 0 {
                blockers.push(format!("session {id} has an unread follow-up"));
            }
            if let Some(kernel) = session.kernel_service() {
                if kernel.user_cell_running() {
                    blockers.push(format!("session {id} is running a kernel cell"));
                }
                if kernel.lossy() {
                    blockers.push(format!(
                        "session {id} has a kernel variable its snapshot cannot hold"
                    ));
                }
            }
            if let Some(child) = handle
                .host
                .children_view()
                .into_iter()
                .find(|child| child.update.status == yi_types::subagent::ChildStatus::Running)
            {
                blockers.push(format!(
                    "session {id} has a running child {}",
                    child.update.id.as_str()
                ));
            }
            for job in yi_runtime::running_jobs(session.job_owner()) {
                blockers.push(format!("session {id} has a running job: {job}"));
            }
        }
        let asking = self
            .pending
            .lock()
            .map(|asks| asks.as_ref().is_some_and(|asks| !asks.is_empty()))
            .unwrap_or(true);
        if asking {
            blockers.push("a permission ask is in flight".to_owned());
        }
        blockers
    }

    /// `_yi/prepare_replace`: the inventory first, then a checked flush of every kernel so the
    /// next worker boots the same variables. An idle session writes nothing to the ledger.
    pub(crate) async fn prepare_replace(&mut self) -> Value {
        let blockers = self.replace_blockers();
        if !blockers.is_empty() {
            return json!({"prepared": false, "blockers": blockers, "sessions": []});
        }
        let sessions: Vec<String> = self.sessions.keys().cloned().collect();
        for handle in self.sessions.values() {
            if let Some(kernel) = handle.session.kernel_service() {
                kernel.dispose().await;
            }
        }
        json!({"prepared": true, "blockers": [], "sessions": sessions})
    }

    /// `_yi/restore`: load the sessions the previous worker owned, without replay. Their
    /// schedulers and mailbox subscriptions bind from the ledger as a fresh daemon's would.
    pub(crate) fn restore(&mut self, params: &Value) -> Value {
        let ids: Vec<String> = params
            .get("sessions")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let mut restored = Vec::new();
        let mut failed = Vec::new();
        for id in ids {
            if self.sessions.contains_key(&id) || self.building.contains_key(&id) {
                restored.push(id);
                continue;
            }
            match self.open_later(&id) {
                Ok(_) => restored.push(id),
                Err(why) => failed.push(format!("session {id}: {why}")),
            }
        }
        json!({"sessions": restored, "failed": failed})
    }
}
