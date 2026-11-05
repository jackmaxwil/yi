use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::{Map, Value, json};
use yi_runtime::session_store::{
    CreateOptions, EntryOrder, EntryQuery, ForkPosition, ForkScope, JsonlRepo, LogOptions,
    SessionRepo, SharedSession, lock_session,
};
use yi_runtime::{AgentSession, Status, available_models, resolve_model};
use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, UserContent};
use yi_types::model::Model;

fn levels_of(model: &Model) -> Vec<String> {
    model
        .supported_efforts()
        .into_iter()
        .map(|effort| effort.to_string())
        .collect()
}

pub struct RpcOptions {
    pub session_dir: PathBuf,
    pub cwd: PathBuf,
}

fn write_line(value: &Value) {
    let mut stdout = std::io::stdout().lock();
    if serde_json::to_writer(&mut stdout, value).is_ok() {
        let _stdout_gone_means_exit = stdout.write_all(b"\n");
        let _flush = stdout.flush();
    }
}

fn ok_frame(id: Option<&str>, command: &str) -> Value {
    let mut frame = Map::new();
    if let Some(id) = id {
        frame.insert("id".to_owned(), json!(id));
    }
    frame.insert("type".to_owned(), json!("response"));
    frame.insert("command".to_owned(), json!(command));
    frame.insert("success".to_owned(), json!(true));
    Value::Object(frame)
}

fn data_frame(id: Option<&str>, command: &str, data: Value) -> Value {
    let mut frame = ok_frame(id, command);
    if let Some(map) = frame.as_object_mut() {
        map.insert("data".to_owned(), data);
    }
    frame
}

fn error_frame(id: Option<&str>, command: &str, error: &str) -> Value {
    let mut frame = Map::new();
    if let Some(id) = id {
        frame.insert("id".to_owned(), json!(id));
    }
    frame.insert("type".to_owned(), json!("response"));
    frame.insert("command".to_owned(), json!(command));
    frame.insert("success".to_owned(), json!(false));
    frame.insert("error".to_owned(), json!(error));
    Value::Object(frame)
}

fn message_text(message: &AgentMessage) -> String {
    let blocks = match message {
        AgentMessage::User {
            content: UserContent::Text(text),
            ..
        } => return text.clone(),
        AgentMessage::User {
            content: UserContent::Blocks(blocks),
            ..
        } => blocks,
        AgentMessage::Assistant { content, .. } => content,
        _ => return String::new(),
    };
    yi_types::message::join_text(blocks, "")
}

struct RpcState {
    session: AgentSession,
    repo: Mutex<JsonlRepo>,
    store: Mutex<SharedSession>,
    session_id: Mutex<String>,
    compacts_in_flight: AtomicUsize,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl RpcState {
    fn shared(&self) -> SharedSession {
        Arc::clone(&locked(&self.store))
    }

    fn store_value<T>(
        &self,
        read: impl FnOnce(&yi_runtime::session_store::SessionStore) -> T,
    ) -> T {
        read(&lock_session(&self.shared()))
    }

    fn adopt(&self, store: SharedSession) -> Result<(), String> {
        let id = lock_session(&store).metadata().id.clone();
        self.session.reset();
        self.session
            .attach_store(Arc::clone(&store))
            .map_err(|error| error.to_string())?;
        *locked(&self.store) = store;
        *locked(&self.session_id) = id;
        Ok(())
    }

    async fn submit_plan(
        &self,
        service: &Arc<yi_runtime::plan::PlanService>,
        payload: &Map<String, Value>,
    ) -> Result<Value, String> {
        let confirmer =
            self.session
                .permission_broker()
                .map(|broker| yi_runtime::plan::authority::Confirmer {
                    broker,
                    store: self.shared(),
                });
        let (service, payload) = (Arc::clone(service), payload.clone());
        tokio::task::spawn_blocking(move || {
            yi_runtime::plan::authority::submit_request(&service, &payload, confirmer.as_ref())
        })
        .await
        .map_err(|error| format!("plan.submit task failed: {error}"))?
    }

    async fn handle(
        &self,
        command_type: &str,
        id: Option<&str>,
        payload: &Map<String, Value>,
    ) -> Value {
        let text_arg = |key: &str| payload.get(key).and_then(Value::as_str).unwrap_or("");
        match command_type {
            "compact" => {
                let outcome = self.session.compact_now().await;
                let running = self.session.status() == Status::Running;
                data_frame(id, "compact", compact_frame(&outcome, running))
            }
            "compact_status" => match self.session.compactor() {
                Some(compactor) => {
                    let status = compactor.status(&self.session.messages(), &self.session.model());
                    data_frame(
                        id,
                        "compact_status",
                        json!({
                            "tokens": status.tokens,
                            "contextWindow": status.context_window,
                            "percent": status.percent,
                            "scheduled": status.scheduled,
                        }),
                    )
                }
                None => error_frame(id, "compact_status", "auto-compaction is not enabled"),
            },
            "prompt" => {
                let message = yi_runtime::session::user_input(text_arg("message"));
                if self.session.prompt_message(message.clone()).is_err() {
                    if text_arg("streamingBehavior") == "steer" {
                        self.session.steer_message(message);
                    } else {
                        self.session.follow_up_message(message);
                    }
                }
                ok_frame(id, "prompt")
            }
            "heartbeat" => {
                let input = text_arg("command");
                match self.session.heartbeat_service() {
                    Some(service) => match service.run(input) {
                        Ok(text) => data_frame(id, "heartbeat", json!({"text": text})),
                        Err(error) => error_frame(id, "heartbeat", &error),
                    },
                    None => error_frame(id, "heartbeat", "no scheduler is attached"),
                }
            }
            "goal" => match self.session.goal_service() {
                Some(service) => {
                    let payload = payload.clone();
                    let outcome = tokio::task::spawn_blocking(move || service.act(&payload))
                        .await
                        .unwrap_or_else(|error| Err(format!("goal task failed: {error}")));
                    match outcome {
                        Ok(goal) => data_frame(id, "goal", json!({"goal": goal})),
                        Err(error) => error_frame(id, "goal", &error),
                    }
                }
                None => error_frame(id, "goal", "no goal service is attached"),
            },
            "plan" => match self.session.plan_service() {
                Some(service) => {
                    let outcome = match text_arg("action") {
                        "get" => {
                            let service = std::sync::Arc::clone(&service);
                            match tokio::task::spawn_blocking(move || service.get()).await {
                                Ok(outcome) => outcome.map(|plan| json!({"plan": plan})),
                                Err(error) => Err(format!("plan.get task failed: {error}")),
                            }
                        }
                        "submit" => self.submit_plan(&service, payload).await,
                        other => Err(format!("unknown plan action {other}; use get|submit")),
                    };
                    match outcome {
                        Ok(data) => data_frame(id, "plan", data),
                        Err(error) => error_frame(id, "plan", &error),
                    }
                }
                None => error_frame(id, "plan", "no plan service is attached"),
            },
            "advisor_stats" => match self.session.advisor() {
                Some(advisor) => data_frame(id, "advisor_stats", json!({"text": advisor.stats()})),
                None => error_frame(id, "advisor_stats", "the advisor is not attached"),
            },
            "steer" => {
                self.session
                    .steer_message(yi_runtime::session::user_input(text_arg("message")));
                ok_frame(id, "steer")
            }
            "follow_up" => {
                self.session
                    .follow_up_message(yi_runtime::session::user_input(text_arg("message")));
                ok_frame(id, "follow_up")
            }
            "abort" => {
                self.session.abort();
                ok_frame(id, "abort")
            }
            "new_session" => {
                let options = CreateOptions {
                    parent_session_id: payload
                        .get("parentSession")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    ..CreateOptions::default()
                };
                let created = locked(&self.repo)
                    .create(options)
                    .map_err(|error| error.to_string());
                match created {
                    Ok(store) => match self.adopt(store) {
                        Ok(()) => data_frame(id, "new_session", json!({"cancelled": false})),
                        Err(error) => error_frame(id, "new_session", &error),
                    },
                    Err(error) => error_frame(id, "new_session", &error),
                }
            }
            "get_state" => {
                let model = self.session.model();
                let (name, file) = self.store_value(|store| {
                    (
                        store.name(),
                        store.file_path().map(|path| path.display().to_string()),
                    )
                });
                data_frame(
                    id,
                    "get_state",
                    json!({
                        "model": model,
                        "thinkingLevel": self.session.effort().to_string(),
                        "isStreaming": self.session.status() == Status::Running,
                        "isCompacting": self
                            .session
                            .compactor()
                            .is_some_and(|compactor| compactor.compacting()),
                        "sessionId": locked(&self.session_id).clone(),
                        "sessionFile": file,
                        "sessionName": name,
                        "autoCompactionEnabled": self.session.compactor().is_some(),
                        "messageCount": self.session.messages().len(),
                        "pendingMessageCount": self.session.pending_count(),
                    }),
                )
            }
            "set_model" => {
                let provider = text_arg("provider");
                let model_id = text_arg("modelId");
                match resolve_model(provider, model_id) {
                    Some(model) => {
                        self.session.set_model(model.clone());
                        data_frame(id, "set_model", json!(model))
                    }
                    None => error_frame(
                        id,
                        "set_model",
                        &format!("unknown model {provider}/{model_id}"),
                    ),
                }
            }
            "get_available_models" => data_frame(
                id,
                "get_available_models",
                json!({"models": available_models()}),
            ),
            "set_thinking_level" => match text_arg("level").parse() {
                Ok(effort) => {
                    let effective = self.session.set_effort(effort);
                    data_frame(
                        id,
                        "set_thinking_level",
                        json!({"level": effective.to_string()}),
                    )
                }
                Err(error) => error_frame(id, "set_thinking_level", &format!("{error}")),
            },
            "get_available_thinking_levels" => data_frame(
                id,
                "get_available_thinking_levels",
                json!({"levels": levels_of(&self.session.model())}),
            ),
            "set_steering_mode" => error_frame(
                id,
                "set_steering_mode",
                "this build delivers every steer as it arrives; there is no mode to set",
            ),
            "set_follow_up_mode" => error_frame(
                id,
                "set_follow_up_mode",
                "this build queues every follow-up as it arrives; there is no mode to set",
            ),
            "get_messages" => data_frame(
                id,
                "get_messages",
                json!({"messages": self.session.messages()}),
            ),
            "get_entries" => {
                let after_seq = payload
                    .get("since")
                    .and_then(Value::as_str)
                    .and_then(|since| {
                        self.store_value(|store| store.entry(since).map(|entry| entry.seq()))
                    });
                let (entries, leaf) = self.store_value(|store| {
                    let entries = store.find_entries(&EntryQuery {
                        order: EntryOrder::OldestFirst,
                        after_seq,
                        ..EntryQuery::default()
                    });
                    (entries, store.leaf_id("main"))
                });
                match (entries, leaf) {
                    (Ok(entries), Ok(leaf)) => data_frame(
                        id,
                        "get_entries",
                        json!({"entries": entries, "leafId": leaf}),
                    ),
                    (Err(error), _) | (_, Err(error)) => {
                        error_frame(id, "get_entries", &error.to_string())
                    }
                }
            }
            "get_session_stats" => data_frame(
                id,
                "get_session_stats",
                json!(self.store_value(|store| store.stats())),
            ),
            "set_session_name" => {
                let name = Some(text_arg("name").to_owned()).filter(|name| !name.is_empty());
                match lock_session(&self.shared()).set_name(name) {
                    Ok(()) => ok_frame(id, "set_session_name"),
                    Err(error) => error_frame(id, "set_session_name", &error.to_string()),
                }
            }
            "get_last_assistant_text" => {
                let text = self
                    .session
                    .messages()
                    .iter()
                    .rev()
                    .find(|message| matches!(message, AgentMessage::Assistant { .. }))
                    .map(message_text);
                data_frame(id, "get_last_assistant_text", json!({"text": text}))
            }
            "get_commands" => data_frame(
                id,
                "get_commands",
                json!({"commands": yi_runtime::slash::SESSION_VERBS}),
            ),
            "switch_session" => {
                let path = PathBuf::from(text_arg("sessionPath"));
                match yi_runtime::session_store::load_session(&path) {
                    Ok(store) => match self.adopt(Arc::new(std::sync::Mutex::new(store))) {
                        Ok(()) => data_frame(id, "switch_session", json!({"cancelled": false})),
                        Err(error) => error_frame(id, "switch_session", &error),
                    },
                    Err(error) => error_frame(id, "switch_session", &error.to_string()),
                }
            }
            "fork" => {
                let entry_id = text_arg("entryId").to_owned();
                let text = self.store_value(|store| {
                    store
                        .entry(&entry_id)
                        .and_then(|entry| match entry {
                            Entry::Message { message, .. } => Some(message_text(&message)),
                            _ => None,
                        })
                        .unwrap_or_default()
                });
                let forked = locked(&self.repo).fork(
                    &locked(&self.session_id).clone(),
                    &ForkScope::Branch {
                        entry_id: Some(entry_id),
                        position: Some(ForkPosition::Before),
                    },
                    CreateOptions::default(),
                );
                match forked.map_err(|error| error.to_string()).and_then(|store| {
                    self.adopt(store)?;
                    Ok(())
                }) {
                    Ok(()) => data_frame(id, "fork", json!({"text": text, "cancelled": false})),
                    Err(error) => error_frame(id, "fork", &error),
                }
            }
            "get_log" => {
                let log = self.store_value(|store| store.log(&LogOptions::default()));
                match log {
                    Ok(log) => data_frame(id, "get_log", json!({"log": log})),
                    Err(error) => error_frame(id, "get_log", &error.to_string()),
                }
            }
            other => error_frame(
                id,
                other,
                &format!("unsupported command in this build: {other}"),
            ),
        }
    }
}

pub fn run_rpc(
    session: AgentSession,
    options: &RpcOptions,
    runtime: tokio::runtime::Runtime,
) -> i32 {
    let mut repo = JsonlRepo::new(
        options.session_dir.clone(),
        options.cwd.display().to_string(),
    );
    let store = match repo.create(CreateOptions::default()) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("error: failed to create session: {error}");
            return 1;
        }
    };
    let session_id = lock_session(&store).metadata().id.clone();
    if let Err(error) = session.attach_store(Arc::clone(&store)) {
        eprintln!("error: failed to attach session store: {error}");
        return 1;
    }
    let state = Arc::new(RpcState {
        session,
        repo: Mutex::new(repo),
        store: Mutex::new(store),
        session_id: Mutex::new(session_id),
        compacts_in_flight: AtomicUsize::new(0),
    });

    runtime.block_on(async move {
        let (command_tx, mut command_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        std::thread::spawn(move || {
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                let Ok(line) = line else { break };
                if command_tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut events = state.session.subscribe();
        let mut answers = tokio::task::JoinSet::new();
        let mut ahead: Option<tokio::sync::oneshot::Receiver<()>> = None;
        loop {
            tokio::select! {
                line = command_rx.recv() => {
                    let Some(line) = line else { break };
                    let trimmed = line.trim_end_matches('\r');
                    if trimmed.is_empty() {
                        continue;
                    }
                    let parsed: Result<Value, _> = serde_json::from_str(trimmed);
                    match parsed {
                        Ok(Value::Object(payload)) => {
                            let id = payload.get("id").and_then(Value::as_str).map(str::to_owned);
                            let command_type = payload
                                .get("type")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_owned();
                            if command_type == "compact" {
                                state.compacts_in_flight.fetch_add(1, Ordering::SeqCst);
                            }
                            // Invariant: a state query answers while a compact runs; mutations stay ordered.
                            let bypass = command_type == "get_state"
                                && state.compacts_in_flight.load(Ordering::SeqCst) > 0;
                            let queued = if bypass { None } else { ahead.take() };
                            let (finished, next) = tokio::sync::oneshot::channel();
                            if !bypass {
                                ahead = Some(next);
                            }
                            let answering = Arc::clone(&state);
                            answers.spawn(async move {
                                if let Some(queued) = queued {
                                    let _ = queued.await;
                                }
                                let frame = answering
                                    .handle(&command_type, id.as_deref(), &payload)
                                    .await;
                                if command_type == "compact" {
                                    answering
                                        .compacts_in_flight
                                        .fetch_sub(1, Ordering::SeqCst);
                                }
                                write_line(&frame);
                                if !bypass {
                                    let _ = finished.send(());
                                }
                            });
                        }
                        _ => write_line(&error_frame(None, "", "invalid command: not a JSON object")),
                    }
                }
                event = yi_runtime::next_event(&mut events) => {
                    if !relay(event) {
                        break;
                    }
                }
                _ = answers.join_next(), if !answers.is_empty() => {}
            }
        }
        let mut open = true;
        while !answers.is_empty() {
            tokio::select! {
                _ = answers.join_next() => {}
                event = yi_runtime::next_event(&mut events), if open => {
                    open = relay(event);
                }
            }
        }
        state.session.wait_idle().await;
        loop {
            let value = match events.try_recv() {
                Ok(event) => serde_json::to_value(&event),
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(dropped)) => {
                    serde_json::to_value(yi_types::event::EventGap { dropped })
                }
                Err(_) => break,
            };
            if let Ok(value) = value {
                write_line(&value);
            }
        }
        0
    })
}

fn relay(event: Option<Result<yi_types::event::AgentEvent, yi_types::event::EventGap>>) -> bool {
    let value = match event {
        Some(Ok(event)) => serde_json::to_value(&event),
        Some(Err(gap)) => serde_json::to_value(gap),
        None => return false,
    };
    if let Ok(value) = value {
        write_line(&value);
    }
    true
}

/// An elision applied too, but it is not a summary: it says so, apart from a compaction's
/// `applied`, and a failure carries its reason. A refused `/compact` is still `scheduled`.
fn compact_frame(
    outcome: &Result<yi_runtime::compaction::CompactOutcome, yi_runtime::compaction::CompactError>,
    running: bool,
) -> Value {
    let applied = outcome.as_ref().is_ok_and(|outcome| outcome.applied());
    let busy = matches!(outcome, Ok(yi_runtime::compaction::CompactOutcome::Busy));
    let mut frame = json!({"applied": applied, "scheduled": busy || (!applied && running)});
    match outcome {
        Ok(yi_runtime::compaction::CompactOutcome::Elided(elision)) => {
            frame["elided"] = json!(elision.messages);
        }
        Err(error) => frame["error"] = json!(error.to_string()),
        Ok(_) => {}
    }
    frame
}

#[cfg(test)]
mod tests {
    use yi_runtime::compaction::{CompactError, CompactOutcome, Elision, Tokens};

    #[test]
    fn a_compact_frame_tells_an_elision_and_a_failure_apart_from_a_summary() {
        let frame = |outcome| super::compact_frame(&outcome, false);
        assert_eq!(
            frame(Ok(CompactOutcome::Summarized))["elided"],
            serde_json::Value::Null
        );
        assert_eq!(
            frame(Ok(CompactOutcome::Elided(Elision {
                messages: 5,
                total: 9,
                window: Tokens(1000)
            })))["elided"],
            5
        );
        let busy = frame(Ok(CompactOutcome::Busy));
        assert_eq!(busy["applied"], false);
        assert_eq!(busy["scheduled"], true, "a refused /compact still runs");
        let failed = frame(Err(CompactError::NoSummary("upstream 529".to_owned())));
        assert_eq!(failed["applied"], false);
        assert!(
            failed["error"]
                .as_str()
                .is_some_and(|error| error.contains("upstream 529"))
        );
    }
}
