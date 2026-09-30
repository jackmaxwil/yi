use std::path::Path;
use std::sync::Arc;

use serde_json::{Map, Value};
use yi_types::url::{Scheme, Url};

use crate::session::{AgentSession, SessionConfig};

pub const READER_PROMPT: &str = include_str!("../prompts/reader.md");
pub const WORKER_PROMPT: &str = include_str!("../prompts/worker.md");
pub(crate) const PARTITION_CAP: usize = 65_536;
pub const HELD_CAP: usize = 64;

pub(crate) fn full_child(refusal: &str) -> String {
    format!("{refusal}; role=\"root\" spawns a full child")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Reader,
    Worker,
}

impl Role {
    const fn name(self) -> &'static str {
        match self {
            Self::Reader => "reader",
            Self::Worker => "worker",
        }
    }

    const fn allowed_tools(self) -> &'static [&'static str] {
        match self {
            Self::Reader => &["read", "grep"],
            Self::Worker => &["read", "grep", "edit", "write", "bash", "get_context"],
        }
    }

    const fn default_tools(self) -> &'static [&'static str] {
        match self {
            Self::Reader => self.allowed_tools(),
            Self::Worker => &["read", "grep", "edit", "write"],
        }
    }

    const fn default_turns(self) -> u32 {
        match self {
            Self::Reader => 3,
            Self::Worker => 12,
        }
    }

    const fn max_turns(self) -> u32 {
        match self {
            Self::Reader => 10,
            Self::Worker => 40,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reader {
    pub role: Role,
    pub tools: Vec<String>,
    pub turns: u32,
    pub schema: Option<Value>,
    pub shared_through: Option<usize>,
}

pub(crate) fn parse(kwargs: &Map<String, Value>) -> Result<Option<Reader>, String> {
    let role = match super::optional_string(kwargs, "role")?.as_deref() {
        None | Some("root") => {
            if let Some(key) = ["tools", "turns", "schema"]
                .iter()
                .find(|key| kwargs.contains_key(**key))
            {
                return Err(format!(
                    "rlm.run {key} shapes a reader or a worker; add role=\"reader\" or role=\"worker\""
                ));
            }
            return Ok(None);
        }
        Some("reader") => Role::Reader,
        Some("worker") => Role::Worker,
        Some(other) => {
            return Err(format!(
                "rlm.run role must be \"reader\", \"worker\" or \"root\", got {other}"
            ));
        }
    };
    if kwargs.get("check").is_some_and(|check| !check.is_null()) {
        return Err(full_child(&format!(
            "a {} answers once and runs no check",
            role.name()
        )));
    }
    Ok(Some(Reader {
        role,
        tools: tools_of(kwargs, role)?,
        turns: turns_of(kwargs, role)?,
        schema: schema_of(kwargs)?,
        shared_through: None,
    }))
}

fn tools_of(kwargs: &Map<String, Value>, role: Role) -> Result<Vec<String>, String> {
    let allowed = role.allowed_tools();
    let Some(value) = kwargs.get("tools").filter(|value| !value.is_null()) else {
        return Ok(role
            .default_tools()
            .iter()
            .map(|name| (*name).to_owned())
            .collect());
    };
    let names = value
        .as_array()
        .ok_or("rlm.run tools must be a list of tool names")?
        .iter()
        .map(|name| name.as_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()
        .ok_or("rlm.run tools must be a list of tool names")?;
    match names.iter().find(|name| !allowed.contains(&name.as_str())) {
        Some(name) => Err(full_child(&format!(
            "a {} may call {}, not {name}",
            role.name(),
            spoken(allowed)
        ))),
        None => Ok(names),
    }
}

fn spoken(names: &[&str]) -> String {
    match names.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => names.join(", "),
    }
}

fn schema_of(kwargs: &Map<String, Value>) -> Result<Option<Value>, String> {
    match kwargs.get("schema") {
        None | Some(Value::Null) => Ok(None),
        Some(schema @ Value::Object(_)) => Ok(Some(schema.clone())),
        Some(other) => Err(format!(
            "rlm.run schema must be a JSON schema object, got {other}"
        )),
    }
}

fn turns_of(kwargs: &Map<String, Value>, role: Role) -> Result<u32, String> {
    let most = role.max_turns();
    match kwargs.get("turns") {
        None | Some(Value::Null) => Ok(role.default_turns()),
        Some(value) => value
            .as_u64()
            .and_then(|turns| u32::try_from(turns).ok())
            .filter(|turns| (1..=most).contains(turns))
            .ok_or_else(|| format!("rlm.run turns must be 1 to {most}, got {value}")),
    }
}

pub fn session(
    provider: Arc<crate::provider::ProviderStream>,
    build: crate::subagent::ChildBuild<'_>,
    reader: &Reader,
    tools: Vec<Arc<dyn yi_tools::Tool>>,
    (cwd, home): (std::path::PathBuf, &Path),
    broker: Option<Arc<crate::permission::PermissionBroker>>,
    rules: Option<Arc<crate::rules::RuleEngine>>,
) -> AgentSession {
    let mut child = AgentSession::new(
        SessionConfig {
            system_prompt: match reader.role {
                Role::Reader => READER_PROMPT.to_owned(),
                Role::Worker => String::new(),
            },
            model: build.model,
            thinking_level: build.thinking,
            tool_execution: yi_loop::ExecutionMode::default(),
        },
        provider,
    );
    if reader.role == Role::Worker {
        let mode = broker
            .as_ref()
            .map_or(yi_permission::PermissionMode::Auto, |broker| broker.mode());
        child.install_extensions(crate::ext::narrow(&cwd, home, WORKER_PROMPT, mode));
        child.enable_compaction();
    }
    child.set_turn_cap(reader.turns);
    child.set_request_shape(crate::session::RequestShape {
        schema: reader.schema.clone(),
        shared_through: reader.shared_through,
    });
    child.set_wall(build.wall);
    if let Some(rules) = rules {
        if reader.role == Role::Worker {
            crate::rules::attach_rules(&child, Arc::clone(&rules));
            let rearm = Arc::clone(&rules);
            child.set_on_compacted(Arc::new(move || rearm.rearm()));
        }
        child.set_rules_engine(rules);
    }
    let named: Vec<_> = tools
        .into_iter()
        .filter(|tool| reader.turns > 1 && reader.tools.iter().any(|name| name == tool.name()))
        .collect();
    if named.is_empty() {
        child.set_reuse(yi_types::model::Reuse::OneShot);
    }
    child.use_tools(named, cwd, broker);
    child
}

pub(crate) fn partition(
    resolver: &crate::fetch::Resolver,
    entries: &[String],
    wall: &crate::wall::Wall,
    cwd: &Path,
) -> Result<String, String> {
    let mut out = String::new();
    for (index, raw) in entries.iter().enumerate() {
        let url: Url = raw
            .parse()
            .map_err(|error| format!("partition {raw}: {error}"))?;
        if matches!(url.scheme(), Scheme::Kernel) {
            return Err(format!(
                "partition {raw}: a kernel value rides context_keys, not the partition"
            ));
        }
        if let Some(denied) = wall.check_url(&url, cwd) {
            return Err(format!(
                "the partition names what its wall denies. {denied}"
            ));
        }
        let mut room = PARTITION_CAP.saturating_sub(out.len());
        if room == 0 {
            out.push_str(&format!(
                "[… kept {index} of {} partition entries: partition cap {PARTITION_CAP} bytes; \
                 entries {} to {} (from {raw}) go in another reader's partition]\n",
                entries.len(),
                index.saturating_add(1),
                entries.len()
            ));
            break;
        }
        let fetched = resolver
            .fetch(&url)
            .map_err(|error| format!("partition {raw}: {error}"))?;
        let first = url
            .fragment()
            .map_or(1, |range| usize::try_from(range.start().get()).unwrap_or(1));
        let mut kept = Vec::new();
        let lines: Vec<&str> = fetched.text.lines().collect();
        for (line, number) in lines.iter().zip(first..) {
            let numbered = format!("{number}:{line}");
            let Some(left) = room.checked_sub(numbered.len().saturating_add(1)) else {
                room = 0;
                break;
            };
            room = left;
            kept.push(numbered);
        }
        let text = crate::ext::sanitize(&kept.join("\n")).into_owned();
        out.push_str(&crate::ext::fence(raw, "untrusted", &text));
        out.push_str("\n\n");
        if kept.len() < lines.len() {
            let (next, last) = (
                first.saturating_add(kept.len()),
                first.saturating_add(lines.len()).saturating_sub(1),
            );
            let rest = match url.scheme() {
                Scheme::Local => format!("read path={} offset={next}", url.path()),
                _ => "not inlined".to_owned(),
            };
            out.push_str(&format!(
                "[… kept {} of {} lines of {raw}: partition cap {PARTITION_CAP} bytes; \
                 lines {next}-{last}: {rest}]\n\n",
                kept.len(),
                lines.len()
            ));
        }
    }
    Ok(out)
}

fn entries(kwargs: &Map<String, Value>) -> Result<Vec<String>, String> {
    match kwargs.get("partition") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| "rlm.run partition must be a list of URLs".to_owned()),
        Some(_) => Err("rlm.run partition must be a list of URLs".to_owned()),
    }
}

pub(crate) fn walls_writes(kwargs: &mut Map<String, Value>) {
    let denied = kwargs
        .entry("deny_write")
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Value::Array(paths) = denied
        && !paths.iter().any(|path| path == ".")
    {
        paths.push(Value::from("."));
    }
}

pub(crate) fn brief(
    host: &super::SubagentHost,
    kwargs: &Map<String, Value>,
    prompt: String,
    cast: &mut super::build::Cast,
) -> Result<(Option<String>, String), String> {
    let named = entries(kwargs)?;
    let fenced = match (named.is_empty(), host.resolver.get()) {
        (true, _) => None,
        (false, Some(resolver)) => Some(partition(resolver, &named, &cast.2, &host.options.cwd)?),
        (false, None) => return Err("this session resolves no partition".to_owned()),
    };
    let Some(reader) = cast.3.as_mut() else {
        return Ok((None, format!("{}{prompt}", fenced.unwrap_or_default())));
    };
    let question = match &reader.schema {
        Some(schema) => {
            format!("{prompt}\n\nReply with one JSON object matching this schema:\n{schema}")
        }
        None => prompt,
    };
    Ok((fenced, question))
}

/// A reader marks its partition when a sibling sent it lately or a fan-out shares it (D314).
pub(crate) fn share(
    host: &super::SubagentHost,
    kwargs: &Map<String, Value>,
    seed: Option<&String>,
    cast: &mut super::build::Cast,
) -> Option<Stagger> {
    let (Some(reader), Some(text)) = (cast.3.as_mut(), seed) else {
        return None;
    };
    let seen = seen_recently(host, text);
    let gathered = kwargs.get("readers").and_then(Value::as_u64).unwrap_or(1) > 1;
    reader.shared_through = (seen || gathered).then_some(0);
    reader.shared_through?;
    let shape = format!(
        "{}{:?}{:?}{:?}{text}",
        cast.0.id, reader.role, reader.tools, reader.schema
    );
    Some(stagger(host, crate::fetch::content_hash(&shape)))
}

const STAGGER_BOUND: std::time::Duration = std::time::Duration::from_secs(20);

/// An entry is read once the response writing it begins: a follower waits for its lead (§9.4).
pub(crate) enum Stagger {
    Lead(tokio::sync::watch::Sender<bool>),
    Follow(tokio::sync::watch::Receiver<bool>),
}

fn stagger(host: &super::SubagentHost, key: String) -> Stagger {
    let Ok(mut leads) = host.leads.lock() else {
        return Stagger::Lead(tokio::sync::watch::channel(false).0);
    };
    leads.retain(|_, lead| !*lead.borrow() && lead.has_changed().is_ok());
    if let Some(lead) = leads.get(&key) {
        return Stagger::Follow(lead.clone());
    }
    let (sender, lead) = tokio::sync::watch::channel(false);
    leads.insert(key, lead);
    Stagger::Lead(sender)
}

impl Stagger {
    pub(crate) fn arm(self, child: &AgentSession) -> Option<tokio::sync::watch::Receiver<bool>> {
        let sender = match self {
            Self::Follow(lead) => return Some(lead),
            Self::Lead(sender) => sender,
        };
        let mut events = child.subscribe();
        drop(tokio::spawn(async move {
            let began = async {
                while let Ok(event) = events.recv().await {
                    if response_began(&event) {
                        break;
                    }
                }
            };
            let _bounded = tokio::time::timeout(STAGGER_BOUND, began).await;
            let _followers_may_be_gone = sender.send(true);
        }));
        None
    }
}

pub(crate) async fn follow(lead: Option<tokio::sync::watch::Receiver<bool>>) {
    if let Some(mut lead) = lead {
        let _begun_or_bounded =
            tokio::time::timeout(STAGGER_BOUND, lead.wait_for(|begun| *begun)).await;
    }
}

fn response_began(event: &yi_types::event::AgentEvent) -> bool {
    use yi_types::event::AgentEvent;
    match event {
        // The loop sends a request's start and waits as other events: an update is upstream bytes.
        AgentEvent::MessageUpdate { .. } | AgentEvent::AgentEnd { .. } => true,
        AgentEvent::MessageEnd { message } => {
            matches!(message, yi_types::message::AgentMessage::Assistant { .. })
        }
        _ => false,
    }
}

const SHARED_WINDOW: std::time::Duration = std::time::Duration::from_secs(300);

fn seen_recently(host: &super::SubagentHost, text: &str) -> bool {
    let Ok(mut seen) = host.partitions.lock() else {
        return false;
    };
    let now = std::time::Instant::now();
    seen.retain(|_, at| now.duration_since(*at) < SHARED_WINDOW);
    seen.insert(crate::fetch::content_hash(text), now).is_some()
}

pub(crate) fn seed(child: &AgentSession, partition: Option<String>) {
    let Some(text) = partition else {
        return;
    };
    let message = crate::session::user_message(&text);
    if let Some(store) = child.store() {
        let _kept_in_memory_either_way =
            yi_session::lock_session(&store).append_message("main", message.clone());
    }
    child.seed_messages(vec![message]);
}
