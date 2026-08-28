use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, Content, StopReason, Usage};
use yi_types::model::Model;
pub use yi_types::subagent::{ChildActivity, ChildId, ChildStatus, ChildUpdate};

use crate::mailbox::{ParentLink, WAIT_MAX_MS, message_params, register_child_messaging};
use crate::provider::{available_models, resolve_model};
use crate::session::AgentSession;

pub const DEFAULT_MAX_DEPTH: u8 = 1;
// A completed child holds its slot until closed: the cap forces the parent to
// reap with rlm.delete_subagent instead of leaking children (design B2).
pub const DEFAULT_MAX_CHILDREN: usize = 8;
pub const PARENT_NAME: &str = "parent";

pub(crate) struct ChildRecord {
    pub(crate) session_name: String,
    session_dir: PathBuf,
    pub(crate) worktree: Option<crate::worktree::Worktree>,
    pub(crate) status: ChildStatus,
    activity: ChildActivity,
    tool_use_count: u64,
    token_count: u64,
    answer_preview: Option<String>,
    pub(crate) error: Option<String>,
    /// B13 wait: reports and terminal transitions the parent has not collected.
    pub(crate) pending: u64,
    pub(crate) session: Arc<AgentSession>,
}

impl ChildRecord {
    fn update(&self, child_id: &str) -> ChildUpdate {
        ChildUpdate {
            id: ChildId(child_id.to_owned()),
            name: self.session_name.clone(),
            status: self.status,
            activity: self.activity,
            tool_use_count: self.tool_use_count,
            token_count: self.token_count,
            answer_preview: self.answer_preview.clone(),
            error: self.error.clone(),
        }
    }
}

/// The session handle is event-stream and store access, not control.
#[derive(Clone)]
pub struct ChildView {
    pub update: ChildUpdate,
    pub session: Arc<AgentSession>,
}

pub struct ChildBuild<'a> {
    pub model: Model,
    pub thinking: Option<String>,
    pub session_dir: &'a Path,
    /// `Some` only for a B11 worktree child; otherwise the parent's own cwd.
    pub cwd: Option<&'a Path>,
    pub link: ParentLink,
}

pub type ChildFactory = dyn Fn(ChildBuild<'_>) -> Result<AgentSession, String> + Send + Sync;
pub type NoticeFn = dyn Fn(&str) + Send + Sync;
pub type AttributeFn = dyn Fn(&Usage) + Send + Sync;

pub struct SubagentHostOptions {
    pub depth: u8,
    pub max_depth: u8,
    pub max_children: usize,
    pub parent_session_dir: PathBuf,
    pub default_model: Model,
    pub factory: Arc<ChildFactory>,
    /// A host status notice, delivered as a user-role message.
    pub notice: Arc<NoticeFn>,
    /// The parent's bus: a child's B7 updates ride it, never the child's own.
    pub events: tokio::sync::broadcast::Sender<AgentEvent>,
    /// The parent's live history, read at spawn for a B5 fork seed.
    pub parent_messages: Arc<dyn Fn() -> Vec<AgentMessage> + Send + Sync>,
    /// The repository a B11 worktree child branches from.
    pub cwd: PathBuf,
    /// A child's B6 report, injected into the parent's own transcript.
    pub report: Arc<dyn Fn(AgentMessage) + Send + Sync>,
    /// Folds a child's billable usage onto the parent's last assistant message.
    pub attribute: Arc<AttributeFn>,
}

pub struct SubagentHost {
    pub(crate) options: SubagentHostOptions,
    pub(crate) children: Mutex<HashMap<String, ChildRecord>>,
}

pub(crate) fn random_suffix() -> Result<String, String> {
    use std::io::Read;
    let mut buffer = [0_u8; 4];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut buffer))
        .map_err(|error| format!("/dev/urandom: {error}"))?;
    let mut out = String::with_capacity(8);
    for byte in buffer {
        out.push_str(&format!("{byte:02x}"));
    }
    Ok(out)
}

fn default_session_name(prompt: &str, child_id: &str) -> String {
    let slug: String = prompt
        .to_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug
        .split('-')
        .filter(|word| !word.is_empty())
        .take(4)
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        format!("subagent-{child_id}")
    } else {
        format!("{slug}-{child_id}")
    }
}

/// B1 seeding; `LastN(n)` counts turn boundaries, not messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fork {
    None,
    All,
    LastN(u64),
}

/// B11: parallel mutators stop sharing one tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isolation {
    None,
    Worktree,
}

fn parse_isolation(kwargs: &Map<String, Value>) -> Result<Isolation, String> {
    match kwargs.get("isolation") {
        None | Some(Value::Null) => Ok(Isolation::None),
        Some(Value::String(value)) => match value.trim() {
            "none" => Ok(Isolation::None),
            "worktree" => Ok(Isolation::Worktree),
            other => Err(format!(
                "rlm.run isolation must be \"none\" or \"worktree\", got {other}"
            )),
        },
        Some(other) => Err(format!("rlm.run isolation must be a string, got {other}")),
    }
}

fn parse_fork(kwargs: &Map<String, Value>) -> Result<Fork, String> {
    match kwargs.get("fork") {
        None | Some(Value::Null) => Ok(Fork::None),
        Some(Value::String(value)) => match value.trim() {
            "none" => Ok(Fork::None),
            "all" => Ok(Fork::All),
            other => other
                .parse::<u64>()
                .ok()
                .filter(|turns| *turns > 0)
                .map(Fork::LastN)
                .ok_or_else(|| {
                    format!("rlm.run fork must be \"none\", \"all\", or a positive turn count, got {other}")
                }),
        },
        Some(Value::Number(number)) => number
            .as_u64()
            .filter(|turns| *turns > 0)
            .map(Fork::LastN)
            .ok_or_else(|| format!("rlm.run fork turn count must be positive, got {number}")),
        Some(other) => Err(format!("rlm.run fork must be a string or number, got {other}")),
    }
}

/// The seed budget is the child's window less the reserve compaction needs.
fn seed_for_fork(parent: &[AgentMessage], fork: Fork, window: u64) -> Vec<AgentMessage> {
    let start = match fork {
        Fork::None => return Vec::new(),
        Fork::All => 0,
        Fork::LastN(turns) => {
            let boundaries: Vec<usize> = parent
                .iter()
                .enumerate()
                .filter(|(_, message)| matches!(message, AgentMessage::User { .. }))
                .map(|(index, _)| index)
                .collect();
            let wanted = usize::try_from(turns).unwrap_or(usize::MAX);
            boundaries
                .len()
                .checked_sub(wanted)
                .and_then(|index| boundaries.get(index).copied())
                .unwrap_or(0)
        }
    };
    let budget = window.saturating_sub(yi_context::Settings::default().reserve_tokens.0);
    let mut kept: Vec<AgentMessage> = Vec::new();
    let mut used = 0_u64;
    for message in parent[start.min(parent.len())..].iter().rev() {
        used = used.saturating_add(yi_context::estimate_message(message).0);
        if used > budget {
            break;
        }
        kept.push(message.clone());
    }
    kept.reverse();
    kept
}

fn require_kwargs(kwargs: &Map<String, Value>) -> Result<(), String> {
    let mut unsupported: Vec<&str> = kwargs
        .keys()
        .map(String::as_str)
        .filter(|key| !matches!(*key, "name" | "model" | "thinking" | "fork" | "isolation"))
        .collect();
    if unsupported.is_empty() {
        return Ok(());
    }
    unsupported.sort_unstable();
    Err(format!(
        "Unsupported rlm.run kwargs: {}",
        unsupported.join(", ")
    ))
}

fn optional_string(kwargs: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match kwargs.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_owned()))
            }
        }
        Some(other) => Err(format!("rlm.run {key} must be a string, got {other}")),
    }
}

fn child_entry(child_id: &str, record: &ChildRecord) -> Value {
    json!({
        "rlm_child_id": child_id,
        "active_session_id": Value::Null,
        "session_id": Value::Null,
        "session_name": record.session_name,
        "session_dir": record.session_dir.to_string_lossy(),
        "status": record.status.as_str(),
    })
}

pub(crate) fn last_assistant_text(messages: &[AgentMessage]) -> Option<String> {
    messages.iter().rev().find_map(|message| match message {
        AgentMessage::Assistant { content, .. } => {
            let text = content
                .iter()
                .filter_map(|content| match content {
                    yi_types::message::Content::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            if text.is_empty() { None } else { Some(text) }
        }
        _ => None,
    })
}

fn preview(text: &str) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() > 240 {
        let capped: String = compact.chars().take(240).collect();
        format!("{capped}…")
    } else {
        compact
    }
}

/// `false` means nothing a client can see moved, so nothing is published.
fn fold_event(record: &mut ChildRecord, event: &AgentEvent) -> bool {
    match event {
        AgentEvent::ToolExecutionStart { .. } => {
            record.tool_use_count = record.tool_use_count.saturating_add(1);
            record.activity = ChildActivity::Executing;
            true
        }
        AgentEvent::ToolExecutionEnd { .. } => {
            record.activity = ChildActivity::Writing;
            true
        }
        AgentEvent::MessageStart {
            message: AgentMessage::Assistant { .. },
        } => {
            record.activity = ChildActivity::Writing;
            true
        }
        AgentEvent::MessageEnd {
            message: AgentMessage::Assistant { usage, content, .. },
        } => {
            let tokens = u64::try_from(usage.total_tokens).unwrap_or(0);
            record.token_count = record.token_count.saturating_add(tokens);
            record.activity = ChildActivity::Waiting;
            let text: String = content
                .iter()
                .filter_map(|block| match block {
                    Content::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            if !text.is_empty() {
                record.answer_preview = Some(preview(&text));
            }
            true
        }
        _ => false,
    }
}

impl SubagentHost {
    pub fn new(options: SubagentHostOptions) -> Self {
        Self {
            options,
            children: Mutex::new(HashMap::new()),
        }
    }

    pub fn children_view(&self) -> Vec<ChildView> {
        self.children
            .lock()
            .map(|children| {
                let mut view: Vec<ChildView> = children
                    .iter()
                    .map(|(id, record)| ChildView {
                        update: record.update(id),
                        session: Arc::clone(&record.session),
                    })
                    .collect();
                view.sort_by(|left, right| left.update.id.cmp(&right.update.id));
                view
            })
            .unwrap_or_default()
    }

    fn publish(&self, child_id: &str) {
        let update = self
            .children
            .lock()
            .ok()
            .and_then(|children| children.get(child_id).map(|record| record.update(child_id)));
        if let Some(update) = update {
            let _ = self.options.events.send(AgentEvent::ChildUpdate { update });
        }
    }

    fn watch(self: &Arc<Self>, child_id: String, session: &Arc<AgentSession>) {
        let host = Arc::clone(self);
        let mut events = session.subscribe();
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(event) => {
                        let moved = host
                            .children
                            .lock()
                            .ok()
                            .and_then(|mut children| {
                                children
                                    .get_mut(&child_id)
                                    .map(|record| fold_event(record, &event))
                            })
                            .unwrap_or(false);
                        if moved {
                            host.publish(&child_id);
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    fn create_child_dir(&self, parent_dir: &Path) -> Result<(PathBuf, String), String> {
        std::fs::create_dir_all(parent_dir).map_err(|error| error.to_string())?;
        for _ in 0..100 {
            let suffix = random_suffix()?;
            let dir = parent_dir.join(format!("sub-{suffix}"));
            match std::fs::create_dir(&dir) {
                Ok(()) => return Ok((dir, format!("sub-{suffix}"))),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(format!("{}: {error}", dir.display())),
            }
        }
        Err("Unable to create unique RLM child session directory".to_owned())
    }

    /// Validates, admits, spawns detached, returns the handle at admission.
    pub fn spawn(
        self: &Arc<Self>,
        prompt: String,
        kwargs: Map<String, Value>,
    ) -> Result<Map<String, Value>, String> {
        require_kwargs(&kwargs)?;
        let requested_name = optional_string(&kwargs, "name")?;
        let requested_model = optional_string(&kwargs, "model")?;
        let thinking = optional_string(&kwargs, "thinking")?;
        let fork = parse_fork(&kwargs)?;
        let isolation = parse_isolation(&kwargs)?;
        if fork == Fork::All && (requested_model.is_some() || thinking.is_some()) {
            return Err(
                "fork=all inherits the parent's model and thinking; drop the override".to_owned(),
            );
        }
        if self.options.depth >= self.options.max_depth {
            return Err(format!(
                "RLM recursion depth limit reached (RLM_DEPTH={}, RLM_MAX_DEPTH={})",
                self.options.depth, self.options.max_depth
            ));
        }
        let model = match &requested_model {
            None => self.options.default_model.clone(),
            Some(selector) => {
                let (provider, id) = selector.split_once('/').ok_or_else(|| {
                    format!("model selector must be provider/model, got {selector}")
                })?;
                resolve_model(provider, id)
                    .ok_or_else(|| format!("no model matches selector {selector}"))?
            }
        };

        let (session_dir, child_id) = self.create_child_dir(&self.options.parent_session_dir)?;
        let session_name =
            requested_name.unwrap_or_else(|| default_session_name(&prompt, &child_id));
        {
            let mut children = self
                .children
                .lock()
                .map_err(|_| "subagent state poisoned")?;
            if children.len() >= self.options.max_children {
                let _ = std::fs::remove_dir_all(&session_dir);
                return Err(format!(
                    "RLM child limit reached ({} children retained); rlm.delete_subagent a finished child first",
                    self.options.max_children
                ));
            }
            if children
                .values()
                .any(|record| record.session_name == session_name)
            {
                let _ = std::fs::remove_dir_all(&session_dir);
                return Err(format!(
                    "Agent session name \"{session_name}\" is already taken at depth {}",
                    self.options.depth.saturating_add(1)
                ));
            }
            let worktree = match isolation {
                Isolation::None => None,
                Isolation::Worktree => Some(crate::worktree::create(
                    &self.options.cwd,
                    &session_dir,
                    &child_id,
                )?),
            };
            let child = (self.options.factory)(ChildBuild {
                model: model.clone(),
                thinking,
                session_dir: &session_dir,
                cwd: worktree.as_ref().map(|tree| tree.path.as_path()),
                link: ParentLink {
                    child_name: session_name.clone(),
                    host: Arc::downgrade(self),
                },
            })?;
            if fork != Fork::None {
                let seed = seed_for_fork(
                    &(self.options.parent_messages)(),
                    fork,
                    child.model().context_window,
                );
                child.seed_messages(seed);
            }
            let session = Arc::new(child);
            children.insert(
                child_id.clone(),
                ChildRecord {
                    session_name: session_name.clone(),
                    session_dir: session_dir.clone(),
                    worktree,
                    status: ChildStatus::Running,
                    activity: ChildActivity::Waiting,
                    tool_use_count: 0,
                    token_count: 0,
                    answer_preview: None,
                    error: None,
                    pending: 0,
                    session: Arc::clone(&session),
                },
            );
            self.watch(child_id.clone(), &session);
            let host = Arc::clone(self);
            let task_child_id = child_id.clone();
            let task_name = session_name.clone();
            let task_prompt = prompt.clone();
            // Invariant: the spawn reply resolves at admission, and blocking here
            // would abort the turn whose cell awaits it.
            tokio::spawn(async move {
                host.run_child(task_child_id, task_name, task_prompt, session)
                    .await;
            });
        }
        self.publish(&child_id);
        let mut reply = Map::new();
        reply.insert("rlm_child_id".to_owned(), Value::String(child_id));
        reply.insert("name".to_owned(), Value::String(session_name));
        reply.insert(
            "session_dir".to_owned(),
            Value::String(session_dir.to_string_lossy().into_owned()),
        );
        reply.insert(
            "model".to_owned(),
            Value::String(format!("{}/{}", model.provider, model.id)),
        );
        Ok(reply)
    }

    async fn run_child(
        self: Arc<Self>,
        child_id: String,
        session_name: String,
        prompt: String,
        session: Arc<AgentSession>,
    ) {
        let content = format!("[task from parent]\n\n{prompt}");
        let outcome = session.prompt(&content);
        let mut error = outcome.err().map(|error| error.to_string());
        if error.is_none() {
            session.wait_idle().await;
            error = session.messages().iter().rev().find_map(|message| {
                if let AgentMessage::Assistant {
                    stop_reason: StopReason::Error,
                    error_message,
                    ..
                } = message
                {
                    Some(
                        error_message
                            .clone()
                            .unwrap_or_else(|| "child run ended with an error".to_owned()),
                    )
                } else {
                    None
                }
            });
        }

        if error.is_none() {
            for message in session.messages() {
                if let AgentMessage::Assistant {
                    stop_reason, usage, ..
                } = &message
                    && !matches!(stop_reason, StopReason::Error | StopReason::Aborted)
                {
                    (self.options.attribute)(usage);
                }
            }
        }

        let status = if error.is_some() {
            ChildStatus::Error
        } else {
            ChildStatus::Completed
        };
        if let Ok(mut children) = self.children.lock()
            && let Some(record) = children.get_mut(&child_id)
        {
            record.status = status;
            record.activity = ChildActivity::Waiting;
            record.error = error.clone();
            record.pending = record.pending.saturating_add(1);
        }
        self.publish(&child_id);
        // Terminal notices reach the parent as user-role host status, never as
        // something that can read as user instructions from the child.
        let notice = match &error {
            Some(error) => format!("[subagent {session_name} ({child_id}) failed]\n{error}"),
            None => {
                let answer = last_assistant_text(&session.messages())
                    .map(|text| preview(&text))
                    .unwrap_or_else(|| "(no final answer text)".to_owned());
                format!(
                    "[subagent {session_name} ({child_id}) completed without replying]\nLast answer: {answer}"
                )
            }
        };
        (self.options.notice)(&notice);
    }

    pub fn list(&self) -> Map<String, Value> {
        let entries: Vec<Value> = self
            .children
            .lock()
            .map(|children| {
                let mut entries: Vec<(String, Value)> = children
                    .iter()
                    .map(|(id, record)| (id.clone(), child_entry(id, record)))
                    .collect();
                entries.sort_by(|left, right| left.0.cmp(&right.0));
                entries.into_iter().map(|(_, entry)| entry).collect()
            })
            .unwrap_or_default();
        let mut reply = Map::new();
        reply.insert("subagents".to_owned(), Value::Array(entries));
        reply
    }

    pub(crate) fn key_of(
        children: &HashMap<String, ChildRecord>,
        target: &str,
    ) -> Result<String, String> {
        children
            .iter()
            .find(|(id, record)| id.as_str() == target || record.session_name == target)
            .map(|(id, _)| id.clone())
            .ok_or_else(|| format!("No RLM child matches \"{target}\""))
    }

    pub fn delete(&self, target: &str) -> Result<Map<String, Value>, String> {
        let mut children = self
            .children
            .lock()
            .map_err(|_| "subagent state poisoned")?;
        let key = Self::key_of(&children, target)?;
        if let Some(record) = children.get(&key)
            && let Some(tree) = &record.worktree
        {
            return Err(format!(
                "child \"{target}\" holds the worktree {}; merge or discard it first",
                tree.path.display()
            ));
        }
        let Some(record) = children.remove(&key) else {
            return Err(format!("No RLM child matches \"{target}\""));
        };
        if record.status == ChildStatus::Running {
            record.session.abort();
        }
        let mut reply = Map::new();
        reply.insert("subagent".to_owned(), child_entry(&key, &record));
        Ok(reply)
    }

    pub fn find_models(query: &str, limit: usize) -> Map<String, Value> {
        let needle = query.to_lowercase();
        let models: Vec<Value> = available_models()
            .into_iter()
            .filter(|model| {
                needle.is_empty()
                    || model.id.to_lowercase().contains(&needle)
                    || model.name.to_lowercase().contains(&needle)
                    || model.provider.to_lowercase().contains(&needle)
            })
            .take(limit)
            .map(|model| {
                json!({
                    "provider": model.provider,
                    "id": model.id,
                    "name": model.name,
                    "selector": format!("{}/{}", model.provider, model.id),
                })
            })
            .collect();
        let mut reply = Map::new();
        reply.insert("models".to_owned(), Value::Array(models));
        reply
    }

    pub fn register(self: &Arc<Self>, registry: &mut crate::kernel::HostRegistry) {
        let host = Arc::clone(self);
        registry.register("agent_message.send", move |payload| {
            let (target, text, followup) = message_params(&payload);
            let host = Arc::clone(&host);
            Box::pin(async move {
                let text = text.ok_or("agent_message.send requires a message")?;
                host.route(PARENT_NAME, &target, &text, followup)
            })
        });
        let host = Arc::clone(self);
        registry.register("agent_message.list_agents", move |_payload| {
            let reply = host.roster();
            Box::pin(async move { Ok(reply) })
        });
        let host = Arc::clone(self);
        registry.register("rlm.wait", move |payload| {
            let timeout = payload
                .get("timeout_ms")
                .and_then(Value::as_u64)
                .unwrap_or(WAIT_MAX_MS);
            let host = Arc::clone(&host);
            Box::pin(async move { Ok(host.wait(timeout).await) })
        });
        let host = Arc::clone(self);
        registry.register("rlm.interrupt", move |payload| {
            let target = payload
                .get("target")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let host = Arc::clone(&host);
            Box::pin(async move {
                let target = target.ok_or("rlm.interrupt requires a target")?;
                host.interrupt(&target)
            })
        });
        let host = Arc::clone(self);
        registry.register("rlm.result", move |payload| {
            let target = payload
                .get("target")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let schema = payload
                .get("schema")
                .cloned()
                .filter(|value| !value.is_null());
            let host = Arc::clone(&host);
            Box::pin(async move {
                let target = target.ok_or("rlm.result requires a target")?;
                host.result(&target, schema.as_ref())
            })
        });
        let host = Arc::clone(self);
        registry.register("rlm.run", move |payload| {
            let prompt = payload
                .get("prompt")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let kwargs = payload
                .get("kwargs")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let host = Arc::clone(&host);
            Box::pin(async move {
                let prompt = prompt.ok_or("rlm.run requires a prompt")?;
                host.spawn(prompt, kwargs)
            })
        });
        let host = Arc::clone(self);
        registry.register("rlm.list_subagents", move |_payload| {
            let reply = host.list();
            Box::pin(async move { Ok(reply) })
        });
        let host = Arc::clone(self);
        registry.register("rlm.delete_subagent", move |payload| {
            let target = payload
                .get("target")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let host = Arc::clone(&host);
            Box::pin(async move {
                let target = target.ok_or("rlm.delete_subagent requires a target")?;
                host.delete(&target)
            })
        });
        for (method, merges) in [
            ("rlm.merge_worktree", true),
            ("rlm.discard_worktree", false),
        ] {
            let host = Arc::clone(self);
            registry.register(method, move |payload| {
                let target = payload
                    .get("target")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let host = Arc::clone(&host);
                Box::pin(async move {
                    let target = target.ok_or_else(|| format!("{method} requires a target"))?;
                    if merges {
                        host.merge_worktree(&target)
                    } else {
                        host.discard_worktree(&target)
                    }
                })
            });
        }
        registry.register("rlm.find_models", move |payload| {
            let query = payload
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let limit = payload
                .get("limit")
                .and_then(Value::as_u64)
                .map(|limit| usize::try_from(limit).unwrap_or(8))
                .unwrap_or(8);
            let reply = Self::find_models(&query, limit);
            Box::pin(async move { Ok(reply) })
        });
        let model = self.options.default_model.clone();
        registry.register("model.info", move |_payload| {
            let reply = json!({
                "provider": model.provider,
                "id": model.id,
                "name": model.name,
                "selector": format!("{}/{}", model.provider, model.id),
                "input": model.input,
            })
            .as_object()
            .cloned()
            .unwrap_or_default();
            Box::pin(async move { Ok(reply) })
        });
    }
}

/// Carried again by every child one level deeper.
#[derive(Clone)]
pub struct RuntimeWiring {
    pub provider: Arc<crate::provider::ProviderStream>,
    pub system_prompt: String,
    pub tool_execution: yi_loop::ExecutionMode,
    pub cwd: PathBuf,
    pub home: PathBuf,
    pub broker: Option<Arc<crate::permission::PermissionBroker>>,
    pub tools: Arc<dyn Fn() -> Vec<Arc<dyn yi_tools::Tool>> + Send + Sync>,
    pub depth: u8,
    pub max_depth: u8,
    pub rlm_dir: PathBuf,
    /// §12 roles resolved to models; `None` keeps the session's own model.
    pub summarizer: Option<Model>,
    /// Naming `models.advisor` in config enables the LLM reviewer (D28).
    pub advisor: Option<Model>,
    /// `plan.staleReminderTurns` config; None keeps the default.
    pub plan_stale_turns: Option<u64>,
    /// Set for a child: its B6 route back into the family that spawned it.
    pub parent_link: Option<ParentLink>,
    /// D13 `bash.autoBackgroundMs`; None keeps every command in the turn.
    pub auto_background: Option<std::time::Duration>,
}

/// Every spawned child wires itself the same way at depth+1; the depth check in
/// [`SubagentHost::spawn`] is what terminates the recursion.
fn wire_schedule(
    session: &AgentSession,
    wiring: &RuntimeWiring,
    registry: &mut crate::kernel::HostRegistry,
) {
    let store = Arc::new(crate::schedule::JobStore::open(
        wiring.rlm_dir.join("scheduled-jobs.json"),
    ));
    let heartbeats = Arc::new(crate::schedule::HeartbeatService {
        store: Arc::clone(&store),
        session_id: wiring
            .rlm_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "session".to_owned()),
        cwd: wiring.cwd.to_string_lossy().into_owned(),
    });
    heartbeats.register(registry);
    let hook = session.heartbeat_hook();
    let busy = session.activity_handle();
    let deliver: Arc<crate::schedule::DeliverFn> = Arc::new(move |job| {
        let activity = crate::schedule::SessionActivity {
            is_streaming: busy(),
            ..Default::default()
        };
        if crate::schedule::should_defer(job, &activity) {
            return crate::schedule::RunOutcome::Skipped;
        }
        let mode = job
            .delivery_mode
            .unwrap_or(crate::schedule::DEFAULT_HEARTBEAT_DELIVERY_MODE);
        hook(
            crate::schedule::heartbeat_message(job, yi_session::now_ms()),
            mode,
        );
        crate::schedule::RunOutcome::Ran
    });
    let scheduler = crate::schedule::Scheduler::start(Arc::clone(&store), deliver, || {
        format!(
            "dsp-{}",
            random_suffix().unwrap_or_else(|_| "00000000".to_owned())
        )
    });
    session.set_schedule(Arc::clone(&store), Arc::clone(&heartbeats), scheduler);
}

fn wire_goal(
    session: &AgentSession,
    registry: &mut crate::kernel::HostRegistry,
    plan_stale_turns: Option<u64>,
) {
    let service = crate::goal::attach_goal(session);
    service.register(registry);
    session.set_goal_service(service);
    let plan = crate::plan::attach_plan(session, plan_stale_turns);
    plan.register(registry);
    session.set_plan_service(plan);
}

fn wire_advisor(session: &AgentSession, wiring: &RuntimeWiring) {
    let hold_sink: Option<crate::advisor::HoldSink> = wiring.broker.as_ref().map(|broker| {
        let broker = Arc::clone(broker);
        Arc::new(move |advice: &yi_types::advisor::Advice| {
            if !broker.can_ask() {
                return false;
            }
            broker.insert_hold(yi_permission::Hold {
                pattern: advice.target.clone().unwrap_or_default(),
                reason: advice.text.clone(),
                source: yi_permission::HoldSource::Advisor,
                expires_at_ms: Some(yi_session::now_ms().saturating_add(3_600_000)),
            });
            true
        }) as crate::advisor::HoldSink
    });
    // V10: ADVISOR.md attention text, project-local, best-effort.
    let attention = std::fs::read_to_string(wiring.cwd.join("ADVISOR.md")).ok();
    let llm = wiring.advisor.clone().map(|model| {
        Arc::new(crate::advisor::review::LlmReviewer::new(
            Arc::clone(&wiring.provider),
            model,
            attention.clone(),
        ))
    });
    let advisor = crate::advisor::attach_advisor(
        session,
        crate::advisor::AdvisorConfig {
            attention,
            reviewer: llm.is_some(),
            ..crate::advisor::AdvisorConfig::default()
        },
        crate::advisor::AdvisorDeps { hold_sink, llm },
    );
    session.set_advisor(advisor);
}

/// A job finishing between turns reports through the R3 follow-up queue, so the
/// model hears about it without a turn being interrupted.
fn wire_job_completions(session: &AgentSession) {
    let follow_up = session.follow_up_hook();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            for report in yi_tools::jobs::registry().take_finished() {
                follow_up(&format!(
                    "<async_result job=\"{}\" exit=\"{}\">{}\n{}</async_result>",
                    report.id,
                    report.exit_code.unwrap_or(-1),
                    report.command,
                    report.output
                ));
            }
        }
    });
}

pub fn attach_runtime(session: &mut AgentSession, wiring: RuntimeWiring) -> Arc<SubagentHost> {
    if session.compactor().is_none() {
        session.enable_compaction_with_summarizer(
            yi_context::Settings::default(),
            wiring.summarizer.clone(),
        );
    }
    crate::checkpoint::wire_turn_checkpoints(session, &wiring.home, &wiring.cwd);
    let mut registry = crate::kernel::HostRegistry::default();
    registry.register_mcp_stubs();
    if let Some(compactor) = session.compactor() {
        // compact.run only schedules and returns — running inline would abort
        // the turn whose cell awaits the reply (design §6).
        registry.register("compact.run", move |payload| {
            let instructions = payload
                .get("instructions")
                .and_then(Value::as_str)
                .map(str::to_owned);
            compactor.schedule_with_instructions(instructions);
            Box::pin(async {
                let mut reply = Map::new();
                reply.insert("scheduled".to_owned(), Value::Bool(true));
                Ok(reply)
            })
        });
    }
    if let Some(status) = session.compact_status_handle() {
        registry.register("compact.status", move |_payload| {
            let status = status();
            Box::pin(async move {
                let mut reply = Map::new();
                reply.insert("tokens".to_owned(), Value::from(status.tokens));
                reply.insert(
                    "context_window".to_owned(),
                    Value::from(status.context_window),
                );
                reply.insert("percent".to_owned(), Value::from(status.percent));
                reply.insert("scheduled".to_owned(), Value::Bool(status.scheduled));
                Ok(reply)
            })
        });
    }
    let factory_wiring = wiring.clone();
    let factory: Arc<ChildFactory> = Arc::new(move |build: ChildBuild<'_>| {
        let mut child = AgentSession::new(
            crate::session::SessionConfig {
                system_prompt: factory_wiring.system_prompt.clone(),
                model: build.model,
                thinking_level: build.thinking,
                tool_execution: factory_wiring.tool_execution,
            },
            Arc::clone(&factory_wiring.provider),
        );
        attach_runtime(
            &mut child,
            RuntimeWiring {
                depth: factory_wiring.depth.saturating_add(1),
                rlm_dir: build.session_dir.to_path_buf(),
                cwd: build
                    .cwd
                    .map_or_else(|| factory_wiring.cwd.clone(), Path::to_path_buf),
                parent_link: Some(build.link),
                ..factory_wiring.clone()
            },
        );
        Ok(child)
    });
    let host = Arc::new(SubagentHost::new(SubagentHostOptions {
        depth: wiring.depth,
        max_depth: wiring.max_depth,
        max_children: DEFAULT_MAX_CHILDREN,
        parent_session_dir: wiring.rlm_dir.clone(),
        default_model: session.model(),
        factory,
        notice: session.notice_hook(),
        events: session.events_sender(),
        parent_messages: session.history_handle(),
        cwd: wiring.cwd.clone(),
        report: {
            let deliver = session.heartbeat_hook();
            Arc::new(move |message| {
                deliver(message, yi_types::schedule::DeliveryMode::Steer);
            })
        },
        attribute: session.attribution_handle(),
    }));
    host.register(&mut registry);
    if let Some(link) = wiring.parent_link.clone() {
        register_child_messaging(link, &host, &mut registry);
    }
    wire_schedule(session, &wiring, &mut registry);
    wire_goal(session, &mut registry, wiring.plan_stale_turns);
    let restore_notice = session.notice_hook();
    let service = Arc::new(crate::kernel::KernelService::new(
        crate::kernel::KernelServiceOptions {
            cwd: wiring.cwd.clone(),
            home: wiring.home.clone(),
            session_dir: Some(wiring.rlm_dir.clone()),
            host: Arc::new(registry),
            on_restore: Some(Arc::new(move |restore| {
                restore_notice(&crate::kernel::restore_notice_text(restore));
            })),
        },
    ));
    {
        let service = Arc::clone(&service);
        let notice = session.notice_hook();
        session.set_on_compacted(Arc::new(move || {
            let service = Arc::clone(&service);
            let notice = Arc::clone(&notice);
            tokio::spawn(async move {
                if let Some(text) = service.sync_after_compaction().await {
                    notice(&text);
                }
            });
        }));
    }
    let mut tools = (wiring.tools)();
    tools.push(crate::kernel::ipython_tool(service));
    wire_advisor(session, &wiring);
    if let (Some(plan), Some(advisor)) = (session.plan_service(), session.advisor()) {
        plan.set_on_change(Arc::new(move |plan| {
            advisor.request_review(Some(crate::plan::summary_line(plan)));
        }));
    }
    let rule_set = crate::rules::discover(&wiring.cwd, &wiring.home);
    if !rule_set.warnings.is_empty() {
        let notice = session.notice_hook();
        for warning in &rule_set.warnings {
            notice(warning);
        }
    }
    if !rule_set.rules.is_empty() {
        let engine = Arc::new(crate::rules::RuleEngine::new(rule_set.rules));
        crate::rules::attach_rules(session, Arc::clone(&engine));
        session.set_rules_engine(engine);
    }
    session.use_tools_with_background(
        tools,
        wiring.cwd.clone(),
        wiring.broker.clone(),
        wiring.auto_background,
    );
    wire_job_completions(session);
    host
}
