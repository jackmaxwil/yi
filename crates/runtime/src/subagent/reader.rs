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
}

pub(crate) fn parse(kwargs: &Map<String, Value>) -> Result<Option<Reader>, String> {
    let role = super::optional_string(kwargs, "role")?;
    match role.as_deref() {
        None | Some("root") => {
            if let Some(key) = ["tools", "turns"]
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
    child.set_wall(build.wall);
    if let Some(rules) = rules {
        child.set_rules_engine(rules);
    }
    let named = tools
        .into_iter()
        .filter(|tool| reader.turns > 1 && reader.tools.iter().any(|name| name == tool.name()))
        .collect();
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
    resolver: Option<&Arc<crate::fetch::Resolver>>,
    kwargs: &Map<String, Value>,
    prompt: String,
    wall: &crate::wall::Wall,
    cwd: &Path,
) -> Result<String, String> {
    let named = entries(kwargs)?;
    match (named.is_empty(), resolver) {
        (true, _) => Ok(prompt),
        (false, Some(resolver)) => Ok(format!(
            "{}{prompt}",
            partition(resolver, &named, wall, cwd)?
        )),
        (false, None) => Err("this session resolves no partition".to_owned()),
    }
}
