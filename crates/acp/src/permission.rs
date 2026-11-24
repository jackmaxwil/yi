//! The permission bridge: a worker's ask reaches the attached client as a JSON-RPC request,
//! and the client's answer wakes the asker. Split from `lib` at the worker-replacement seam.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol_schema::v2 as acp;
use serde_json::{Value, json};
use yi_runtime::{AskOutcome, Asker};
use yi_types::acp::{AcpAsker, AcpMeta};
use yi_types::permission::{choices, chosen};

use crate::LineSink;

/// `None` once stdin has ended: no answer can come, so an ask rejects instead of waiting.
pub(crate) type PendingAsks = Arc<Mutex<Option<HashMap<String, std::sync::mpsc::Sender<Value>>>>>;
const POLL: Duration = Duration::from_millis(100);

pub(crate) enum PendingWait {
    UntilStopped,
    Attempts(u32),
}

pub(crate) fn with_pending_ask<F>(
    pending: &PendingAsks,
    id: &str,
    sink: &LineSink,
    request: &Value,
    wait: PendingWait,
    stopped: &F,
) -> Option<Value>
where
    F: Fn() -> bool + ?Sized,
{
    let (sender, receiver) = std::sync::mpsc::channel();
    let mut asks = pending.lock().ok()?;
    asks.as_mut()?.insert(id.to_owned(), sender);
    drop(asks);
    sink(request);
    let response = match wait {
        PendingWait::UntilStopped => loop {
            match receiver.recv_timeout(POLL) {
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) if !stopped() => {}
                other => break other.ok(),
            }
        },
        PendingWait::Attempts(polls) => {
            let mut answer = None;
            for _ in 0..polls {
                answer = receiver.recv_timeout(POLL).ok();
                if answer.is_some() || stopped() {
                    break;
                }
            }
            answer
        }
    };
    if let Ok(mut guard) = pending.lock()
        && let Some(asks) = guard.as_mut()
    {
        asks.remove(id);
    }
    response
}

fn permission_options(grants: &[yi_runtime::Grant]) -> Vec<acp::PermissionOption> {
    use acp::PermissionOptionKind::{AllowAlways, AllowOnce, RejectOnce};
    let labels = grants.iter().map(|grant| grant.label.as_str());
    choices(labels, AllowOnce, |_| AllowAlways, RejectOnce)
        .into_iter()
        .map(|(option_id, name, kind)| acp::PermissionOption::new(option_id, name, kind))
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
        let meta = ask.agent.map(|name| {
            let mut meta = AcpMeta::default();
            meta.yi.asker = Some(AcpAsker {
                child_id: name.to_owned(),
            });
            crate::meta_map(&meta)
        });
        let params = acp::RequestPermissionRequest::new(
            session_id.as_str(),
            ask.title,
            permission_options(ask.grants),
        )
        .description(ask.description)
        .subject(ask.patch.map(|patch| {
            let diff = acp::Diff::patch(
                patch,
                ask.changes.iter().map(|path| diff_change(path)).collect(),
            );
            acp::RequestPermissionSubject::from(
                acp::ToolCallUpdate::new(ask.tool_call_id.unwrap_or_default())
                    .content(vec![acp::ToolCallContent::from(diff)]),
            )
        }))
        .meta(meta);
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/request_permission",
            "params": params,
        });
        let stopped = || ask.stop.is_some_and(|stop| stop());
        let response = with_pending_ask(
            &pending,
            &id,
            &sink,
            &request,
            PendingWait::UntilStopped,
            &stopped,
        );
        let Some(response) = response else {
            sink(&json!({"jsonrpc": "2.0", "method": "_yi/permission_closed",
                "params": {"sessionId": session_id, "requestId": id}}));
            return AskOutcome::Reject;
        };
        let result = response.get("result").cloned().unwrap_or(Value::Null);
        match serde_json::from_value::<acp::RequestPermissionResponse>(result).map(|r| r.outcome) {
            Ok(acp::RequestPermissionOutcome::Selected(selected)) => chosen(
                &selected.option_id.0,
                AskOutcome::AllowOnce,
                AskOutcome::AllowAlways,
                AskOutcome::Reject,
            ),
            _ => AskOutcome::Reject,
        }
    })
}

/// Invariant: an ask precedes the write it gates, so what is on disk is the pre-image.
fn diff_change(path: &std::path::Path) -> acp::DiffChange {
    let path = path.to_string_lossy().into_owned();
    if std::path::Path::new(&path).exists() {
        acp::DiffChange::modify(path)
    } else {
        acp::DiffChange::add(path)
    }
}
