//! The plan test rig: a host over scripted faux children and an engine over a scratch store.
use std::sync::Arc;

use yi_types::message::AgentMessage;
use yi_types::plan::doc::{Delegation, PlanId, TodoLabel};

use super::dispatch::SessionDelegate;
use crate::fetch::FetchLog;
use crate::subagent::SubagentHost;
use serde_json::{Map, Value};
use std::num::NonZeroUsize;
use std::sync::Mutex;
use yi_loop::ExecutionMode;
use yi_types::backoff::backoff;
use yi_types::message::StopReason;
use yi_types::message::UserContent;
use yi_types::plan::doc::{Check, SpawnSpec, TodoState};

use crate::plan::ops::{Actor, Op, OpRequest, PlanEngine, TodoSpec};
use crate::plan::store::PlanStore;
use crate::scratch::Scratch;
use crate::session::{AgentSession, SessionConfig};
use crate::subagent::{ChildBuild, ChildStatus, SubagentHostOptions};

pub(in crate::plan) type TestResult = Result<(), Box<dyn std::error::Error>>;

use yi_ai::faux::faux_model;

pub(crate) fn reply(text: &str) -> AgentMessage {
    yi_ai::faux::faux_assistant_message(vec![yi_ai::faux::faux_text(text)], StopReason::Stop)
}

pub(in crate::plan) fn scripted(script: Vec<AgentMessage>) -> AgentSession {
    let provider = Arc::new(crate::provider::ProviderStream::new(None));
    provider.queue_faux(script);
    AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    )
}

pub(in crate::plan) fn faux_session(replies: &[&str]) -> AgentSession {
    scripted(replies.iter().map(|text| reply(text)).collect())
}

pub(in crate::plan) type Sink<T> = Arc<Mutex<Vec<T>>>;

pub(in crate::plan) fn sink<T: Send + 'static>() -> (Sink<T>, Arc<dyn Fn(T) + Send + Sync>) {
    let sink: Sink<T> = Arc::new(Mutex::new(Vec::new()));
    let writer = Arc::clone(&sink);
    let push = Arc::new(move |item: T| {
        if let Ok(mut items) = writer.lock() {
            items.push(item);
        }
    });
    (sink, push)
}

pub(crate) struct Rig {
    pub(in crate::plan) engine: Arc<PlanEngine>,
    pub(crate) host: Arc<SubagentHost>,
    pub(in crate::plan) pins: Arc<FetchLog>,
    pub(in crate::plan) reports: Sink<AgentMessage>,
    /// The host's own lifecycle notices, which a child the engine took never sends.
    pub(in crate::plan) notices: Sink<String>,
    /// The engine's lines to the owner.
    pub(in crate::plan) said: Sink<AgentMessage>,
    pub(in crate::plan) cwd: Scratch,
}

pub(in crate::plan) fn rig(child_answer: &'static str) -> Result<Rig, Box<dyn std::error::Error>> {
    rig_of(vec![reply(child_answer)], false)
}

/// The root session's rig: the engine takes each finished plan child (D225).
pub(in crate::plan) fn hooked(
    script: Vec<AgentMessage>,
) -> Result<Rig, Box<dyn std::error::Error>> {
    rig_of(script, true)
}

pub(crate) fn rig_of(
    script: Vec<AgentMessage>,
    finish: bool,
) -> Result<Rig, Box<dyn std::error::Error>> {
    let root = Scratch::new("yi-dispatch-rig")?;
    let (events, _keep) = tokio::sync::broadcast::channel(64);
    let (reports, report) = sink();
    let (notices, notice) = sink::<String>();
    let (said, say) = sink();
    let asks = serde_json::to_string(&script).is_ok_and(|text| text.contains("ask_user"));
    let cwd = root.to_path_buf();
    let host = Arc::new(SubagentHost::new(SubagentHostOptions {
        provider: Arc::new(crate::provider::ProviderStream::new(None)),
        depth: 0,
        max_depth: 1,
        max_children: 8,
        parent_session_dir: root.join("children"),
        cwd: root.to_path_buf(),
        home: root.join("home"),
        lane_slots: 1,
        defaults: Arc::new(|| (faux_model(), yi_types::model::Effort::Medium)),
        factory: Arc::new(move |build: ChildBuild<'_>| {
            let mut child = scripted(script.clone());
            if asks {
                let ask = crate::auto_review::AskUserTool::new(None).asking(Some(build.link));
                let ask: Arc<dyn yi_tools::Tool> = Arc::new(ask);
                child.use_tools(vec![ask], cwd.clone(), None);
            }
            Ok(child)
        }),
        notice: Arc::new(move |text: &str, _| notice(text.to_owned())),
        events,
        parent_messages: Arc::new(Vec::new),
        report: Arc::new(move |message, _| report(message)),
        attribute: Arc::new(|_usage| {}),
        store: Arc::new(|| None),
        plans_dir: root.join(crate::plan::PLANS_DIR),
        family_live: crate::fetch::KernelServiceMap::new(),
    }));
    let pins = Arc::new(FetchLog::new());
    let delegate = Arc::new(SessionDelegate::new(
        Arc::clone(&host),
        Arc::new(|_message, _mode| {}),
        Arc::clone(&pins),
    ));
    let engine = Arc::new(
        PlanEngine::new(PlanStore::open(root.join("plans"))?, delegate)
            .with_width(NonZeroUsize::new(2).ok_or("width")?)
            .with_cwd(root.to_path_buf()),
    );
    if finish {
        crate::plan::finish::install(&host, &engine, Arc::new(move |message, _mode| say(message)));
    }
    Ok(Rig {
        engine,
        host,
        pins,
        reports,
        notices,
        said,
        cwd: root,
    })
}

pub(in crate::plan) fn owner(op: Op) -> OpRequest {
    OpRequest {
        plan: None,
        actor: Actor::Owner,
        op,
        request_id: None,
        expected_revision: None,
    }
}

pub(in crate::plan) fn delegated(label: &str) -> Result<TodoSpec, Box<dyn std::error::Error>> {
    Ok(TodoSpec {
        label: TodoLabel::new(label)?,
        after: Vec::new(),
        delegation: Some(Delegation {
            spec: SpawnSpec {
                role: Some("worker".to_owned()),
                model: None,
                effort: None,
                tools: Vec::new(),
                isolation: None,
                budget: None,
                wall: None,
                parent_close: None,
                extra: Map::new(),
            },
            accept: Check::Command("true".to_owned()),
            output: None,
            context: Vec::new(),
            note: None,
            extra: Map::new(),
        }),
        contract: None,
        children: Vec::new(),
        cites: Default::default(),
    })
}

pub(in crate::plan) fn worker(name: &str) -> Map<String, Value> {
    let named = [("name", name), ("role", "root")];
    named
        .map(|(key, value)| (key.to_owned(), Value::from(value)))
        .into_iter()
        .collect()
}

pub(in crate::plan) async fn wait_done(host: &Arc<SubagentHost>) -> bool {
    let mut pace = backoff(std::time::Duration::from_millis(25));
    for _ in 0..400 {
        let done = host
            .children_view()
            .iter()
            .any(|child| child.update.status == ChildStatus::Completed);
        if done {
            return true;
        }
        tokio::time::sleep(pace()).await;
    }
    false
}

/// `until` re-read at every stir of the family (epoch, update, settle) until it holds.
pub(in crate::plan) async fn stirred_until(
    host: &SubagentHost,
    mut until: impl FnMut() -> Result<bool, Box<dyn std::error::Error>>,
) -> TestResult {
    let stirred = host
        .children
        .lock()
        .map(|children| Arc::clone(&children.stirred))
        .map_err(|_| "family state poisoned")?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(50);
    loop {
        let mut stir = std::pin::pin!(stirred.notified());
        stir.as_mut().enable();
        if until()? {
            return Ok(());
        }
        tokio::time::timeout_at(deadline, stir)
            .await
            .map_err(|_| "the family never reached the awaited state")?;
    }
}

pub(in crate::plan) fn texts(messages: &Arc<Mutex<Vec<AgentMessage>>>) -> Vec<String> {
    messages
        .lock()
        .map(|sink| {
            sink.iter()
                .filter_map(|message| match message {
                    AgentMessage::Custom {
                        content: UserContent::Text(text),
                        ..
                    } => Some(text.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The todo once the engine has stepped it and told the owner, polled from the store.
pub(in crate::plan) async fn settled(
    rig: &Rig,
    plan: &PlanId,
    label: &str,
) -> Result<yi_types::plan::doc::Todo, Box<dyn std::error::Error>> {
    let label = TodoLabel::new(label)?;
    let mut left = None;
    stirred_until(&rig.host, || {
        let read = rig.engine.store().read(plan)?;
        let todo = read.todo(&label).ok_or("todo missing")?;
        if !texts(&rig.said).is_empty() && !matches!(todo.state, TodoState::Running { .. }) {
            left = Some(todo.clone());
        }
        Ok(left.is_some())
    })
    .await?;
    left.ok_or_else(|| format!("{label:?} never left running").into())
}
