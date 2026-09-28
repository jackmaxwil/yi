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
        Some(name) => Err(format!(
            "a reader may call {}, not {name}",
            READER_TOOLS.join(" and ")
        )),
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
    let named = tools
        .into_iter()
        .filter(|tool| reader.tools.iter().any(|name| name == tool.name()))
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
    for raw in entries {
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
        let fetched = resolver
            .fetch(&url)
            .map_err(|error| format!("partition {raw}: {error}"))?;
        let first = url.fragment().map_or(1, |range| range.start().get());
        let numbered: Vec<String> = fetched
            .text
            .lines()
            .zip(first..)
            .map(|(line, number)| format!("{number}:{line}"))
            .collect();
        let text = crate::ext::sanitize(&numbered.join("\n")).into_owned();
        out.push_str(&crate::ext::fence(raw, "untrusted", &text));
        out.push_str("\n\n");
    }
    Ok(yi_context::fit(&out, yi_context::Bytes(PARTITION_CAP)).text)
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
