use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;
use yi_loop::interrupt::InterruptSignal;
use yi_loop::{ExecutionMode, LoopConfig, LoopContext, run_loop};
use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, Usage, UserContent};
use yi_types::model::Model;

use crate::provider::ProviderStream;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Idle,
    Running,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionError {
    Busy,
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => formatter.write_str("a run is already in progress"),
        }
    }
}

impl std::error::Error for SessionError {}

pub struct SessionConfig {
    pub system_prompt: String,
    pub model: Model,
    pub thinking_level: Option<String>,
    pub tool_execution: ExecutionMode,
}

struct Shared {
    messages: Mutex<Vec<AgentMessage>>,
    steer: Mutex<Vec<AgentMessage>>,
    follow_up: Mutex<Vec<AgentMessage>>,
    status: Mutex<Status>,
    last_usage: Mutex<Option<Usage>>,
    store: Mutex<Option<yi_session::SharedSession>>,
    store_error: Mutex<Option<String>>,
    events: broadcast::Sender<AgentEvent>,
    idle: tokio::sync::Notify,
    signal: InterruptSignal,
}

fn persist_message(shared: &Shared, message: &AgentMessage) {
    let store = shared
        .store
        .lock()
        .map(|handle| handle.clone())
        .unwrap_or_default();
    if let Some(store) = store
        && let Err(error) = yi_session::lock_session(&store).append_message("main", message.clone())
        && let Ok(mut slot) = shared.store_error.lock()
    {
        *slot = Some(error.to_string());
    }
}

pub struct AgentSession {
    config: SessionConfig,
    provider: Arc<ProviderStream>,
    shared: Arc<Shared>,
    tools: Vec<Arc<dyn yi_loop::AgentTool>>,
}

impl AgentSession {
    pub fn new(config: SessionConfig, provider: Arc<ProviderStream>) -> Self {
        provider.set_thinking_level(config.thinking_level.clone());
        let (events, _) = broadcast::channel(1024);
        Self {
            config,
            provider,
            shared: Arc::new(Shared {
                messages: Mutex::new(Vec::new()),
                steer: Mutex::new(Vec::new()),
                follow_up: Mutex::new(Vec::new()),
                status: Mutex::new(Status::Idle),
                last_usage: Mutex::new(None),
                store: Mutex::new(None),
                store_error: Mutex::new(None),
                events,
                idle: tokio::sync::Notify::new(),
                signal: InterruptSignal::default(),
            }),
            tools: Vec::new(),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.shared.events.subscribe()
    }

    pub fn status(&self) -> Status {
        self.shared
            .status
            .lock()
            .map(|status| *status)
            .unwrap_or(Status::Idle)
    }

    pub fn last_usage(&self) -> Option<Usage> {
        self.shared
            .last_usage
            .lock()
            .ok()
            .and_then(|usage| usage.clone())
    }

    pub fn set_tools(&mut self, tools: Vec<Arc<dyn yi_loop::AgentTool>>) {
        self.tools = tools;
    }

    /// Wires yi-tools implementations through the loop adapter; aborting the
    /// session cancels any tool subprocess still running.
    pub fn use_tools(&mut self, tools: Vec<Arc<dyn yi_tools::Tool>>, cwd: std::path::PathBuf) {
        let adapters = tools
            .into_iter()
            .map(|tool| {
                let shared = Arc::clone(&self.shared);
                let cancelled: yi_tools::CancelFlag = Arc::new(move || shared.signal.is_fired());
                Arc::new(crate::tools::ToolAdapter::new(tool, cwd.clone(), cancelled))
                    as Arc<dyn yi_loop::AgentTool>
            })
            .collect();
        self.tools = adapters;
    }

    /// Attaches a session store: loads the main branch's messages as the
    /// in-memory history, then persists every subsequent MessageEnd to it.
    pub fn attach_store(
        &self,
        store: yi_session::SharedSession,
    ) -> Result<usize, yi_session::SessionError> {
        let loaded: Vec<AgentMessage> = {
            let session = yi_session::lock_session(&store);
            session
                .find_entries_on_branch(
                    "main",
                    &yi_session::EntryQuery {
                        order: yi_session::EntryOrder::OldestFirst,
                        ..yi_session::EntryQuery::default()
                    },
                    &yi_session::BranchBounds::default(),
                )?
                .into_iter()
                .filter_map(|entry| match entry {
                    yi_types::entry::Entry::Message { message, .. } => Some(message),
                    _ => None,
                })
                .collect()
        };
        let count = loaded.len();
        if let Ok(mut messages) = self.shared.messages.lock() {
            *messages = loaded;
        }
        if let Ok(mut slot) = self.shared.store.lock() {
            *slot = Some(store);
        }
        Ok(count)
    }

    pub fn store(&self) -> Option<yi_session::SharedSession> {
        self.shared
            .store
            .lock()
            .map(|handle| handle.clone())
            .unwrap_or_default()
    }

    pub fn store_error(&self) -> Option<String> {
        self.shared
            .store_error
            .lock()
            .map(|slot| slot.clone())
            .unwrap_or_default()
    }

    pub fn steer(&self, text: &str) {
        if let Ok(mut queue) = self.shared.steer.lock() {
            queue.push(user_message(text));
        }
    }

    pub fn follow_up(&self, text: &str) {
        if let Ok(mut queue) = self.shared.follow_up.lock() {
            queue.push(user_message(text));
        }
    }

    pub fn abort(&self) {
        self.shared.signal.fire();
    }

    pub async fn wait_idle(&self) {
        loop {
            if self.status() == Status::Idle {
                return;
            }
            self.shared.idle.notified().await;
        }
    }

    /// Returns at admission; the run streams in a spawned task (R6).
    pub fn prompt(&self, text: &str) -> Result<(), SessionError> {
        {
            let Ok(mut status) = self.shared.status.lock() else {
                return Err(SessionError::Busy);
            };
            if *status == Status::Running {
                return Err(SessionError::Busy);
            }
            *status = Status::Running;
        }
        let shared = Arc::clone(&self.shared);
        let provider = Arc::clone(&self.provider);
        let system_prompt = self.config.system_prompt.clone();
        let model = self.config.model.clone();
        let tool_execution = self.config.tool_execution;
        let tools = self.tools.clone();
        let prompt = user_message(text);
        tokio::spawn(async move {
            let mut context = LoopContext {
                system_prompt,
                messages: shared
                    .messages
                    .lock()
                    .map(|messages| messages.clone())
                    .unwrap_or_default(),
                tools,
            };
            let steer = Arc::clone(&shared);
            let follow = Arc::clone(&shared);
            let mut config = LoopConfig::new(model);
            config.tool_execution = tool_execution;
            config.get_steering_messages = Some(Box::new(move || {
                steer
                    .steer
                    .lock()
                    .map(|mut queue| std::mem::take(&mut *queue))
                    .unwrap_or_default()
            }));
            config.get_follow_up_messages = Some(Box::new(move || {
                follow
                    .follow_up
                    .lock()
                    .map(|mut queue| std::mem::take(&mut *queue))
                    .unwrap_or_default()
            }));
            let emit_shared = Arc::clone(&shared);
            let mut emit = move |event: AgentEvent| {
                if let AgentEvent::MessageEnd { message } = &event {
                    if let AgentMessage::Assistant { usage, .. } = message
                        && let Ok(mut last) = emit_shared.last_usage.lock()
                    {
                        *last = Some(usage.clone());
                    }
                    persist_message(&emit_shared, message);
                }
                let _ = emit_shared.events.send(event);
            };
            run_loop(
                &mut context,
                vec![prompt],
                &config,
                &shared.signal,
                &mut emit,
                provider.as_ref(),
            )
            .await;
            if let Ok(mut messages) = shared.messages.lock() {
                *messages = context.messages;
            }
            if let Ok(mut status) = shared.status.lock() {
                *status = Status::Idle;
            }
            shared.idle.notify_waiters();
        });
        Ok(())
    }

    pub fn messages(&self) -> Vec<AgentMessage> {
        self.shared
            .messages
            .lock()
            .map(|messages| messages.clone())
            .unwrap_or_default()
    }

    pub fn provider(&self) -> &ProviderStream {
        &self.provider
    }
}

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::User {
        content: UserContent::Text(text.to_owned()),
        timestamp: 0,
    }
}
