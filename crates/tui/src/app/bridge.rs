use std::collections::HashSet;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use tokio::sync::broadcast::error::RecvError;
use yi_runtime::{AgentSession, SubagentHost, session::user_input};
use yi_types::event::AgentEvent;

use super::{AskRequest, Command, UiEvent};

/// One bus into the UI queue, the parent's or a child's. A lagged receiver reports the gap
/// and keeps going, as the ACP forwarder does; only a closed bus or a gone UI ends it.
pub async fn forward(
    mut events: tokio::sync::broadcast::Receiver<AgentEvent>,
    child_id: Option<String>,
    ui: Sender<UiEvent>,
) {
    loop {
        let ui_event = match events.recv().await {
            Ok(event) => match &child_id {
                None => UiEvent::Agent(event),
                Some(child_id) => UiEvent::Child {
                    child_id: child_id.clone(),
                    event,
                },
            },
            Err(RecvError::Lagged(dropped)) => {
                let whose = child_id.as_deref().unwrap_or("this session");
                UiEvent::Reply(crate::port::Reply::Notice(format!(
                    "the display fell {dropped} events behind {whose} and skipped them; the roster catches it up"
                )))
            }
            Err(RecvError::Closed) => break,
        };
        if ui.send(ui_event).is_err() {
            break;
        }
    }
}

type Bridge = (
    Receiver<UiEvent>,
    tokio::sync::mpsc::UnboundedSender<Command>,
    std::thread::JoinHandle<()>,
);

/// Every session call happens inside the runtime context; `spawn_run` needs it.
pub(crate) fn spawn_runtime_bridge(
    runtime: tokio::runtime::Runtime,
    session: &Arc<AgentSession>,
    host: &Arc<SubagentHost>,
    ask_rx: Receiver<AskRequest>,
    roster_every: Duration,
) -> Bridge {
    let (ui_tx, ui_rx) = std::sync::mpsc::channel::<UiEvent>();
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<Command>();
    let handle = runtime.handle().clone();

    handle.spawn(forward(session.subscribe(), None, ui_tx.clone()));

    let ask_tx = ui_tx.clone();
    std::thread::spawn(move || {
        for ask in ask_rx {
            if ask_tx.send(UiEvent::Ask(ask)).is_err() {
                break;
            }
        }
    });

    let roster_tx = ui_tx.clone();
    let roster_host = Arc::clone(host);
    let roster_handle = handle.clone();
    handle.spawn(async move {
        let mut seen: HashSet<String> = HashSet::new();
        let mut interval = tokio::time::interval(roster_every);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let children = roster_host.children_view();
            for child in &children {
                let child_id = child.update.id.as_str().to_owned();
                if !seen.insert(child_id.clone()) {
                    continue;
                }
                roster_handle.spawn(forward(
                    child.session.subscribe(),
                    Some(child_id),
                    roster_tx.clone(),
                ));
            }
            if roster_tx.send(UiEvent::Children(children)).is_err() {
                break;
            }
        }
    });

    let driver_session = Arc::clone(session);
    let driver_host = Arc::clone(host);
    let reply_tx = ui_tx.clone();
    let runtime_thread =
        std::thread::spawn(move || {
            runtime.block_on(async move {
                while let Some(command) = cmd_rx.recv().await {
                    match command {
                        Command::Prompt(text) => {
                            let _ = driver_session.prompt_message(user_input(&text));
                        }
                        Command::Steer(text) => driver_session.steer_message(user_input(&text)),
                        Command::SummarizeBranch(stub) => {
                            let session = Arc::clone(&driver_session);
                            tokio::spawn(async move {
                                yi_runtime::summarize_branch(&session, stub).await
                            });
                        }
                        Command::Slash(line) => {
                            crate::port::slash_off_thread(&driver_session, line, &reply_tx);
                        }
                        Command::StopChild(child_id) => {
                            if let Some(child) = crate::port::child_of(&driver_host, &child_id) {
                                child.session.abort();
                            }
                        }
                        Command::ChildHistory(child_id) => {
                            if let Some(child) = crate::port::child_of(&driver_host, &child_id) {
                                let entries = crate::port::branch_of(&child.session);
                                let _ = reply_tx.send(UiEvent::Reply(
                                    crate::port::Reply::ChildHistory { child_id, entries },
                                ));
                            }
                        }
                        Command::Abort => driver_session.abort(),
                        Command::Shutdown => break,
                    }
                }
            });
        });
    (ui_rx, cmd_tx, runtime_thread)
}
