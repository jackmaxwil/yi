//! A host over faux children for the lease tests: what each build was handed, what the parent
//! was told, and a store the lease journal lands in.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_runtime::{
    AgentSession, ProviderStream, SessionConfig, SubagentHost, SubagentHostOptions, Wall,
};
use yi_types::lease::LeaseRecord;
use yi_types::message::{AgentMessage, StopReason};
use yi_types::model::{Model, ModelCost};

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

pub fn faux_model() -> Model {
    let zero = || serde_json::Number::from(0u64);
    Model {
        id: "faux-1".to_owned(),
        name: "Faux".to_owned(),
        api: "faux".to_owned(),
        provider: "faux".to_owned(),
        base_url: "http://localhost:0".to_owned(),
        reasoning: false,
        input: vec!["text".to_owned()],
        cost: ModelCost {
            input: zero(),
            output: zero(),
            cache_read: zero(),
            cache_write: zero(),
            tiers: None,
        },
        context_window: 128_000,
        max_tokens: 16_384,
        compat: None,
        thinking_level_map: None,
        headers: None,
    }
}

/// `hold` makes every child run that shell command first, so its turn stays open; each
/// reply bills 120 tokens.
pub fn family(
    dir: PathBuf,
    cwd: PathBuf,
    store: yi_session::SharedSession,
    hold: Option<&'static str>,
) -> Family {
    let built: Arc<Mutex<Vec<Built>>> = Arc::default();
    let notices: Arc<Mutex<Vec<String>>> = Arc::default();
    let (seen, told, journal) = (Arc::clone(&built), Arc::clone(&notices), store.clone());
    let unplugged = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let plug = Arc::clone(&unplugged);
    let (events, _keep) = tokio::sync::broadcast::channel(256);
    let host = Arc::new(SubagentHost::new(SubagentHostOptions {
        depth: 0,
        max_depth: 1,
        max_children: 8,
        parent_session_dir: dir.join("children"),
        plans_dir: cwd.join(".yi/plans"),
        cwd,
        home: dir.join("home"),
        lane_slots: 2,
        defaults: Arc::new(|| (faux_model(), yi_types::model::Effort::Medium)),
        factory: Arc::new(move |build: yi_runtime::ChildBuild<'_>| {
            if let Ok(mut seen) = seen.lock() {
                seen.push(Built {
                    wall: build.wall.clone(),
                    deadline: build.deadline,
                    tokens: build.tokens,
                });
            }
            let provider = Arc::new(ProviderStream::new(None, None));
            let mut script = Vec::new();
            if let Some(command) = hold {
                let mut args = serde_json::Map::new();
                args.insert("command".to_owned(), command.into());
                let call = faux_tool_call("call-1", "bash", args);
                script.push(faux_assistant_message(vec![call], StopReason::ToolUse));
            }
            let mut reply = faux_assistant_message(vec![faux_text("ok")], StopReason::Stop);
            if let AgentMessage::Assistant { usage, .. } = &mut reply {
                usage.total_tokens = 120;
            }
            script.push(reply);
            provider.queue_faux(script);
            let config = SessionConfig {
                system_prompt: "child sys".to_owned(),
                model: build.model,
                thinking_level: build.thinking,
                tool_execution: yi_loop::ExecutionMode::Sequential,
            };
            let mut child = AgentSession::new(config, provider);
            if hold.is_some() {
                let cwd = build.cwd.map_or_else(std::env::temp_dir, PathBuf::from);
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
        family_live: Arc::new(|| 0),
    }));
    Family {
        host,
        built,
        notices,
        store,
        unplugged,
        events,
    }
}

impl Family {
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
