//! A host over faux children for the lease tests: what each build was handed, what the parent
//! was told, and a store the lease journal lands in.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub(crate) use crate::faux_model::faux_model;
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_runtime::{
    AgentSession, ProviderStream, SessionConfig, SubagentHost, SubagentHostOptions, Wall,
};
use yi_types::lease::LeaseRecord;
use yi_types::message::{AgentMessage, StopReason};

/// The `details.errorKind` a tool's refusal names, `untagged` when it names none: what the
/// tool-failure census reads.
pub fn refusal_kind(tool: &dyn yi_tools::Tool, args: serde_json::Value) -> String {
    let input = args.as_object().cloned().unwrap_or_default();
    let output = tool.execute(input, &yi_tools::ToolContext::new(std::env::temp_dir()));
    assert!(output.is_error, "{args}");
    output.result.details["errorKind"]
        .as_str()
        .unwrap_or("untagged")
        .to_owned()
}

/// A finished reply that reports it cost `dollars`: what a priced provider's terminal message
/// carries and the faux provider's own replies never do.
pub fn priced_reply(text: &str, dollars: f64) -> AgentMessage {
    let mut message = faux_assistant_message(vec![faux_text(text)], StopReason::Stop);
    if let AgentMessage::Assistant { usage, .. } = &mut message {
        usage.input = 100;
        usage.output = 20;
        usage.total_tokens = 120;
        usage.cost.total = serde_json::Number::from_f64(dollars).unwrap_or_else(|| 0.into());
    }
    message
}

pub fn hung_up(text: &str) -> AgentMessage {
    let mut message = faux_assistant_message(vec![faux_text(text)], StopReason::Error);
    if let AgentMessage::Assistant { error_message, .. } = &mut message {
        *error_message = Some("the provider hung up".to_owned());
    }
    message
}

pub fn child_session(
    build: yi_runtime::ChildBuild<'_>,
    provider: Arc<ProviderStream>,
) -> AgentSession {
    AgentSession::new(
        SessionConfig {
            system_prompt: "child sys".to_owned(),
            model: build.model,
            thinking_level: build.thinking,
            tool_execution: yi_loop::ExecutionMode::Sequential,
        },
        provider,
    )
}

pub struct Built {
    pub wall: Wall,
    pub deadline: Option<std::time::Duration>,
    pub tokens: Option<u64>,
}

pub struct Family {
    pub host: Arc<SubagentHost>,
    pub built: Arc<Mutex<Vec<Built>>>,
    pub notices: Arc<Mutex<Vec<String>>>,
    pub store: yi_session::SharedSession,
    /// Set, the parent has no transcript: every lease journal write is refused.
    pub unplugged: Arc<std::sync::atomic::AtomicBool>,
    pub events: tokio::sync::broadcast::Sender<yi_types::event::AgentEvent>,
    pub scripts: Arc<Mutex<VecDeque<Vec<AgentMessage>>>>,
}

/// How many usage rows the store booked under a cause `wanted` accepts.
pub fn booked(
    store: &yi_session::SharedSession,
    wanted: impl Fn(&str) -> bool,
) -> Result<usize, yi_session::SessionError> {
    let records =
        yi_session::lock_session(store).find_records(&yi_session::RecordQuery::default())?;
    let usage = records.into_iter().filter(|record| matches!(record, yi_types::record::LaneRecord::Usage { cause, .. } if wanted(cause)));
    Ok(usage.count())
}

pub fn memory_store(id: &str) -> yi_session::SharedSession {
    Arc::new(Mutex::new(yi_session::SessionStore::in_memory(
        yi_session::SessionMetadata {
            id: id.to_owned(),
            created_at: 0,
            parent_session_id: None,
            name: None,
        },
    )))
}

pub async fn wait_until(within_ms: u64, mut ready: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(within_ms);
    while std::time::Instant::now() <= deadline {
        if ready() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    ready()
}

pub async fn wait_for_path(path: &Path, message: &str) -> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..200 {
        if path.exists() {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    Err(message.into())
}

/// `hold` makes every child run that shell command first, so its turn stays open; each
/// reply bills 120 tokens and costs a quarter dollar.
pub fn family(
    dir: PathBuf,
    cwd: PathBuf,
    store: yi_session::SharedSession,
    hold: Option<&'static str>,
) -> Family {
    family_with_slots(dir, cwd, store, hold, 2, 0, 1)
}

pub fn family_with_slots(
    dir: PathBuf,
    cwd: PathBuf,
    store: yi_session::SharedSession,
    hold: Option<&'static str>,
    lane_slots: u8,
    depth: u8,
    max_depth: u8,
) -> Family {
    family_build(dir, cwd, store, hold, lane_slots, depth, max_depth)
}

fn family_build(
    dir: PathBuf,
    cwd: PathBuf,
    store: yi_session::SharedSession,
    hold: Option<&'static str>,
    lane_slots: u8,
    depth: u8,
    max_depth: u8,
) -> Family {
    let built: Arc<Mutex<Vec<Built>>> = Arc::default();
    let scripts: Arc<Mutex<VecDeque<Vec<AgentMessage>>>> = Arc::default();
    let queued = Arc::clone(&scripts);
    let notices: Arc<Mutex<Vec<String>>> = Arc::default();
    let (seen, told, journal) = (Arc::clone(&built), Arc::clone(&notices), store.clone());
    let unplugged = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let plug = Arc::clone(&unplugged);
    let (events, _keep) = tokio::sync::broadcast::channel(256);
    let host = Arc::new(SubagentHost::new(SubagentHostOptions {
        provider: Arc::new(ProviderStream::new(None)),
        depth,
        max_depth,
        max_children: 8,
        parent_session_dir: dir.join("children"),
        plans_dir: cwd.join(".yi/plans"),
        cwd,
        home: dir.join("home"),
        lane_slots,
        defaults: Arc::new(|| (faux_model(128_000), yi_types::model::Effort::Medium)),
        factory: Arc::new(move |build: yi_runtime::ChildBuild<'_>| {
            if let Ok(mut seen) = seen.lock() {
                seen.push(Built {
                    wall: build.wall.clone(),
                    deadline: build.deadline,
                    tokens: build.tokens,
                });
            }
            let provider = Arc::new(ProviderStream::new(None));
            let scripted = queued.lock().ok().and_then(|mut queue| queue.pop_front());
            let tools = hold.is_some() || scripted.is_some();
            let script = match scripted {
                Some(script) => script,
                None => {
                    let mut script = Vec::new();
                    if let Some(command) = hold {
                        let mut args = serde_json::Map::new();
                        args.insert("command".to_owned(), command.into());
                        let call = faux_tool_call("call-1", "bash", args);
                        script.push(faux_assistant_message(vec![call], StopReason::ToolUse));
                    }
                    script.push(priced_reply("ok", 0.25));
                    script
                }
            };
            provider.queue_faux(script);
            let wall = build.wall.clone();
            let cwd = build.cwd.map_or_else(std::env::temp_dir, PathBuf::from);
            let mut child = child_session(build, provider);
            if tools {
                child.set_wall(wall);
                child.use_tools(yi_tools::builtin_tools(), cwd, None);
            }
            Ok(child)
        }),
        notice: Arc::new(move |text: &str, _| {
            if let Ok(mut told) = told.lock() {
                told.push(text.to_owned());
            }
        }),
        events: events.clone(),
        parent_messages: Arc::new(Vec::new),
        report: Arc::new(|_message, _| {}),
        attribute: Arc::new(|_usage| {}),
        store: Arc::new(move || {
            (!plug.load(std::sync::atomic::Ordering::SeqCst)).then(|| journal.clone())
        }),
        family_live: yi_runtime::fetch::KernelServiceMap::new(),
    }));
    Family {
        scripts,
        host,
        built,
        notices,
        store,
        unplugged,
        events,
    }
}

impl Family {
    pub fn queue(&self, script: Vec<AgentMessage>) -> Result<(), String> {
        self.scripts
            .lock()
            .map(|mut queue| queue.push_back(script))
            .map_err(|_| "script queue poisoned".to_owned())
    }

    pub fn journal(&self) -> Vec<LeaseRecord> {
        let query = yi_session::EntryQuery {
            custom_type: Some("lease".to_owned()),
            order: yi_session::EntryOrder::OldestFirst,
            ..yi_session::EntryQuery::default()
        };
        let entries = yi_session::lock_session(&self.store).find_entries(&query);
        entries
            .unwrap_or_default()
            .into_iter()
            .filter_map(|entry| match entry {
                yi_types::entry::Entry::Custom { data, .. } => serde_json::from_value(data?).ok(),
                _ => None,
            })
            .collect()
    }

    pub fn state_of(&self, name: &str) -> Option<String> {
        let status = self.host.status();
        let members = status["members"].as_array()?;
        let member = members.iter().find(|member| member["name"] == name)?;
        member["state"].as_str().map(str::to_owned)
    }

    /// Polls, never sleeps blind: returns as soon as the child reads `state`.
    pub async fn reaches(&self, name: &str, state: &str) -> bool {
        for _ in 0..400 {
            if self.state_of(name).as_deref() == Some(state) {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        }
        false
    }
}

/// A builtin tool behind the runtime's URL routing, run once: its text and whether it refused.
pub fn routed_tool(
    resolver: &Arc<yi_runtime::fetch::Resolver>,
    name: &str,
    input: serde_json::Value,
) -> Result<(String, bool), Box<dyn std::error::Error>> {
    let mut tools: Vec<Arc<dyn yi_tools::Tool>> = yi_tools::builtin_tools()
        .into_iter()
        .filter(|tool| tool.name() == name)
        .collect();
    yi_runtime::fetch::route_urls(&mut tools, resolver);
    let input = input.as_object().cloned().ok_or("input is not an object")?;
    let output = tools
        .first()
        .ok_or("no such tool")?
        .execute(input, &yi_tools::ToolContext::new(std::env::temp_dir()));
    let text = yi_types::message::join_text(&output.result.content, "\n");
    Ok((text, output.is_error))
}

/// The #1184 gate's report.py output: 301 lines, the fact on line 219.
pub fn report_output() -> String {
    let mut lines: Vec<String> = (0..300)
        .map(|i| format!("setting_{i:03} = fb3f5c"))
        .collect();
    lines.insert(218, "release_id = rel-09d88875".to_owned());
    lines.join("\n")
}
