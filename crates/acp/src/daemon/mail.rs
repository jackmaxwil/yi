//! A request waiting in a stopped session's mailbox starts that session.
use serde_json::{Value, json};
use tokio::sync::mpsc;
use yi_runtime::post::Waiting;
use yi_types::mail::Envelope;

use super::{Input, SUPERVISOR, SessionEntry, Supervisor, now_ms};

const MAIL_POLL: std::time::Duration = std::time::Duration::from_secs(2);

pub(super) fn poll(channels: std::path::PathBuf, input: mpsc::UnboundedSender<Input>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(MAIL_POLL).await;
            let dir = channels.clone();
            let scan = tokio::task::spawn_blocking(move || {
                yi_runtime::post::waiting_requests(&dir, now_ms())
            });
            let Ok(waiting) = scan.await else { continue };
            if !waiting.is_empty() && input.send(Input::Mail(waiting)).is_err() {
                break;
            }
        }
    });
}

/// The requests a headless session owes, taken when it raises an ask no client can answer.
pub(super) fn owed_by_ask(
    entry: &mut SessionEntry,
    method: &str,
    frame: &Value,
) -> Option<Vec<Envelope>> {
    let ask = method == "session/request_permission" && frame.get("id").is_some();
    (ask && entry.headless && entry.attached.is_empty()).then(|| std::mem::take(&mut entry.owed))
}

impl Supervisor {
    pub(super) fn forget_failed_wake(&mut self, resumed: Option<String>) {
        let failed = resumed.filter(|id| self.sessions.get(id).is_some_and(|e| e.provisional));
        if let Some(id) = failed {
            self.sessions.remove(&id);
        }
    }

    pub(super) fn refuse_ask(
        &mut self,
        root: String,
        session_id: &str,
        frame: &Value,
        owed: &[Envelope],
    ) {
        if let Some(request_id) = frame.get("id").and_then(Value::as_str) {
            self.worker_requests.remove(request_id);
        }
        let reply = json!({
            "jsonrpc": "2.0",
            "id": frame.get("id"),
            "result": {"outcome": {"outcome": "cancelled"}},
        });
        let _ = self.input.send(Input::WorkerReply(root, reply));
        let title = frame
            .pointer("/params/title")
            .and_then(Value::as_str)
            .unwrap_or("a permission request");
        let why = format!(
            "session {session_id} was started to read your request and refused \"{title}\": nobody was attached to approve it"
        );
        let channels = self.options.session_dir.join("channels");
        for request in owed {
            if let Err(error) = yi_runtime::post::refuse(&channels, request, &why) {
                eprintln!(
                    "daemon: could not tell {} of the refusal: {error}",
                    request.from
                );
            }
        }
    }

    /// Starts the stopped sessions mail is waiting on: the worker for the session's repository
    /// resumes it with no replay and no turn, which binds its mailbox, and the letter is read.
    pub(super) async fn wake(&mut self, waiting: Vec<Waiting>) {
        for Waiting { session, request } in waiting {
            if !self.woken.insert(request.id.0.clone()) {
                continue;
            }
            let dir = self.options.session_dir.clone();
            let Some(root) = yi_runtime::history::session_cwd(&dir, &session) else {
                continue;
            };
            let root = root.to_string_lossy().into_owned();
            if self.sessions.contains_key(&session) && self.workers.contains_key(&root) {
                continue;
            }
            if let Err(error) = self.worker_for(&root).await {
                eprintln!("daemon: no worker for {root}: {error}");
                continue;
            }
            let entry = (self.sessions.entry(session.clone()))
                .or_insert_with(|| SessionEntry::provisional(root.clone()));
            if entry.attached.is_empty() {
                entry.headless = true;
                entry.owed.push(request);
            }
            let frame = json!({
                "jsonrpc": "2.0",
                "method": "session/resume",
                "params": {"sessionId": session, "cwd": root},
            });
            self.forward_to_worker(&root, SUPERVISOR, Some(json!("mail")), frame)
                .await;
        }
    }
}
