//! The permission bridge: a worker's ask reaches the attached client as a JSON-RPC request,
//! and the client's answer wakes the asker. Split from `lib` at the worker-replacement seam.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use yi_runtime::{AskOutcome, Asker};
use yi_types::acp::{
    AcpDiffChange, AcpDiffOperation, AcpDiffPatch, AcpFrame, AcpPatchFormat, AcpPermissionOption,
    AcpPermissionOptionKind, AcpPermissionOutcome, AcpPermissionParams, AcpPermissionSubject,
    AcpToolCallUpdate, AcpToolContent,
};
use yi_types::permission::{choices, chosen};

use crate::LineSink;

/// `None` once stdin has ended: no answer can come, so an ask rejects instead of waiting.
pub(crate) type PendingAsks = Arc<Mutex<Option<HashMap<String, std::sync::mpsc::Sender<Value>>>>>;

fn permission_options(grants: &[yi_runtime::Grant]) -> Vec<AcpPermissionOption> {
    use AcpPermissionOptionKind::{AllowAlways, AllowOnce, RejectOnce};
    let labels = grants.iter().map(|grant| grant.label.as_str());
    choices(labels, AllowOnce, |_| AllowAlways, RejectOnce)
        .into_iter()
        .map(|(option_id, name, kind)| AcpPermissionOption {
            option_id,
            name,
            kind,
        })
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
        let (sender, receiver) = std::sync::mpsc::channel();
        let open = (pending.lock().ok())
            .and_then(|mut map| map.as_mut().map(|map| map.insert(id.clone(), sender)))
            .is_some();
        if !open {
            return AskOutcome::Reject;
        }
        let params = AcpPermissionParams {
            session_id: session_id.clone(),
            title: ask.title.to_owned(),
            description: Some(ask.description.to_owned()),
            options: permission_options(ask.grants),
            subject: ask.patch.map(|patch| AcpPermissionSubject::ToolCall {
                tool_call: Box::new(AcpToolCallUpdate {
                    tool_call_id: ask.tool_call_id.unwrap_or_default().to_owned(),
                    content: Some(vec![AcpToolContent::Diff {
                        changes: ask.changes.iter().map(|path| diff_change(path)).collect(),
                        patch: Some(AcpDiffPatch {
                            format: AcpPatchFormat::GitPatch,
                            text: patch.to_owned(),
                        }),
                    }]),
                    ..AcpToolCallUpdate::default()
                }),
            }),
        };
        let request = AcpFrame {
            jsonrpc: "2.0".to_owned(),
            id: Some(Value::String(id.clone())),
            method: "session/request_permission".to_owned(),
            params: serde_json::to_value(&params).ok(),
        };
        sink(&json!(request));
        let response = receiver.recv();
        if let Some(map) = pending.lock().ok().as_mut().and_then(|map| map.as_mut()) {
            map.remove(&id);
        }
        let Ok(response) = response else {
            return AskOutcome::Reject;
        };
        let outcome = response
            .get("result")
            .and_then(|result| result.get("outcome"))
            .cloned()
            .unwrap_or(Value::Null);
        match serde_json::from_value::<AcpPermissionOutcome>(outcome) {
            Ok(AcpPermissionOutcome::Selected { option_id }) => chosen(
                &option_id,
                AskOutcome::AllowOnce,
                AskOutcome::AllowAlways,
                AskOutcome::Reject,
            ),
            _ => AskOutcome::Reject,
        }
    })
}

/// Invariant: an ask precedes the write it gates, so what is on disk is the pre-image.
fn diff_change(path: &std::path::Path) -> AcpDiffChange {
    let operation = if path.exists() {
        AcpDiffOperation::Modify
    } else {
        AcpDiffOperation::Add
    };
    AcpDiffChange {
        operation,
        path: path.to_string_lossy().into_owned(),
    }
}
