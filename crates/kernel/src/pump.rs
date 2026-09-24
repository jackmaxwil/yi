use std::sync::Arc;

use serde_json::{Map, Value, json};
use tokio::sync::mpsc;
use yi_types::kernel::{ConnectionInfo, JupyterMessage};
use zeromq::{Socket, SocketRecv, SocketSend};

use crate::AGENT_MESSAGE_DISPLAY_MIME;
use crate::HOST_COMM_TARGET;
use crate::client::{
    ExecuteError, Inner, Lifecycle, dispatch_late_agent_message, frames_of, now_iso, trim_tail,
    zmq_message,
};
use crate::framing::{decode, encode};
use crate::journal::record_orphan_process_state;
use crate::reduce::{Reduction, StreamChunk, parent_msg_id, reduce};

pub(crate) struct ChildTasks {
    pub(crate) kill_tx: mpsc::UnboundedSender<()>,
    pub(crate) monitor: tokio::task::JoinHandle<()>,
    pub(crate) stderr: tokio::task::JoinHandle<()>,
}

pub(crate) fn spawn_child_tasks(
    inner: &Arc<Inner>,
    mut child: tokio::process::Child,
) -> ChildTasks {
    let stderr = child.stderr.take();
    let stderr_task = {
        let inner = Arc::clone(inner);
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let Some(mut stderr) = stderr else { return };
            let mut buffer = [0_u8; 4_096];
            while let Ok(read) = stderr.read(&mut buffer).await {
                if read == 0 {
                    break;
                }
                if let Ok(mut tail) = inner.kernel_stderr.lock() {
                    tail.push_str(&String::from_utf8_lossy(&buffer[..read]));
                    trim_tail(&mut tail);
                }
            }
        })
    };
    let (kill_tx, mut kill_rx) = mpsc::unbounded_channel::<()>();
    let monitor = {
        let inner = Arc::clone(inner);
        tokio::spawn(async move {
            let mut kill_confirmed = false;
            loop {
                tokio::select! {
                    status = child.wait() => {
                        if let Ok(mut exited) = inner.exited.lock() {
                            *exited = true;
                        }
                        inner.exit_notify.notify_waiters();
                        // The journal record flips inactive only on a confirmed kill: a wrong
                        // inactive write could mask a reused pid (design K8).
                        if let Some(pid) = child.id().or_else(|| inner.child_pid.lock().ok().and_then(|slot| *slot))
                            && kill_confirmed {
                                record_orphan_process_state(pid, false, now_iso());
                            }
                        if inner.state() != Lifecycle::Shutdown {
                            let detail = status
                                .map(|status| status.to_string())
                                .unwrap_or_else(|error| error.to_string());
                            inner.diagnostic(&format!("unexpected exit {detail}"));
                            inner.set_state(Lifecycle::Shutdown);
                            inner.cleanup_resources();
                        }
                        break;
                    }
                    signal = kill_rx.recv() => {
                        if signal.is_none() {
                            continue;
                        }
                        if child.start_kill().is_ok() {
                            kill_confirmed = true;
                        }
                    }
                }
            }
        })
    };
    ChildTasks {
        kill_tx,
        monitor,
        stderr: stderr_task,
    }
}

pub(crate) async fn connect_sockets(
    info: &ConnectionInfo,
) -> Result<
    (
        zeromq::DealerSocket,
        zeromq::SubSocket,
        zeromq::DealerSocket,
    ),
    String,
> {
    let endpoint = |port: u32| format!("{}://{}:{port}", info.transport, info.ip);
    let mut shell = zeromq::DealerSocket::new();
    let mut iopub = zeromq::SubSocket::new();
    let mut control = zeromq::DealerSocket::new();
    shell
        .connect(&endpoint(info.shell_port))
        .await
        .map_err(|error| format!("shell connect: {error}"))?;
    iopub
        .connect(&endpoint(info.iopub_port))
        .await
        .map_err(|error| format!("iopub connect: {error}"))?;
    control
        .connect(&endpoint(info.control_port))
        .await
        .map_err(|error| format!("control connect: {error}"))?;
    iopub
        .subscribe("")
        .await
        .map_err(|error| format!("iopub subscribe: {error}"))?;
    Ok((shell, iopub, control))
}

pub(crate) fn spawn_control_task(
    inner: &Arc<Inner>,
    mut control: zeromq::DealerSocket,
    mut control_rx: mpsc::UnboundedReceiver<Vec<Vec<u8>>>,
) -> tokio::task::JoinHandle<()> {
    let inner = Arc::clone(inner);
    tokio::spawn(async move {
        loop {
            tokio::select! {
                outbound = control_rx.recv() => {
                    let Some(frames) = outbound else { break };
                    if let Some(message) = zmq_message(frames)
                        && control.send(message).await.is_err() {
                            break;
                        }
                }
                incoming = control.recv() => {
                    let Ok(message) = incoming else {
                        if inner.state() != Lifecycle::Shutdown {
                            inner.diagnostic("control pump failed");
                        }
                        break;
                    };
                    let Some(decoded) = decode(&frames_of(message)) else { continue };
                    let Some(parent) = parent_msg_id(&decoded).map(str::to_owned) else { continue };
                    let sender = inner.pending_control.lock().ok().and_then(|mut pending| {
                        let matches = pending
                            .get(&parent)
                            .is_some_and(|(msg_type, _)| *msg_type == decoded.header.msg_type);
                        if matches { pending.remove(&parent) } else { None }
                    });
                    if let Some((_, tx)) = sender {
                        let _ = tx.send(());
                    }
                }
            }
        }
    })
}

pub(crate) fn spawn_iopub_task(
    inner: &Arc<Inner>,
    mut iopub: zeromq::SubSocket,
) -> tokio::task::JoinHandle<()> {
    let inner = Arc::clone(inner);
    tokio::spawn(async move {
        loop {
            let incoming = iopub.recv().await;
            let Ok(message) = incoming else {
                if inner.state() != Lifecycle::Shutdown {
                    inner.diagnostic("iopub pump failed");
                    inner.reject_active(ExecuteError::Failed(
                        "Kernel IOPub channel failed".to_owned(),
                    ));
                }
                break;
            };
            let Some(decoded) = decode(&frames_of(message)) else {
                continue;
            };
            // Comms dispatch before the parent-header filter: a detached task's host request
            // must dispatch with no active execution (design K7).
            match decoded.header.msg_type.as_str() {
                "comm_open" | "comm_msg" | "comm_close" => handle_comm(&inner, &decoded),
                _ => handle_execution_message(&inner, &decoded),
            }
        }
    })
}

// Shell replies are never read by callers; drain them in a background task or
// the receive queue grows unboundedly (design K4).
pub(crate) fn spawn_shell_task(
    mut shell: zeromq::DealerSocket,
    mut shell_rx: mpsc::UnboundedReceiver<Vec<Vec<u8>>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                outbound = shell_rx.recv() => {
                    let Some(frames) = outbound else { break };
                    if let Some(message) = zmq_message(frames)
                        && shell.send(message).await.is_err() {
                            break;
                        }
                }
                incoming = shell.recv() => {
                    if incoming.is_err() {
                        break;
                    }
                }
            }
        }
    })
}

pub(crate) fn handle_execution_message(inner: &Arc<Inner>, message: &JupyterMessage) {
    let parent = parent_msg_id(message).map(str::to_owned);
    let mut done = false;
    let mut late_payload = None;
    if let Ok(mut active) = inner.active.lock() {
        let matches = active
            .as_ref()
            .is_some_and(|current| parent.as_deref() == Some(current.cell.request_msg_id.as_str()));
        if !matches {
            if matches!(
                message.header.msg_type.as_str(),
                "display_data" | "update_display_data"
            ) {
                late_payload = message
                    .content
                    .get("data")
                    .and_then(|data| data.get(AGENT_MESSAGE_DISPLAY_MIME))
                    .cloned();
            }
        } else if let Some(current) = active.as_mut() {
            if current.settled
                && matches!(
                    message.header.msg_type.as_str(),
                    "display_data" | "update_display_data"
                )
            {
                late_payload = message
                    .content
                    .get("data")
                    .and_then(|data| data.get(AGENT_MESSAGE_DISPLAY_MIME))
                    .cloned();
            }
            if late_payload.is_none() {
                let mut stream_sink = current.on_stream.take();
                let reduction = {
                    let mut callback = stream_sink
                        .as_mut()
                        .map(|sink| move |chunk: StreamChunk<'_>| sink(chunk.name, chunk.text));
                    reduce(
                        &mut current.cell,
                        message,
                        callback
                            .as_mut()
                            .map(|callback| callback as &mut dyn FnMut(StreamChunk<'_>)),
                    )
                };
                current.on_stream = stream_sink;
                done = reduction == Reduction::Done;
            }
        }
    }
    if let Some(payload) = late_payload {
        let dispatched =
            dispatch_late_agent_message(&inner.late_handlers, parent.as_deref(), Some(&payload));
        if dispatched {
            return;
        }
    }
    if done {
        inner.resolve_active(parent.as_deref(), true, None);
    }
}

pub(crate) fn handle_comm(inner: &Arc<Inner>, message: &JupyterMessage) {
    let Some(comm_id) = message.content.get("comm_id").and_then(Value::as_str) else {
        return;
    };
    match message.header.msg_type.as_str() {
        "comm_close" => {
            if let Ok(mut targets) = inner.comm_targets.lock() {
                targets.remove(comm_id);
            }
            // Incident: an abandoned `rlm.receive` polled on for 300 s and lost the mail it took.
            if let Ok(mut in_flight) = inner.in_flight_host.lock() {
                in_flight.retain(|(id, task)| {
                    if id == comm_id {
                        task.abort();
                    }
                    id != comm_id
                });
            }
            if let Ok(mut handled) = inner.handled_host_comm_ids.lock() {
                handled.remove(comm_id);
            }
        }
        "comm_open" => {
            let Some(target) = message.content.get("target_name").and_then(Value::as_str) else {
                return;
            };
            if let Ok(mut targets) = inner.comm_targets.lock() {
                targets.insert(comm_id.to_owned(), target.to_owned());
            }
            if target == HOST_COMM_TARGET {
                start_host_request(inner, comm_id, message.content.get("data"));
            }
        }
        "comm_msg" => {
            let is_host = inner.comm_targets.lock().ok().is_some_and(|targets| {
                targets.get(comm_id).map(String::as_str) == Some(HOST_COMM_TARGET)
            });
            if is_host {
                start_host_request(inner, comm_id, message.content.get("data"));
            }
        }
        _ => {}
    }
}

fn start_host_request(inner: &Arc<Inner>, comm_id: &str, data: Option<&Value>) {
    {
        let Ok(mut handled) = inner.handled_host_comm_ids.lock() else {
            return;
        };
        // One dispatch per comm id: the Python shim's comm_open carries the
        // payload and a duplicate comm_msg must not double-dispatch (design K7).
        if !handled.insert(comm_id.to_owned()) {
            return;
        }
    }
    let payload = data.and_then(Value::as_object).cloned();
    let inner_task = Arc::clone(inner);
    let comm_id = comm_id.to_owned();
    let comm_id_owned = comm_id.clone();
    let task = tokio::spawn(async move {
        let reply = dispatch_host_request(&inner_task, payload).await;
        let data = match reply {
            Ok(mut result) => {
                let mut envelope = Map::new();
                envelope.insert("status".to_owned(), Value::String("ok".to_owned()));
                envelope.append(&mut result);
                // The envelope owns `status`; a handler result must not overwrite it.
                envelope.insert("status".to_owned(), Value::String("ok".to_owned()));
                envelope
            }
            Err(error) => {
                inner_task.diagnostic(&format!("host request failed for comm {comm_id}: {error}"));
                let mut envelope = Map::new();
                envelope.insert("status".to_owned(), Value::String("error".to_owned()));
                envelope.insert("error".to_owned(), Value::String(error));
                envelope
            }
        };
        let content = json!({"comm_id": comm_id, "data": data})
            .as_object()
            .cloned()
            .unwrap_or_default();
        let reply_send = inner_task
            .connection
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .and_then(|connection| {
                let message = inner_task.build("comm_msg", content).ok()?;
                // Replies go on the control channel so a busy shell cannot
                // starve them (design K7).
                Some(inner_task.send_control(encode(&message, &connection.key)))
            });
        if !matches!(reply_send, Some(Ok(()))) {
            inner_task.diagnostic(&format!(
                "failed to send host request reply for comm {comm_id}"
            ));
        }
    });
    if let Ok(mut in_flight) = inner.in_flight_host.lock() {
        in_flight.retain(|(_, task)| !task.is_finished());
        in_flight.push((comm_id_owned, task));
    }
}

async fn dispatch_host_request(
    inner: &Arc<Inner>,
    payload: Option<Map<String, Value>>,
) -> Result<Map<String, Value>, String> {
    let mut payload = payload.ok_or("host request payload must be an object")?;
    let request_type = payload
        .get("type")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or("host request payload must have a string type")?;
    // Tag the request with the cell that triggered it: a blocking call is the in-flight
    // execution, and a detached spawn falls back to the last cell's source.
    let cell_source = inner
        .active
        .lock()
        .ok()
        .and_then(|active| active.as_ref().map(|current| current.cell.code.clone()))
        .or_else(|| {
            inner
                .last_cell_code
                .lock()
                .ok()
                .and_then(|last| last.clone())
        });
    if let Some(source) = cell_source {
        payload.insert("cellSourceCode".to_owned(), Value::String(source));
    }
    let host = inner.host.as_ref().ok_or_else(|| {
        format!("host request type \"{request_type}\" is not available in this session")
    })?;
    match host.dispatch(&request_type, payload) {
        Some(future) => future.await,
        None => Err(format!(
            "host request type \"{request_type}\" is not available in this session"
        )),
    }
}
