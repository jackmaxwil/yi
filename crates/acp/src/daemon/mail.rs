//! A request waiting in a stopped session's mailbox starts that session.
use serde_json::{Value, json};
use tokio::sync::mpsc;
use yi_runtime::post::Waiting;
use yi_types::mail::Envelope;

use super::{ClientId, Input, SessionEntry, Supervisor, now_ms};

/// The id the daemon itself sends requests under; no client ever holds it.
pub(super) const SUPERVISOR: ClientId = 0;

const MAIL_POLL: std::time::Duration = std::time::Duration::from_secs(2);

pub(super) fn poll(sessions: &std::path::Path, input: mpsc::UnboundedSender<Input>) {
    let channels = sessions.join("channels");
    tokio::spawn(async move {
        let mut quiet = yi_runtime::post::Quiet::new();
        loop {
            tokio::time::sleep(MAIL_POLL).await;
            let dir = channels.clone();
            let scan = tokio::task::spawn_blocking(move || {
                let waiting = yi_runtime::post::waiting_requests(&dir, now_ms(), &mut quiet);
                (waiting, quiet)
            });
            let Ok((waiting, kept)) = scan.await else {
                quiet = yi_runtime::post::Quiet::new();
                continue;
            };
            quiet = kept;
            if !waiting.is_empty() && input.send(Input::Mail(waiting)).is_err() {
                break;
            }
        }
    });
}

/// The requests a headless session owes, taken when it raises an ask no client can answer and
/// dropped when its turn ends, since that turn answered them.
pub(super) fn owed_by_ask(
    entry: &mut SessionEntry,
    method: &str,
    frame: &Value,
    state: Option<&str>,
) -> Option<Vec<Envelope>> {
    if state == Some("idle") && entry.last_state.as_deref() == Some("running") {
        entry.owed.clear();
    }
    let ask = method == "session/request_permission" && frame.get("id").is_some();
    (ask && entry.headless && entry.attached.is_empty()).then(|| std::mem::take(&mut entry.owed))
}

impl Supervisor {
    pub(super) fn forget_failed_wake(&mut self, resumed: Option<String>, frame: &Value) {
        let Some(id) = resumed else { return };
        let Some(entry) = self.sessions.get_mut(&id) else {
            return;
        };
        let (owed, provisional) = (std::mem::take(&mut entry.owed), entry.provisional);
        entry.headless = false;
        if provisional {
            self.sessions.remove(&id);
        }
        let error = frame.pointer("/error/message").and_then(Value::as_str);
        let why = format!(
            "session {id} could not be started: {}",
            error.unwrap_or("no reason given")
        );
        self.tell(&owed, &why);
    }

    pub(super) fn tell_exited(&mut self, root: &str) {
        let owed: Vec<Envelope> = (self.sessions.values_mut())
            .filter(|entry| entry.root == root)
            .flat_map(|entry| std::mem::take(&mut entry.owed))
            .collect();
        self.tell(
            &owed,
            "the session started to read your request exited before answering",
        );
    }

    pub(super) fn tell(&self, requests: &[Envelope], why: &str) {
        let channels = self.options.session_dir.join("channels");
        for request in requests {
            if let Err(error) = yi_runtime::post::refuse(&channels, request, why) {
                eprintln!(
                    "daemon: could not tell {} of the refusal: {error}",
                    request.from
                );
            }
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
        self.tell(owed, &why);
    }

    /// Starts the stopped sessions mail is waiting on: the worker for the session's repository
    /// resumes it with no replay and no turn, which binds its mailbox, and the letter is read.
    pub(super) async fn wake(&mut self, waiting: Vec<Waiting>) {
        for Waiting { session, request } in waiting {
            if !self.woken.insert(request.id.0.clone()) {
                continue;
            }
            let dir = self.options.session_dir.clone();
            if let Some(entry) = self.sessions.get_mut(&session).filter(|e| e.loaded) {
                if entry.headless {
                    entry.owed.push(request);
                }
                continue;
            }
            let Some(root) = yi_runtime::history::session_cwd(&dir, &session) else {
                self.tell(
                    &[request],
                    &format!("session {session} has no history to start"),
                );
                continue;
            };
            let root = root.to_string_lossy().into_owned();
            if let Err(error) = self.worker_for(&root).await {
                let why = format!("session {session} could not be started: {error}");
                self.tell(&[request], &why);
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
