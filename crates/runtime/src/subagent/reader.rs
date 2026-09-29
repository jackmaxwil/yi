use std::path::Path;
use std::sync::Arc;

use serde_json::{Map, Value};
use yi_types::url::{Scheme, Url};

use crate::session::{AgentSession, SessionConfig};

pub const READER_PROMPT: &str = include_str!("../prompts/reader.md");
const READER_TOOLS: [&str; 2] = ["read", "grep"];
const DEFAULT_TURNS: u32 = 3;
const MAX_TURNS: u32 = 10;
pub(crate) const PARTITION_CAP: usize = 65_536;
pub const HELD_CAP: usize = 64;

pub(crate) fn full_child(refusal: &str) -> String {
    format!("{refusal}; role=\"root\" spawns a full child")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reader {
    pub tools: Vec<String>,
    pub turns: u32,
    pub schema: Option<Value>,
    pub shared_through: Option<usize>,
}

pub(crate) fn parse(kwargs: &Map<String, Value>) -> Result<Option<Reader>, String> {
    let role = super::optional_string(kwargs, "role")?;
    match role.as_deref() {
        None | Some("root") => {
            if let Some(key) = ["tools", "turns", "schema"]
                .iter()
                .find(|key| kwargs.contains_key(**key))
            {
                return Err(format!(
                    "rlm.run {key} shapes a reader; add role=\"reader\""
                ));
            }
            Ok(None)
        }
        Some("reader") if kwargs.get("check").is_some_and(|check| !check.is_null()) => {
            Err(full_child("a reader answers once and runs no check"))
        }
        Some("reader") => Ok(Some(Reader {
            tools: tools_of(kwargs)?,
            turns: turns_of(kwargs)?,
            schema: schema_of(kwargs)?,
            shared_through: None,
        })),
        Some(other) => Err(format!(
            "rlm.run role must be \"reader\" or \"root\", got {other}"
        )),
    }
}

fn tools_of(kwargs: &Map<String, Value>) -> Result<Vec<String>, String> {
    let Some(value) = kwargs.get("tools").filter(|value| !value.is_null()) else {
        return Ok(READER_TOOLS.map(str::to_owned).to_vec());
    };
    let names = value
        .as_array()
        .ok_or("rlm.run tools must be a list of tool names")?
        .iter()
        .map(|name| name.as_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()
        .ok_or("rlm.run tools must be a list of tool names")?;
    match names
        .iter()
        .find(|name| !READER_TOOLS.contains(&name.as_str()))
    {
        Some(name) => Err(full_child(&format!(
            "a reader may call {}, not {name}",
            READER_TOOLS.join(" and ")
        ))),
        None => Ok(names),
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

fn turns_of(kwargs: &Map<String, Value>) -> Result<u32, String> {
    match kwargs.get("turns") {
        None | Some(Value::Null) => Ok(DEFAULT_TURNS),
        Some(value) => value
            .as_u64()
            .and_then(|turns| u32::try_from(turns).ok())
            .filter(|turns| (1..=MAX_TURNS).contains(turns))
            .ok_or_else(|| format!("rlm.run turns must be 1 to {MAX_TURNS}, got {value}")),
    }
}

pub fn session(
    provider: Arc<crate::provider::ProviderStream>,
    build: crate::subagent::ChildBuild<'_>,
    reader: &Reader,
    tools: Vec<Arc<dyn yi_tools::Tool>>,
    cwd: std::path::PathBuf,
    broker: Option<Arc<crate::permission::PermissionBroker>>,
    rules: Option<Arc<crate::rules::RuleEngine>>,
) -> AgentSession {
    let mut child = AgentSession::new(
        SessionConfig {
            system_prompt: READER_PROMPT.to_owned(),
            model: build.model,
            thinking_level: build.thinking,
            tool_execution: yi_loop::ExecutionMode::default(),
        },
        provider,
    );
    child.set_turn_cap(reader.turns);
    child.set_request_shape(crate::session::RequestShape {
        schema: reader.schema.clone(),
        shared_through: reader.shared_through,
    });
    child.set_wall(build.wall);
    if let Some(rules) = rules {
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

/// A reader marks its partition when a sibling sent it lately or a fan-out shares it (`readers`,
/// counted by `rlm.run`), so a fan-out's first reader writes it; a lone one pays nothing (D314).
pub(crate) fn share(
    host: &super::SubagentHost,
    kwargs: &Map<String, Value>,
    seed: Option<&String>,
    cast: &mut super::build::Cast,
) {
    if let (Some(reader), Some(text)) = (cast.3.as_mut(), seed) {
        let seen = seen_recently(host, text);
        let gathered = kwargs.get("readers").and_then(Value::as_u64).unwrap_or(1) > 1;
        reader.shared_through = (seen || gathered).then_some(0);
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
