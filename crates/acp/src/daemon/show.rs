//! A worker's `_yi/show` and its answer. It is shown now or not at all: never parked for a
//! client that attaches later, and never answered by a client that only says it cannot.
use serde_json::{Value, json};

use super::pending::Held;
use super::{Input, Supervisor};
use yi_types::acp::SHOW_METHOD;

impl Supervisor {
    /// Sends a worker's show to every client watching its session; with none, the worker is
    /// told at once. True when `frame` was a show and is dealt with.
    pub(super) fn worker_asked(&mut self, root: &str, frame: &Value, line: &str) -> bool {
        if frame.get("method").and_then(Value::as_str) != Some(SHOW_METHOD) {
            return false;
        }
        let session = frame.pointer("/params/sessionId").and_then(Value::as_str);
        let entry = session.and_then(|session| self.sessions.get(session));
        let watchers = entry.map(|entry| entry.attached.iter().copied().collect::<Vec<_>>());
        let watchers = watchers.unwrap_or_default();
        if !watchers.is_empty()
            && let Some(id) = frame.get("id").and_then(Value::as_str)
        {
            let held = Held::new(root.to_owned(), frame);
            self.worker_requests.insert(id.to_owned(), held);
        }
        for client in &watchers {
            self.send_client(*client, line.to_owned());
        }
        if watchers.is_empty() {
            if let Some(id) = frame.get("id").and_then(Value::as_str) {
                self.worker_requests.remove(id);
            }
            let id = frame.get("id").cloned().unwrap_or(Value::Null);
            let error = json!({"code": -32603, "message": "no client attached"});
            let reply = json!({"jsonrpc": "2.0", "id": id, "error": error});
            let _ = self.input.send(Input::WorkerReply(root.to_owned(), reply));
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use tokio::sync::mpsc;

    use super::super::Input;
    use super::super::pending::Held;
    use super::super::tests::supervisor_with_rows;

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
    /// cannot show, sent first, is not, while a permission ask's error is. The worker is `cat`.
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
        for id in ["show_pinned_0", "perm_pinned_1"] {
            let held = Held::new("/repo".to_owned(), &json!({"id": id}));
            supervisor.worker_requests.insert(id.to_owned(), held);
        }
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
        let refused = json!({"jsonrpc": "2.0", "id": "perm_pinned_1",
            "error": {"code": -32601, "message": "Method not found"}});
        supervisor.handle_client_line(7, refused.clone()).await;
        let Ok(Ok(Some(line))) = tokio::time::timeout(limit, lines.next_line()).await else {
            return Err("a permission ask's error never reached the worker".to_owned());
        };
        assert_eq!(line, refused.to_string());
        assert!(supervisor.worker_requests.is_empty());
        Ok(())
    }

    /// Watched shows own a response id until a result arrives. A client error leaves it for the
    /// next client, while a detached show does not leave an id behind.
    #[tokio::test]
    async fn an_answer_finds_its_worker_and_a_clients_error_is_dropped() -> Result<(), String> {
        let mut supervisor = supervisor_with_rows();
        let watched = show("pinned");
        assert!(supervisor.worker_asked("/repo", &watched, &watched.to_string()));
        assert_eq!(
            supervisor
                .worker_requests
                .get("show_pinned_0")
                .map(|held| held.root.as_str()),
            Some("/repo")
        );
        let Some(entry) = supervisor.sessions.get_mut("loose") else {
            return Err("the detached session was not seeded".to_owned());
        };
        entry.attached.clear();
        let detached = show("loose");
        assert!(supervisor.worker_asked("/repo", &detached, &detached.to_string()));
        assert!(
            !supervisor.worker_requests.contains_key("show_loose_0"),
            "a detached show is refused immediately"
        );
        let error = json!({"jsonrpc": "2.0", "id": "show_pinned_0",
            "error": {"code": -32601, "message": "Method not found"}});
        supervisor.handle_client_line(7, error).await;
        assert_eq!(
            supervisor
                .worker_requests
                .get("show_pinned_0")
                .map(|held| held.root.as_str()),
            Some("/repo"),
            "a client error leaves the request for another client"
        );
        let answer = json!({"jsonrpc": "2.0", "id": "show_pinned_0", "result": {}});
        supervisor.handle_client_line(7, answer).await;
        assert!(supervisor.worker_requests.is_empty());
        Ok(())
    }
}
