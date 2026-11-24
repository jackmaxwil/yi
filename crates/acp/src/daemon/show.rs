//! A worker's `_yi/show` and its answer. It is shown now or not at all: never parked for a
//! client that attaches later, and never answered by a client that only says it cannot.
use serde_json::{Value, json};

use super::{Input, Supervisor};
use yi_types::acp::SHOW_METHOD;

impl Supervisor {
    /// Sends a worker's show to every client watching its session; with none, the worker is
    /// told at once. True when `frame` was a show and is dealt with; any other id is kept.
    pub(super) fn worker_asked(&mut self, root: &str, frame: &Value, line: &str) -> bool {
        if frame.get("method").and_then(Value::as_str) != Some(SHOW_METHOD) {
            if let Some(id) = frame.get("id").and_then(Value::as_str) {
                self.worker_requests.insert(id.to_owned(), root.to_owned());
            }
            return false;
        }
        let session = frame.pointer("/params/sessionId").and_then(Value::as_str);
        let entry = session.and_then(|session| self.sessions.get(session));
        let watchers = entry.map(|entry| entry.attached.iter().copied().collect::<Vec<_>>());
        let watchers = watchers.unwrap_or_default();
        for client in &watchers {
            self.send_client(*client, line.to_owned());
        }
        if watchers.is_empty() {
            let id = frame.get("id").cloned().unwrap_or(Value::Null);
            let error = json!({"code": -32603, "message": "no client attached"});
            let reply = json!({"jsonrpc": "2.0", "id": id, "error": error});
            let _ = self.input.send(Input::WorkerReply(root.to_owned(), reply));
        }
        true
    }

    /// The worker a client's answer to a show goes back to: the one that holds the session
    /// the request id names. None for an error, which another client's answer must not lose to.
    pub(super) fn show_root(&self, request_id: &str, frame: &Value) -> Option<String> {
        let session = request_id.strip_prefix("show_")?.rsplit_once('_')?.0;
        let shown = frame.get("error").is_none();
        let entry = self.sessions.get(session).filter(|_| shown)?;
        Some(entry.root.clone())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use tokio::sync::mpsc;

    use super::super::tests::supervisor_with_rows;
    use super::super::{Input, SessionEntry};

    fn show(session: &str) -> Value {
        let params =
            json!({"sessionId": session, "address": "a.rs", "cwd": "/repo", "strength": "tab"});
        json!({"jsonrpc": "2.0", "id": format!("show_{session}_0"), "method": "_yi/show", "params": params})
    }

    /// A show goes to the client watching its session. With none watching, the worker is told
    /// at once: a show kept for whoever attaches next would open a file minutes late.
    #[tokio::test]
    async fn a_show_reaches_the_watching_client_and_is_never_kept_for_a_later_one()
    -> Result<(), String> {
        let mut supervisor = supervisor_with_rows();
        let (client, mut sent) = mpsc::channel(8);
        supervisor.clients.insert(7, client);
        let (input, mut inputs) = mpsc::unbounded_channel();
        supervisor.input = input;
        let frame = show("pinned");
        supervisor.handle_worker_line("/repo".to_owned(), frame.clone(), frame.to_string());
        assert_eq!(
            sent.try_recv().ok(),
            Some(frame.to_string()),
            "to the client"
        );
        assert!(
            inputs.try_recv().is_err(),
            "the worker waits for its answer"
        );
        if let Some(entry) = supervisor.sessions.get_mut("pinned") {
            entry.attached.clear();
        }
        supervisor.handle_worker_line("/repo".to_owned(), frame.clone(), frame.to_string());
        let Ok(Input::WorkerReply(root, reply)) = inputs.try_recv() else {
            return Err("the worker was left waiting with no client attached".to_owned());
        };
        assert_eq!((root.as_str(), &reply["id"]), ("/repo", &frame["id"]));
        assert!(reply.get("error").is_some(), "{reply}");
        assert!(sent.try_recv().is_err(), "and no client was sent it");
        assert!(supervisor.parked.is_empty() && supervisor.worker_requests.is_empty());
        Ok(())
    }

    /// A client's answer is written to the worker that asked; the error of a client that
    /// cannot show, sent first, is not. The worker here is `cat`, read back line by line.
    #[tokio::test]
    async fn a_clients_answer_is_written_to_the_worker_and_its_error_is_not() -> Result<(), String>
    {
        use tokio::io::AsyncBufReadExt;
        let mut supervisor = supervisor_with_rows();
        let mut stub = tokio::process::Command::new("cat");
        stub.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = stub.spawn().map_err(|error| error.to_string())?;
        let stdin = child.stdin.take().ok_or("the stub worker has no stdin")?;
        let stdout = child.stdout.take().ok_or("the stub worker has no stdout")?;
        let (root, generation) = ("/repo".to_owned(), 0);
        let worker = super::super::Worker {
            child,
            stdin,
            generation,
        };
        supervisor.workers.insert(root, worker);
        let deaf = json!({"jsonrpc": "2.0", "id": "show_pinned_0",
            "error": {"code": -32601, "message": "Method not found"}});
        let answer = json!({"jsonrpc": "2.0", "id": "show_pinned_0", "result": {"outcome": "tab"}});
        supervisor.handle_client_line(8, deaf).await;
        supervisor.handle_client_line(7, answer.clone()).await;
        let mut lines = tokio::io::BufReader::new(stdout).lines();
        let limit = std::time::Duration::from_secs(5);
        let Ok(Ok(Some(line))) = tokio::time::timeout(limit, lines.next_line()).await else {
            return Err("the worker never read the answer".to_owned());
        };
        assert_eq!(
            line,
            answer.to_string(),
            "the answer, with no error ahead of it"
        );
        Ok(())
    }

    /// An answer goes back to the worker that holds the session its id names. A client that
    /// only says it cannot show is not an answer: another client's must still get through.
    #[test]
    fn an_answer_finds_its_worker_and_a_clients_error_is_dropped() {
        let mut supervisor = supervisor_with_rows();
        let other = SessionEntry::new("/else".to_owned(), None);
        supervisor.sessions.insert("a_b".to_owned(), other);
        let shown = json!({"result": {"outcome": "tab"}});
        let deaf = json!({"error": {"code": -32601, "message": "Method not found"}});
        let root = |id: &str, frame: &Value| supervisor.show_root(id, frame);
        assert_eq!(root("show_pinned_3", &shown).as_deref(), Some("/repo"));
        assert_eq!(root("show_a_b_0", &shown).as_deref(), Some("/else"));
        assert_eq!(root("show_pinned_3", &deaf), None, "an error");
        assert_eq!(root("show_gone_1", &shown), None, "no such session");
        assert_eq!(root("perm_pinned_3", &shown), None, "not a show");
    }
}
