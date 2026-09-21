#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;
#[path = "support/family.rs"]
mod support;

use std::error::Error;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use yi_ai::faux::{faux_assistant_message, faux_text};
use yi_loop::ExecutionMode;
use yi_runtime::{
    AgentSession, HostRegistry, KernelService, KernelServiceOptions, ProviderStream, SessionConfig,
    SubagentHost, SubagentHostOptions,
};
use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, StopReason};
use yi_types::model::{Model, ModelCost};
use yi_types::subagent::{ChildActivity, ChildStatus, ChildUpdate};

type TestResult = Result<(), Box<dyn Error>>;

fn faux_model() -> Model {
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

fn child_reply(text: &str) -> AgentMessage {
    let mut message = faux_assistant_message(vec![faux_text(text)], StopReason::Stop);
    if let AgentMessage::Assistant { usage, .. } = &mut message {
        usage.input = 100;
        usage.output = 20;
        usage.total_tokens = 120;
    }
    message
}

struct Harness {
    host: Arc<SubagentHost>,
    notices: Arc<Mutex<Vec<String>>>,
    attributed: Arc<AtomicU32>,
    events: tokio::sync::broadcast::Sender<AgentEvent>,
    parent: Arc<Mutex<Vec<AgentMessage>>>,
    child_cwd: Arc<Mutex<Option<PathBuf>>>,
    inbox: Arc<Mutex<Vec<String>>>,
    entries: Arc<Mutex<Vec<AgentMessage>>>,
    store: yi_session::SharedSession,
    root: Scratch,
}

struct HarnessOptions {
    /// The child's one reply ends on a provider error instead of an answer.
    child_errors: bool,
    depth: u8,
    max_depth: u8,
    child_answer: &'static str,
    /// Some(command) makes the child call `bash` before answering, which is
    /// what moves the B7 tool counter and activity.
    tool_command: Option<&'static str>,
    /// The repository worktree children branch from.
    cwd: Option<PathBuf>,
    /// A parent whose lifecycle wake replaces the notice sink.
    wake_parent: Option<Arc<AgentSession>>,
}

fn harness(depth: u8, max_depth: u8, child_answer: &'static str) -> std::io::Result<Harness> {
    harness_with(HarnessOptions {
        child_errors: false,
        depth,
        max_depth,
        child_answer,
        tool_command: None,
        cwd: None,
        wake_parent: None,
    })
}

fn parent_session(replies: &[&str]) -> Arc<AgentSession> {
    Arc::new(parent_session_scripted(Vec::new(), replies))
}

/// `first` runs ahead of the text replies: a tool call there holds the parent's turn open.
fn parent_session_scripted(first: Vec<AgentMessage>, replies: &[&str]) -> AgentSession {
    let provider = Arc::new(ProviderStream::new(None, None));
    let mut script = first;
    for reply in replies {
        script.push(faux_assistant_message(
            vec![faux_text(reply)],
            StopReason::Stop,
        ));
    }
    provider.queue_faux(script);
    AgentSession::new(
        SessionConfig {
            system_prompt: "parent sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    )
}

fn notice_texts(messages: &[AgentMessage]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| match message {
            AgentMessage::User {
                content: yi_types::message::UserContent::Text(text),
                ..
            } if text.starts_with("[subagent ") => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn assistant_count(messages: &[AgentMessage]) -> usize {
    messages
        .iter()
        .filter(|message| matches!(message, AgentMessage::Assistant { .. }))
        .count()
}

fn harness_with(options: HarnessOptions) -> std::io::Result<Harness> {
    let HarnessOptions {
        child_errors,
        depth,
        max_depth,
        child_answer,
        tool_command,
        cwd,
        wake_parent,
    } = options;
    let root = Scratch::new("yi-recursion")?;
    let cwd = cwd.unwrap_or_else(std::env::temp_dir);
    let child_cwd: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
    let cwd_sink = Arc::clone(&child_cwd);
    let notices = Arc::new(Mutex::new(Vec::new()));
    let attributed = Arc::new(AtomicU32::new(0));
    let notice_sink = Arc::clone(&notices);
    let attribute_sink = Arc::clone(&attributed);
    let (events, _keep) = tokio::sync::broadcast::channel(256);
    let inbox: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let inbox_sink = Arc::clone(&inbox);
    let entries: Arc<Mutex<Vec<AgentMessage>>> = Arc::new(Mutex::new(Vec::new()));
    let entry_sink = Arc::clone(&entries);
    let store: yi_session::SharedSession = Arc::new(Mutex::new(
        yi_session::SessionStore::in_memory(yi_session::SessionMetadata {
            id: "recursion-test".to_owned(),
            created_at: 0,
            parent_session_id: None,
            name: None,
        }),
    ));
    let store_handle = store.clone();
    let parent: Arc<Mutex<Vec<AgentMessage>>> = Arc::new(Mutex::new(Vec::new()));
    let parent_source = Arc::clone(&parent);
    let host = Arc::new(SubagentHost::new(SubagentHostOptions {
        depth,
        max_depth,
        max_children: 8,
        parent_session_dir: root.to_path_buf(),
        cwd: cwd.clone(),
        home: root.join("home"),
        lane_slots: 2,
        defaults: Arc::new(|| (faux_model(), yi_types::model::Effort::Medium)),
        factory: Arc::new(move |build: yi_runtime::ChildBuild<'_>| {
            if let Ok(mut slot) = cwd_sink.lock() {
                *slot = build.cwd.map(std::path::Path::to_path_buf);
            }
            let provider = Arc::new(ProviderStream::new(None, None));
            let mut script = Vec::new();
            if let Some(command) = tool_command {
                let mut args = serde_json::Map::new();
                args.insert("command".to_owned(), Value::String(command.to_owned()));
                script.push(faux_assistant_message(
                    vec![yi_ai::faux::faux_tool_call("call-1", "bash", args)],
                    StopReason::ToolUse,
                ));
            }
            if child_errors {
                script.push(faux_assistant_message(
                    vec![faux_text(child_answer)],
                    StopReason::Error,
                ));
            }
            script.push(child_reply(child_answer));
            // A second scripted reply so a B13 followup has a turn to run.
            script.push(child_reply(child_answer));
            provider.queue_faux(script);
            let mut child = AgentSession::new(
                SessionConfig {
                    system_prompt: "child sys".to_owned(),
                    model: build.model,
                    thinking_level: build.thinking,
                    tool_execution: ExecutionMode::Sequential,
                },
                provider,
            );
            if tool_command.is_some() {
                child.use_tools(yi_tools::builtin_tools(), std::env::temp_dir(), None);
            }
            Ok(child)
        }),
        notice: match wake_parent {
            Some(parent) => yi_runtime::wiring::lifecycle_notice(&parent),
            None => Arc::new(move |text: &str| {
                if let Ok(mut sink) = notice_sink.lock() {
                    sink.push(text.to_owned());
                }
            }),
        },
        events: events.clone(),
        report: Arc::new(move |message| {
            if let Ok(mut sink) = entry_sink.lock() {
                sink.push(message.clone());
            }
            if let AgentMessage::Custom {
                content: yi_types::message::UserContent::Text(text),
                ..
            } = message
                && let Ok(mut sink) = inbox_sink.lock()
            {
                sink.push(text);
            }
        }),
        parent_messages: Arc::new(move || {
            parent_source
                .lock()
                .map(|messages| messages.clone())
                .unwrap_or_default()
        }),
        attribute: Arc::new(move |usage| {
            attribute_sink.fetch_add(u32::from(usage.total_tokens == 120), Ordering::SeqCst);
        }),
        store: Arc::new(move || Some(store_handle.clone())),
        plans_dir: cwd.join(".yi/plans"),
        family_live: Arc::new(|| 0),
    }));
    Ok(Harness {
        host,
        notices,
        attributed,
        events,
        parent,
        child_cwd,
        inbox,
        entries,
        store,
        root,
    })
}

/// A poll budget, not a sleep: every loop below exits on its first true
/// predicate, so a wider bound costs nothing when green and only buys headroom
/// where the old 3 s ceiling failed — a loaded shared runner.
const POLL_ATTEMPTS: usize = 400;
const POLL_INTERVAL_MS: u64 = 30;

fn kwargs(pairs: &[(&str, &str)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), Value::String((*value).to_owned())))
        .collect()
}

async fn wait_for_status(host: &Arc<SubagentHost>, child_id: &str, status: &str) -> bool {
    for _ in 0..POLL_ATTEMPTS {
        let list = host.list();
        let found = list["subagents"].as_array().and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry["rlm_child_id"] == child_id)
        });
        if found.is_some_and(|entry| entry["status"] == status) {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    false
}

#[tokio::test]
async fn spawn_runs_child_to_completion_with_notice_and_attribution() -> TestResult {
    let harness = harness(0, 1, "the answer is forty-two")?;
    let reply = harness
        .host
        .spawn(
            "compute the answer".to_owned(),
            kwargs(&[("name", "helper")]),
        )
        .map_err(|error| error.to_string())?;
    let child_id = reply["rlm_child_id"]
        .as_str()
        .ok_or("missing child id")?
        .to_owned();
    assert!(child_id.starts_with("sub-"));
    assert_eq!(reply["name"], "helper");
    assert_eq!(reply["model"], "faux/faux-1");
    assert!(
        harness.root.join(&child_id).is_dir(),
        "child session dir must exist"
    );

    assert!(
        wait_for_status(&harness.host, &child_id, "completed").await,
        "child must reach completed"
    );
    assert_eq!(
        harness.attributed.load(Ordering::SeqCst),
        1,
        "child usage must attribute once"
    );
    let notices = harness.notices.lock().map_err(|_| "poisoned")?.clone();
    assert_eq!(notices.len(), 1);
    assert!(
        notices[0].contains("sent you no message")
            && notices[0].contains("the answer is forty-two"),
        "terminal notice must carry the answer preview: {}",
        notices[0]
    );
    Ok(())
}

async fn collect_updates(
    events: &mut tokio::sync::broadcast::Receiver<AgentEvent>,
) -> Vec<ChildUpdate> {
    let mut seen = Vec::new();
    while let Ok(Ok(event)) =
        tokio::time::timeout(std::time::Duration::from_secs(20), events.recv()).await
    {
        if let AgentEvent::ChildUpdate { update } = event {
            let terminal = update.status != ChildStatus::Running;
            seen.push(update);
            if terminal {
                break;
            }
        }
    }
    seen
}

#[tokio::test]
async fn child_updates_ride_the_parent_bus_with_counts_and_activity() -> TestResult {
    let harness = harness_with(HarnessOptions {
        child_errors: false,
        depth: 0,
        max_depth: 1,
        child_answer: "swept the logs",
        tool_command: Some("echo probing"),
        cwd: None,
        wake_parent: None,
    })?;
    let mut events = harness.events.subscribe();
    harness
        .host
        .spawn("sweep the logs".to_owned(), kwargs(&[("name", "sweeper")]))
        .map_err(|error| error.to_string())?;

    let updates = collect_updates(&mut events).await;
    let first = updates.first().ok_or("no update at admission")?;
    assert_eq!(first.name, "sweeper");
    assert_eq!(first.status, ChildStatus::Running);
    assert!(
        updates
            .iter()
            .any(|update| update.activity == ChildActivity::Executing),
        "the tool call must be visible as activity, not inferred: {updates:?}"
    );
    let last = updates.last().ok_or("no terminal update")?;
    assert_eq!(last.status, ChildStatus::Completed);
    assert_eq!(last.tool_use_count, 1, "one bash call, counted once");
    assert_eq!(last.token_count, 120, "the child's usage is reported");
    assert_eq!(
        last.answer_preview.as_deref(),
        Some("swept the logs"),
        "the answer preview crosses without the parent reading the child's stream"
    );
    Ok(())
}

fn parent_turn(user: &str, answer: &str) -> [AgentMessage; 2] {
    [
        AgentMessage::host_user(yi_types::message::UserContent::Text(user.to_owned()), 0),
        child_reply(answer),
    ]
}

async fn first_child_messages(harness: &Harness) -> Vec<AgentMessage> {
    let Some(child) = harness.host.children_view().into_iter().next() else {
        return Vec::new();
    };
    for _ in 0..POLL_ATTEMPTS {
        child.session.wait_idle().await;
        let messages = child.session.messages();
        if user_texts(&messages)
            .iter()
            .any(|text| text.starts_with("[task from parent]"))
        {
            return messages;
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    Vec::new()
}

/// Everything the child was handed: its task, and any agent message since.
fn inbound_texts(messages: &[AgentMessage]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| match message {
            AgentMessage::User {
                content: yi_types::message::UserContent::Text(text),
                ..
            }
            | AgentMessage::Custom {
                content: yi_types::message::UserContent::Text(text),
                ..
            } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn user_texts(messages: &[AgentMessage]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| match message {
            AgentMessage::User {
                content: yi_types::message::UserContent::Text(text),
                ..
            } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn fork_seeds_the_child_with_the_turns_it_asked_for() -> TestResult {
    let cold = harness(0, 1, "cold")?;
    {
        let mut parent = cold.parent.lock().map_err(|_| "poisoned")?;
        parent.extend(parent_turn("turn one", "a1"));
        parent.extend(parent_turn("turn two", "a2"));
        parent.extend(parent_turn("turn three", "a3"));
    }
    cold.host
        .spawn("do the thing".to_owned(), kwargs(&[("name", "cold")]))
        .map_err(|error| error.to_string())?;
    assert_eq!(
        user_texts(&first_child_messages(&cold).await),
        vec!["[task from parent]\n\ndo the thing".to_owned()],
        "no fork means a cold child: the parent's turns never cross"
    );

    let forked = harness(0, 1, "forked")?;
    {
        let mut parent = forked.parent.lock().map_err(|_| "poisoned")?;
        parent.extend(parent_turn("turn one", "a1"));
        parent.extend(parent_turn("turn two", "a2"));
        parent.extend(parent_turn("turn three", "a3"));
    }
    forked
        .host
        .spawn(
            "continue it".to_owned(),
            kwargs(&[("name", "last-one"), ("fork", "1")]),
        )
        .map_err(|error| error.to_string())?;
    assert_eq!(
        user_texts(&first_child_messages(&forked).await),
        vec![
            "turn three".to_owned(),
            "[task from parent]\n\ncontinue it".to_owned()
        ],
        "fork=1 seeds the last turn boundary onward, nothing older"
    );

    let whole = harness(0, 1, "whole")?;
    {
        let mut parent = whole.parent.lock().map_err(|_| "poisoned")?;
        parent.extend(parent_turn("turn one", "a1"));
        parent.extend(parent_turn("turn two", "a2"));
    }
    whole
        .host
        .spawn(
            "carry on".to_owned(),
            kwargs(&[("name", "all"), ("fork", "all")]),
        )
        .map_err(|error| error.to_string())?;
    assert_eq!(
        user_texts(&first_child_messages(&whole).await).len(),
        3,
        "fork=all seeds both parent turns plus the task"
    );
    let override_refused = whole.host.spawn(
        "carry on".to_owned(),
        kwargs(&[
            ("name", "all-override"),
            ("fork", "all"),
            ("model", "faux/faux-1"),
        ]),
    );
    assert_eq!(
        override_refused.err().as_deref(),
        Some("fork=all inherits the parent's model and thinking; drop the override"),
        "an All fork rejects overrides rather than silently ignoring them"
    );
    let bad_fork = whole.host.spawn(
        "nope".to_owned(),
        kwargs(&[("name", "bad"), ("fork", "-2")]),
    );
    assert!(
        bad_fork
            .err()
            .is_some_and(|error| error.contains("positive turn count")),
        "a malformed fork names the vocabulary"
    );
    Ok(())
}

async fn child_sees(harness: &Harness, name: &str, needle: &str) -> bool {
    for _ in 0..POLL_ATTEMPTS {
        let seen = harness
            .host
            .children_view()
            .into_iter()
            .filter(|child| child.update.name == name)
            .any(|child| {
                inbound_texts(&child.session.messages())
                    .iter()
                    .any(|text| text.contains(needle))
            });
        if seen {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    false
}

#[tokio::test]
async fn agent_messages_route_by_name_and_broadcast_with_receipts() -> TestResult {
    let harness = harness(0, 1, "ok")?;
    for name in ["alpha", "beta"] {
        harness
            .host
            .spawn(format!("work {name}"), kwargs(&[("name", name)]))
            .map_err(|error| error.to_string())?;
        assert!(child_sees(&harness, name, "[task from parent]").await);
    }

    harness.host.wait(0, None).await;
    let inboxed = harness
        .host
        .route("parent", "beta", "no rush", false)
        .map_err(|error| error.to_string())?;
    assert_eq!(
        inboxed["receipts"][0]["state"], "inboxed",
        "beta's turn is over, so nothing will drain a queue: the receipt says so"
    );
    assert!(
        !child_sees(&harness, "beta", "no rush").await,
        "a plain send never starts a turn: it waits for the next one"
    );

    harness
        .host
        .route("parent", "alpha", "look at this now", true)
        .map_err(|error| error.to_string())?;
    assert!(
        child_sees(&harness, "alpha", "<agent_message from=\"parent\">").await,
        "a followup reaches the child inside the provenance envelope"
    );

    let broadcast = harness
        .host
        .route("alpha", "all", "siblings, status?", false)
        .map_err(|error| error.to_string())?;
    let receipts = broadcast["receipts"]
        .as_array()
        .ok_or("no receipts")?
        .clone();
    assert_eq!(receipts.len(), 1, "the sender is not its own audience");
    assert_eq!(receipts[0]["target"], "beta");

    let unknown = harness.host.route("parent", "gamma", "hello", false);
    assert!(
        unknown.err().is_some_and(
            |error| error.contains("no agent named \"gamma\"") && error.contains("alpha")
        ),
        "an unknown target names the children that do exist"
    );
    assert!(
        harness
            .host
            .route("parent", "parent", "hi", false)
            .err()
            .is_some_and(|error| error.contains("cannot send to itself"))
    );
    Ok(())
}

/// Every envelope a child's turns were handed, as `(from, seq, body)` in presented order.
fn presented(harness: &Harness, name: &str) -> Vec<(String, u64, String)> {
    let child = harness
        .host
        .children_view()
        .into_iter()
        .find(|child| child.update.name == name);
    let messages = child
        .map(|child| child.session.messages())
        .unwrap_or_default();
    messages
        .iter()
        .filter_map(|message| match message {
            AgentMessage::Custom {
                details: Some(mail),
                ..
            } => Some((
                mail["from"].as_str()?.to_owned(),
                mail["seq"].as_u64()?,
                mail["body"].as_str()?.to_owned(),
            )),
            _ => None,
        })
        .collect()
}

async fn finished(harness: &Harness, name: &str) -> Result<(), String> {
    let kwargs = kwargs(&[("name", name)]);
    let reply = harness.host.spawn(format!("work {name}"), kwargs)?;
    let id = reply["rlm_child_id"].as_str().ok_or("no child id")?;
    match wait_for_status(&harness.host, id, "completed").await {
        true => Ok(()),
        false => Err(format!("{name} never completed")),
    }
}

/// A child whose first turn a two-second tool call holds open, returned once it is running.
async fn busy_child() -> Result<(Harness, yi_runtime::ChildFeed), Box<dyn Error>> {
    let harness = harness_with(HarnessOptions {
        child_errors: false,
        depth: 0,
        max_depth: 1,
        child_answer: "ok",
        tool_command: Some("sleep 2"),
        cwd: None,
        wake_parent: None,
    })?;
    let kwargs = kwargs(&[("name", "busy")]);
    harness.host.spawn("hold a turn open".to_owned(), kwargs)?;
    let busy = harness
        .host
        .children_view()
        .pop()
        .ok_or("no child")?
        .session;
    while busy.status() != yi_runtime::Status::Running {
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    Ok((harness, busy))
}

fn state_of(reply: &Map<String, Value>) -> &str {
    reply["receipts"][0]["state"].as_str().unwrap_or_default()
}

/// The inbox of `name` as the host wrote it: one JSON entry per accepted envelope.
fn inbox_of(harness: &Harness, name: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let store = harness.host.transcript(name).ok_or("no transcript")?;
    let entries = yi_runtime::session_store::lock_session(&store).find_entries(
        &yi_runtime::session_store::EntryQuery {
            custom_type: Some("agent_message".to_owned()),
            order: yi_runtime::session_store::EntryOrder::OldestFirst,
            ..Default::default()
        },
    )?;
    Ok(entries
        .iter()
        .map(|entry| serde_json::to_string(entry).unwrap_or_default())
        .collect())
}

#[tokio::test]
async fn a_body_over_sixteen_kib_is_refused_not_trimmed() -> TestResult {
    let harness = harness(0, 1, "ok")?;
    finished(&harness, "reader").await?;
    let over = "x".repeat(16 * 1024 + 1);
    let error = harness
        .host
        .route("parent", "reader", &over, false)
        .err()
        .ok_or("accepted")?;
    assert!(
        error.contains("nothing was sent")
            && error.contains("rlm.put")
            && error.contains("family://"),
        "the refusal names the alternative: {error}"
    );
    assert!(
        inbox_of(&harness, "reader")?.is_empty(),
        "no trimmed copy was inboxed"
    );
    harness.host.route("parent", "reader", &over[1..], false)?;
    assert!(
        inbox_of(&harness, "reader")?[0].contains(&over[1..]),
        "a body at the cap arrives whole"
    );
    Ok(())
}

#[tokio::test]
async fn send_returns_queued_woken_or_inboxed() -> TestResult {
    let (harness, busy) = busy_child().await?;
    let plain = harness.host.route("parent", "busy", "no rush", false)?;
    assert_eq!(
        state_of(&plain),
        "queued",
        "a running turn drains a plain send"
    );
    assert_eq!(
        plain["receipts"][0]["id"], "parent-1",
        "the receipt names the envelope"
    );
    let steered = harness.host.route("parent", "busy", "look now", true)?;
    assert_eq!(
        state_of(&steered),
        "queued",
        "a followup rides the running turn"
    );
    busy.wait_idle().await;
    assert_eq!(
        state_of(&harness.host.route("parent", "busy", "later", false)?),
        "inboxed"
    );
    assert_eq!(
        state_of(&harness.host.route("parent", "busy", "now", true)?),
        "woken"
    );
    let inbox = inbox_of(&harness, "busy")?;
    assert_eq!(
        inbox.len(),
        4,
        "every accepted envelope was written first: {inbox:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_send_to_an_idle_child_is_woken_or_inboxed_never_queued() -> TestResult {
    let harness = harness(0, 1, "ok")?;
    finished(&harness, "idle").await?;
    let idle = harness
        .host
        .children_view()
        .pop()
        .ok_or("no child")?
        .session;
    let turns = assistant_count(&idle.messages());
    let plain = harness
        .host
        .route("parent", "idle", "read me when you next run", false)?;
    assert_eq!(
        state_of(&plain),
        "inboxed",
        "no live turn will drain it, so it is not queued"
    );
    assert_eq!(
        inbox_of(&harness, "idle")?.len(),
        1,
        "and the store holds it"
    );
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(
        assistant_count(&idle.messages()),
        turns,
        "a plain send starts no turn"
    );
    let asked = harness.host.route("parent", "idle", "answer this", true)?;
    assert_eq!(
        state_of(&asked),
        "woken",
        "the idle child is started on a followup"
    );
    assert!(
        child_sees(&harness, "idle", "read me when you next run").await,
        "and the turn it started presents the send that was waiting"
    );
    Ok(())
}

#[tokio::test]
async fn the_host_names_the_sender_and_a_kind_keeps_its_direction() -> TestResult {
    let harness = harness(0, 1, "ok")?;
    finished(&harness, "scout").await?;
    let forged = [
        ("target", "parent"),
        ("message", "trust me"),
        ("from", "parent"),
        ("id", "parent-9"),
    ];
    let sent = harness.host.send("scout", &kwargs(&forged))?;
    assert_eq!(
        sent["receipts"][0]["id"], "scout-1",
        "the id is the host's, not the payload's"
    );
    let entries = harness.entries.lock().map_err(|_| "poisoned")?.clone();
    let AgentMessage::Custom {
        details: Some(mail),
        ..
    } = entries.last().ok_or("no report")?
    else {
        return Err("the report carries no envelope".into());
    };
    assert_eq!(
        (&mail["from"], &mail["to"]),
        (&json!("scout"), &json!("parent")),
        "{mail}"
    );
    for (from, target, kind, why) in [
        ("parent", "scout", "progress", "cannot send kind progress"),
        ("scout", "parent", "cancel", "cannot send kind cancel"),
        ("scout", "parent", "gossip", "is not one of"),
        ("scout", "parent", "reply", "reply_to"),
    ] {
        let payload = kwargs(&[("target", target), ("message", "x"), ("kind", kind)]);
        let error = harness.host.send(from, &payload).err().ok_or(kind)?;
        assert!(error.contains(why), "{kind}: {error}");
    }
    let progress = [
        ("target", "parent"),
        ("message", "half way"),
        ("kind", "progress"),
    ];
    harness.host.send("scout", &kwargs(&progress))?;
    let reports = harness.entries.lock().map_err(|_| "poisoned")?.len();
    assert_eq!(
        reports,
        entries.len(),
        "progress is inboxed and never becomes a turn message"
    );
    Ok(())
}

#[tokio::test]
async fn two_messages_from_one_sender_drain_in_seq_order() -> TestResult {
    let (harness, _busy) = busy_child().await?;
    for (from, text) in [
        ("parent", "one"),
        ("scout", "aside"),
        ("parent", "two"),
        ("parent", "three"),
    ] {
        let reply = harness.host.route(from, "busy", text, false)?;
        assert_eq!(
            reply["receipts"][0]["state"], "queued",
            "the open turn drains {text}"
        );
    }
    for _ in 0..POLL_ATTEMPTS {
        if presented(&harness, "busy").len() == 4 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    let seen = presented(&harness, "busy");
    let from_parent: Vec<(u64, &str)> = seen
        .iter()
        .filter(|(from, ..)| from == "parent")
        .map(|(_, seq, body)| (*seq, body.as_str()))
        .collect();
    assert_eq!(
        from_parent,
        vec![(1, "one"), (2, "two"), (3, "three")],
        "one sender's messages are numbered per pair and presented in that order: {seen:?}"
    );
    assert!(
        seen.contains(&("scout".to_owned(), 1, "aside".to_owned())),
        "another sender counts on its own and is never sorted into the first: {seen:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_message_to_a_finished_child_is_inboxed_and_readable_by_history() -> TestResult {
    let harness = harness(0, 1, "ok")?;
    finished(&harness, "done").await?;
    let desk =
        yi_runtime::fetch::SessionTranscripts::new(Arc::clone(&harness.host), None, &harness.root);
    let resolver =
        yi_runtime::fetch::Resolver::new(harness.root.to_path_buf(), yi_runtime::Wall::default())
            .with_transcripts(Arc::new(desk));
    let first = harness
        .host
        .route("parent", "done", "kept for later", false)?;
    assert_eq!(first["receipts"][0]["state"], "inboxed");
    let inbox: yi_types::url::Url = "history://done/custom/agent_message".parse()?;
    let served = resolver.fetch(&inbox)?;
    assert_eq!(
        served.text.lines().count(),
        1,
        "the inbox holds the envelope alone: {}",
        served.text
    );
    assert!(
        served.text.contains("\"body\":\"kept for later\"") && served.text.contains("\"seq\":1")
    );
    assert_eq!(
        resolver.fetch(&inbox)?.text,
        served.text,
        "reading an inbox does not grow it"
    );

    harness.host.reap("done")?;
    let late = harness
        .host
        .route("parent", "done", "after the reap", false)?;
    assert_eq!(
        late["receipts"][0]["state"], "inboxed",
        "a retired child keeps its inbox"
    );
    let page = |offset| yi_runtime::fetch::Page { offset, limit: 1 };
    let head = resolver.fetch_page(&inbox, Some(page(0)))?;
    assert!(head.text.contains("kept for later") && head.next_offset == Some(1));
    let tail = resolver.fetch_page(&inbox, Some(page(1)))?;
    assert!(tail.text.contains("after the reap") && tail.next_offset.is_none());
    let since: yi_types::url::Url = "history://done/tail/1/custom/agent_message".parse()?;
    assert!(resolver.fetch(&since)?.text.contains("after the reap"));
    assert!(
        resolver.fetch_page(&since, Some(page(0))).is_err(),
        "a tail slides under an append, so it refuses a page"
    );
    let gone = harness.host.route("parent", "never-was", "hello", false);
    assert!(gone.is_err(), "a name nobody held has no inbox to write");
    Ok(())
}

#[tokio::test]
async fn request_returns_the_matching_reply_and_times_out_without_one() -> TestResult {
    let harness = harness(0, 1, "ok")?;
    finished(&harness, "oracle").await?;
    finished(&harness, "bystander").await?;
    let host = Arc::clone(&harness.host);
    let asking = tokio::spawn(async move {
        host.request("parent", "oracle", "which suite is red?", 20_000)
            .await
    });
    assert!(
        child_sees(&harness, "oracle", "reply_to=\"parent-1\"").await,
        "the request wakes its respondent and names the call that answers it"
    );
    let reply = |text: &str| -> Map<String, Value> {
        let pairs = [
            ("target", "parent"),
            ("message", text),
            ("reply_to", "parent-1"),
        ];
        kwargs(&pairs)
    };
    harness
        .host
        .send("bystander", &reply("not mine to answer"))?;
    let elsewhere = [
        ("target", "bystander"),
        ("message", "meant for a sibling"),
        ("reply_to", "parent-1"),
    ];
    harness.host.send("oracle", &kwargs(&elsewhere))?;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        !asking.is_finished(),
        "only the named respondent, answering the sender that asked, resolves a request"
    );
    harness.host.send("oracle", &reply("the fetch suite"))?;
    let answer = asking.await??;
    assert_eq!(answer["reply"], "the fetch suite");
    assert_eq!(answer["envelope"]["inReplyTo"], "parent-1");
    assert_eq!(answer["envelope"]["conversation"], "parent-1");

    let silent = harness
        .host
        .request("parent", "bystander", "anyone?", 1_000)
        .await;
    let error = silent
        .err()
        .ok_or("a request nobody answers must time out")?;
    assert!(
        error.contains("no reply to parent-") && error.contains("bystander"),
        "{error}"
    );
    harness.host.send(
        "bystander",
        &kwargs(&[
            ("target", "parent"),
            ("message", "late"),
            ("reply_to", "parent-5"),
        ]),
    )?;
    let inbox = harness.inbox.lock().map_err(|_| "poisoned")?.clone();
    assert!(
        inbox.iter().any(|text| text.contains("late")),
        "a late reply lands in history and resolves nothing: {inbox:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_child_reports_upward_and_the_parent_waits_for_it() -> TestResult {
    let harness = harness(0, 1, "ok")?;
    harness
        .host
        .spawn("do it".to_owned(), kwargs(&[("name", "scout")]))
        .map_err(|error| error.to_string())?;
    assert!(child_sees(&harness, "scout", "[task from parent]").await);
    // Reads past the terminal transition so the wait below observes the report only.
    let settled = harness.host.wait(0, None).await;
    let cursor = settled["cursor"].as_u64();

    let link = yi_runtime::ParentLink {
        child_name: "scout".to_owned(),
        host: Arc::downgrade(&harness.host),
    };
    link.send("parent", "found the leak in run.rs", false)
        .map_err(|error| error.to_string())?;
    let inbox = harness.inbox.lock().map_err(|_| "poisoned")?.clone();
    assert!(
        inbox
            .iter()
            .any(|text| text.contains("<agent_message from=\"scout\">")
                && text.contains("found the leak in run.rs")),
        "a child's report reaches the parent as provenanced data: {inbox:?}"
    );

    let woken = harness.host.wait(60_000, cursor).await;
    assert_eq!(woken["updated"], serde_json::json!(["scout"]));
    let quiet = harness.host.wait(0, woken["cursor"].as_u64()).await;
    assert_eq!(
        quiet["updated"],
        serde_json::json!([]),
        "an update is behind the cursor it was read at, not reported forever"
    );
    assert_eq!(quiet["timeout_ms"], 1000, "the clamp is applied");
    assert_eq!(quiet["clamped"], true, "and reported");
    Ok(())
}

#[tokio::test]
async fn a_finished_child_hands_back_a_schema_checked_result() -> TestResult {
    let harness = harness(0, 1, "{\"files\": 3}")?;
    harness
        .host
        .spawn("count files".to_owned(), kwargs(&[("name", "counter")]))
        .map_err(|error| error.to_string())?;
    assert!(wait_for_status_named(&harness, "counter").await);

    let schema = serde_json::json!({
        "type": "object",
        "properties": {"files": {"type": "number"}},
        "required": ["files"]
    });
    let result = harness
        .host
        .result("counter", Some(&schema))
        .map_err(|error| error.to_string())?;
    assert_eq!(result["json"]["files"], 3);

    let wrong = serde_json::json!({
        "type": "object",
        "properties": {"paths": {"type": "array"}},
        "required": ["paths"]
    });
    assert!(
        harness
            .host
            .result("counter", Some(&wrong))
            .err()
            .is_some_and(|error| error.contains("result rejected")),
        "a result that misses its schema is refused at the seam, not passed on"
    );
    assert!(
        harness
            .host
            .interrupt("counter")
            .is_ok_and(|reply| reply.contains_key("interrupted")),
        "interrupt keeps the record where delete reaps it"
    );
    assert_eq!(
        harness.host.children_view().len(),
        1,
        "an interrupted child is still on the roster"
    );
    Ok(())
}

#[tokio::test]
async fn a_malformed_schema_is_refused_before_the_answer_is_read() -> TestResult {
    let harness = harness(0, 1, "{\"files\": 3}")?;
    harness
        .host
        .spawn("count files".to_owned(), kwargs(&[("name", "counter")]))
        .map_err(|error| error.to_string())?;
    assert!(wait_for_status_named(&harness, "counter").await);

    assert!(
        harness
            .host
            .result("counter", Some(&json!(30)))
            .err()
            .is_some_and(|error| error.contains("schema rejected")),
        "a schema that is not an object is the caller's error, not the child's"
    );
    Ok(())
}

#[tokio::test]
async fn a_child_that_messaged_its_parent_finishes_without_the_silent_note() -> TestResult {
    let harness = harness(0, 1, "ok")?;
    let reply = harness
        .host
        .spawn("do it".to_owned(), kwargs(&[("name", "scout")]))
        .map_err(|error| error.to_string())?;
    let child_id = reply["rlm_child_id"]
        .as_str()
        .ok_or("missing child id")?
        .to_owned();
    // The child task cannot run until this test awaits, so the report is in
    // before the terminal notice is built.
    yi_runtime::ParentLink {
        child_name: "scout".to_owned(),
        host: Arc::downgrade(&harness.host),
    }
    .send("parent", "hello", false)
    .map_err(|error| error.to_string())?;

    assert!(
        wait_for_status(&harness.host, &child_id, "completed").await,
        "child must reach completed"
    );
    let notices = harness.notices.lock().map_err(|_| "poisoned")?.clone();
    assert_eq!(notices.len(), 1);
    assert!(
        notices[0].contains("finished]")
            && notices[0].contains("Last answer: ok")
            && !notices[0].contains("sent you no message"),
        "a child that reported is not told it stayed silent: {}",
        notices[0]
    );
    Ok(())
}

async fn wait_for_status_named(harness: &Harness, name: &str) -> bool {
    for _ in 0..POLL_ATTEMPTS {
        if harness
            .host
            .children_view()
            .iter()
            .any(|child| child.update.name == name && child.update.status != ChildStatus::Running)
        {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    false
}

fn branches(repo: &std::path::Path) -> Result<String, Box<dyn Error>> {
    let output = yi_tools::command("git")
        .current_dir(repo)
        .args(["branch", "--list", "yi/*"])
        .output()?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn git_repo(label: &str) -> Result<Scratch, Box<dyn Error>> {
    let repo = Scratch::new(&format!("yi-wt-{label}"))?;
    let run = |args: &[&str]| -> Result<(), Box<dyn Error>> {
        let status = yi_tools::command("git")
            .current_dir(&repo)
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("git {} failed", args.join(" ")).into())
        }
    };
    run(&["init", "-q", "-b", "main"])?;
    run(&["config", "user.email", "test@example.invalid"])?;
    run(&["config", "user.name", "Yi Test"])?;
    std::fs::write(repo.join("README.md"), "base\n")?;
    run(&["add", "-A"])?;
    run(&["commit", "-qm", "base"])?;
    Ok(repo)
}

#[tokio::test]
async fn a_worktree_child_gets_its_own_checkout_and_hands_it_back() -> TestResult {
    let repo = git_repo("merge")?;
    let harness = harness_with(HarnessOptions {
        child_errors: false,
        depth: 0,
        max_depth: 1,
        child_answer: "isolated",
        tool_command: None,
        cwd: Some(repo.to_path_buf()),
        wake_parent: None,
    })?;
    let reply = harness
        .host
        .spawn(
            "edit in isolation".to_owned(),
            kwargs(&[("name", "mutator"), ("isolation", "worktree")]),
        )
        .map_err(|error| error.to_string())?;
    let child_id = reply["rlm_child_id"]
        .as_str()
        .ok_or("missing child id")?
        .to_owned();
    let tree = harness
        .child_cwd
        .lock()
        .map_err(|_| "poisoned")?
        .clone()
        .ok_or("the child was built without its worktree as cwd")?;
    assert!(
        tree.join("README.md").is_file(),
        "the child works in a real checkout of the parent's HEAD: {}",
        tree.display()
    );
    assert!(
        wait_for_status(&harness.host, &child_id, "completed").await,
        "child must finish"
    );

    std::fs::write(tree.join("child.txt"), "written by the child\n")?;
    assert!(
        harness
            .host
            .delete("mutator")
            .err()
            .is_some_and(|error| error.contains("merge or discard it first")),
        "reaping a child with unmerged work must refuse, not drop the tree"
    );
    let merged = harness
        .host
        .merge_worktree("mutator")
        .map_err(|error| error.to_string())?;
    assert_eq!(merged["merged"], true);
    assert_eq!(
        std::fs::read_to_string(repo.join("child.txt"))?,
        "written by the child\n",
        "the child's work lands in the parent's checkout"
    );
    assert!(
        branches(&repo)?
            .lines()
            .all(|line| !line.contains(&format!("yi/{child_id}"))),
        "a merged branch is deleted and the slot goes back to the pool"
    );
    assert!(
        harness.host.delete("mutator").is_ok(),
        "once merged, the slot is reapable"
    );
    Ok(())
}

#[tokio::test]
async fn discarding_a_worktree_throws_the_branch_away() -> TestResult {
    let repo = git_repo("discard")?;
    let harness = harness_with(HarnessOptions {
        child_errors: false,
        depth: 0,
        max_depth: 1,
        child_answer: "discarded",
        tool_command: None,
        cwd: Some(repo.to_path_buf()),
        wake_parent: None,
    })?;
    let reply = harness
        .host
        .spawn(
            "try something".to_owned(),
            kwargs(&[("name", "spike"), ("isolation", "worktree")]),
        )
        .map_err(|error| error.to_string())?;
    let child_id = reply["rlm_child_id"]
        .as_str()
        .ok_or("missing child id")?
        .to_owned();
    assert!(wait_for_status(&harness.host, &child_id, "completed").await);
    let tree = harness
        .child_cwd
        .lock()
        .map_err(|_| "poisoned")?
        .clone()
        .ok_or("no worktree")?;
    std::fs::write(tree.join("spike.txt"), "throwaway\n")?;
    harness
        .host
        .discard_worktree("spike")
        .map_err(|error| error.to_string())?;
    assert!(!repo.join("spike.txt").exists(), "nothing crosses back");
    assert!(
        branches(&repo)?
            .lines()
            .all(|line| !line.contains(&format!("yi/{child_id}"))),
        "a discarded branch is deleted; the slot itself is pooled, not removed"
    );
    assert!(
        harness
            .host
            .discard_worktree("spike")
            .err()
            .is_some_and(|error| error.contains("has no worktree")),
        "a second hand-back names the reason rather than half-working"
    );
    Ok(())
}

#[tokio::test]
async fn depth_limit_name_collision_slots_and_delete() -> TestResult {
    let at_limit = harness(1, 1, "unused")?;
    let refused = at_limit.host.spawn("nested".to_owned(), Map::new());
    assert_eq!(
        refused.err().as_deref(),
        Some("RLM recursion depth limit reached (RLM_DEPTH=1, RLM_MAX_DEPTH=1)"),
        "a depth-1 child must not spawn grandchildren"
    );

    let harness = harness(0, 1, "ok")?;
    harness
        .host
        .spawn("first".to_owned(), kwargs(&[("name", "twin")]))
        .map_err(|error| error.to_string())?;
    let collision = harness
        .host
        .spawn("second".to_owned(), kwargs(&[("name", "twin")]));
    assert!(
        collision
            .err()
            .is_some_and(|error| error.contains("\"twin\" is already taken at depth 1")),
        "duplicate names must be rejected"
    );

    for index in 0..7 {
        harness
            .host
            .spawn(
                format!("filler {index}"),
                kwargs(&[("name", &format!("filler-{index}"))]),
            )
            .map_err(|error| error.to_string())?;
    }
    let overflow = harness.host.spawn("ninth".to_owned(), Map::new());
    assert!(
        overflow
            .err()
            .is_some_and(|error| error.contains("child limit reached")),
        "a completed child must hold its slot until closed"
    );

    let deleted = harness
        .host
        .delete("twin")
        .map_err(|error| error.to_string())?;
    assert_eq!(deleted["subagent"]["session_name"], "twin");
    let after = harness
        .host
        .spawn("tenth".to_owned(), kwargs(&[("name", "tenth")]));
    assert!(
        after.is_ok(),
        "deleting a child must release its slot: {after:?}"
    );

    assert!(harness.host.delete("no-such-child").is_err());

    let unknown_kwarg = harness
        .host
        .spawn("bad".to_owned(), kwargs(&[("name", "bad"), ("junk", "1")]));
    assert_eq!(
        unknown_kwarg.err().as_deref(),
        Some("Unsupported rlm.run kwargs: junk"),
        "unknown kwargs are an error, never silently dropped"
    );
    Ok(())
}

#[tokio::test]
async fn rlm_run_round_trips_through_a_real_kernel() -> TestResult {
    let harness = harness(0, 1, "kernel child says hi")?;
    let mut registry = HostRegistry::default();
    registry.register_mcp_stubs();
    harness.host.register(&mut registry);
    let service = Arc::new(KernelService::new(KernelServiceOptions {
        cwd: std::env::temp_dir(),
        home: std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default(),
        session_dir: Some(harness.root.to_path_buf()),
        family_dir: None,
        host: Arc::new(registry),
        on_restore: None,
        sandbox: None,
        snapshot_key: None,
        cell_ceiling: None,
    }));

    let cancelled: yi_tools::CancelFlag = Arc::new(|| false);
    let spawn_cell = tokio::task::spawn_blocking({
        let service = Arc::clone(&service);
        let cancelled = Arc::clone(&cancelled);
        move || {
            yi_tools::KernelBridge::execute_cell(
                service.as_ref(),
                "h = await rlm.run('greet the parent', name='helper')\nprint(h.rlm_child_id.startswith('sub-'), h.name, h.model)",
                &cancelled,
            )
        }
    })
    .await??;
    assert!(
        spawn_cell.result.stdout.contains("True helper faux/faux-1"),
        "rlm.run must return a validated spawn handle: {} {}",
        spawn_cell.result.stdout,
        spawn_cell.result.stderr
    );

    let list_cell = tokio::task::spawn_blocking({
        let service = Arc::clone(&service);
        let cancelled = Arc::clone(&cancelled);
        move || {
            yi_tools::KernelBridge::execute_cell(
                service.as_ref(),
                "import asyncio\nfor _ in range(100):\n    subs = await rlm.list_subagents()\n    if subs and subs[0].status == 'completed':\n        break\n    await asyncio.sleep(0.05)\nprint(subs[0].session_name, subs[0].status)",
                &cancelled,
            )
        }
    })
    .await??;
    assert!(
        list_cell.result.stdout.contains("helper completed"),
        "rlm.list_subagents must expose the completed child: {} {}",
        list_cell.result.stdout,
        list_cell.result.stderr
    );

    let roster_cell = tokio::task::spawn_blocking({
        let service = Arc::clone(&service);
        let cancelled = Arc::clone(&cancelled);
        move || {
            yi_tools::KernelBridge::execute_cell(
                service.as_ref(),
                "agents = await rlm.list_agents()\nreceipt = await rlm.send('helper', 'status?')\ntry:\n    await rlm.request('helper', 'ping?', timeout=1)\nexcept RuntimeError as error:\n    asked = 'no reply to parent-' in str(error)\nprint([a['name'] for a in agents], receipt['receipts'][0]['state'], asked)",
                &cancelled,
            )
        }
    })
    .await??;
    assert!(
        roster_cell
            .result
            .stdout
            .contains("['parent', 'helper'] inboxed True"),
        "the family and the send path are reachable from the kernel: {} {}",
        roster_cell.result.stdout,
        roster_cell.result.stderr
    );

    let delete_cell = tokio::task::spawn_blocking({
        let service = Arc::clone(&service);
        let cancelled = Arc::clone(&cancelled);
        move || {
            yi_tools::KernelBridge::execute_cell(
                service.as_ref(),
                "gone = await rlm.delete_subagent('helper')\nprint(gone.rlm_child_id.startswith('sub-'), len(await rlm.list_subagents()))",
                &cancelled,
            )
        }
    })
    .await??;
    assert!(
        delete_cell.result.stdout.contains("True 0"),
        "rlm.delete_subagent must reap the child: {} {}",
        delete_cell.result.stdout,
        delete_cell.result.stderr
    );

    let refused_cell = tokio::task::spawn_blocking({
        let service = Arc::clone(&service);
        let cancelled = Arc::clone(&cancelled);
        move || {
            yi_tools::KernelBridge::execute_cell(
                service.as_ref(),
                "try:\n    await rlm.run('too deep', junk=1)\nexcept RuntimeError as e:\n    print(f'refused: {e}')",
                &cancelled,
            )
        }
    })
    .await??;
    assert!(
        refused_cell
            .result
            .stdout
            .contains("refused: Unsupported rlm.run kwargs: junk"),
        "handler errors must surface as Python RuntimeError: {}",
        refused_cell.result.stdout
    );

    let scope_cell = tokio::task::spawn_blocking({
        let service = Arc::clone(&service);
        let cancelled = Arc::clone(&cancelled);
        move || {
            yi_tools::KernelBridge::execute_cell(
                service.as_ref(),
                "board = {'grid': [1, 2, 3]}\nsecret = 'never scoped'\nscoped = await rlm.run('use the board', name='scoped', context_keys=['board'])\ntry:\n    await rlm.run('and this', name='nope', context_keys=['absent'])\nexcept KeyError as e:\n    print(f'keyerror: {e}')\nprint([s.session_name for s in await rlm.list_subagents()])",
                &cancelled,
            )
        }
    })
    .await??;
    assert!(
        scope_cell
            .result
            .stdout
            .contains("not bound in this kernel"),
        "an unbound context key raises in the cell that named it: {} {}",
        scope_cell.result.stdout,
        scope_cell.result.stderr
    );
    assert!(
        scope_cell.result.stdout.contains("['scoped']"),
        "the refused spawn never reached the host: {}",
        scope_cell.result.stdout
    );
    let brief = child_brief(&harness).await;
    assert!(
        brief.contains("board = {\"grid\": [1, 2, 3]}"),
        "the kernel serializes the named variable into the child's brief: {brief:?}"
    );
    assert!(
        !brief.contains("never scoped") && !brief.contains("secret"),
        "nothing of the parent's namespace beyond the named keys reaches the child: {brief:?}"
    );

    service.dispose().await;
    Ok(())
}

fn protocol_kwargs(name: &str, pairs: &[(&str, Value)]) -> Map<String, Value> {
    let mut kwargs = kwargs(&[("name", name)]);
    for (key, value) in pairs {
        kwargs.insert((*key).to_owned(), value.clone());
    }
    kwargs
}

async fn child_brief(harness: &Harness) -> String {
    user_texts(&first_child_messages(harness).await)
        .into_iter()
        .find(|text| text.contains("[task from parent]"))
        .unwrap_or_default()
}

/// The names in the block, in the order the child reads them.
fn scoped_names(brief: &str) -> Vec<String> {
    brief
        .lines()
        .skip_while(|line| *line != "<parent_context>")
        .skip(1)
        .take_while(|line| *line != "</parent_context>")
        .filter_map(|line| line.split_once(" = ").map(|(name, _)| name.to_owned()))
        .collect()
}

#[tokio::test]
async fn a_scoped_child_reads_the_named_keys_and_nothing_else() -> TestResult {
    let harness = harness(0, 1, "solved")?;
    let oversized = "g".repeat(5_000);
    harness
        .host
        .spawn(
            "solve the board".to_owned(),
            protocol_kwargs(
                "scoped",
                &[(
                    "context",
                    json!({"board": "3x3 of colours", "trace": oversized}),
                )],
            ),
        )
        .map_err(|error| error.to_string())?;
    let brief = child_brief(&harness).await;
    assert_eq!(
        scoped_names(&brief),
        vec!["board".to_owned(), "trace".to_owned()],
        "the child's whole view of the parent is the named keys: {brief:?}"
    );
    assert!(
        brief.contains("board = 3x3 of colours"),
        "a named key reaches the child with its value: {brief:?}"
    );
    assert!(
        brief.contains("[truncated to 4096 chars]"),
        "an oversized value is capped with a named marker, never silently cut: {brief:?}"
    );
    assert!(
        brief.starts_with("[task from parent]") && brief.ends_with("solve the board"),
        "scope rides the child's first user message ahead of its task, never a trusted block: {brief:?}"
    );

    let refused = harness.host.spawn(
        "resolve them here".to_owned(),
        protocol_kwargs("unresolved", &[("context_keys", json!(["board"]))]),
    );
    assert_eq!(
        refused.err().as_deref(),
        Some("Unsupported rlm.run kwargs: context_keys"),
        "the wire surface stays closed: the kernel resolves names, the host takes values"
    );

    let too_many: Map<String, Value> = (0..9)
        .map(|index| (format!("k{index}"), Value::String("v".to_owned())))
        .collect();
    let over_width = harness.host.spawn(
        "carry everything".to_owned(),
        protocol_kwargs("wide", &[("context", Value::Object(too_many))]),
    );
    assert!(
        over_width
            .err()
            .is_some_and(|error| error.contains("at most 8 are scoped")),
        "an unbounded brief is refused at admission"
    );
    Ok(())
}

#[tokio::test]
async fn a_checked_childs_malformed_answer_is_fatal() -> TestResult {
    let harness = harness(0, 1, "I fixed it, trust me")?;
    harness
        .host
        .spawn(
            "fix the parser".to_owned(),
            protocol_kwargs("checked", &[("check", json!("true"))]),
        )
        .map_err(|error| error.to_string())?;
    assert!(wait_for_status_named(&harness, "checked").await);
    let error = harness
        .host
        .result("checked", None)
        .err()
        .ok_or("a checked child's prose must never pass as a result")?;
    assert!(
        error.contains("owes a result object"),
        "the refusal names the contract the answer missed: {error}"
    );
    assert!(
        error.contains("I fixed it, trust me"),
        "the refusal carries the raw answer, so nothing arrives as a silent null: {error}"
    );
    Ok(())
}

#[tokio::test]
async fn a_checked_childs_result_is_held_back_while_its_check_is_red() -> TestResult {
    let harness = harness(0, 1, "{\"value\": 1, \"discoveries\": []}")?;
    harness
        .host
        .spawn(
            "land the fix".to_owned(),
            protocol_kwargs("red", &[("check", json!("exit 3"))]),
        )
        .map_err(|error| error.to_string())?;
    assert!(wait_for_status_named(&harness, "red").await);
    let error = harness
        .host
        .result("red", None)
        .err()
        .ok_or("a red node must not hand its answer on")?;
    assert!(
        error.contains("check is red") && error.contains("exited 3"),
        "the refusal carries the check's own evidence: {error}"
    );
    Ok(())
}

/// Adjudication reads the canonical plan file, never the session fact: the
/// named todo's delegation carries the runnable acceptance.
fn write_canonical_plan(cwd: &std::path::Path, todos: &[(&str, &str)]) -> TestResult {
    use yi_types::plan::doc::{
        Check, Delegation, GoalText, Plan, PlanId, PlanTier, RetryCount, SpawnSpec, Todo,
        TodoLabel, TodoState,
    };
    let store = yi_runtime::plan::store::PlanStore::open(cwd.join(".yi/plans"))?;
    let todos = todos
        .iter()
        .map(|(label, check)| {
            Ok(Todo {
                label: TodoLabel::new(*label)?,
                after: Vec::new(),
                state: TodoState::Pending,
                delegation: Some(Delegation {
                    spec: SpawnSpec {
                        role: None,
                        model: None,
                        effort: None,
                        tools: Vec::new(),
                        isolation: None,
                        budget: None,
                        wall: None,
                        parent_close: None,
                        extra: Map::new(),
                    },
                    accept: Check::Command((*check).to_owned()),
                    output: None,
                    context: Vec::new(),
                    note: None,
                    extra: Map::new(),
                }),
                subplan: None,
                retries: RetryCount::default(),
                children: Vec::new(),
                note: None,
                attempt: yi_types::plan::doc::AttemptId::FIRST,
                refusals: 0,
                contract: None,
                contract_hash: None,
                extra: Map::new(),
            })
        })
        .collect::<Result<Vec<_>, yi_types::plan::doc::DocError>>()?;
    let plan = Plan::opening(
        PlanId::new("adjudication")?,
        GoalText::new("adjudicate discoveries")?,
        PlanTier::Root,
        todos,
    );
    store.write(&plan)?;
    Ok(())
}

const TWO_DISCOVERIES: &str = r#"{"value": "patched", "discoveries": [
    {"text": "the retry loop double-counts", "violatesCheckOf": "t1", "fingerprint": "aaa"},
    {"text": "the README example is stale", "violatesCheckOf": "t2", "fingerprint": "bbb"}
]}"#;

const ONE_DISCOVERY: &str = r#"{"value": "patched", "discoveries": [
    {"text": "the retry loop double-counts", "violatesCheckOf": "t1", "fingerprint": "aaa"}
]}"#;

fn discovery_rows(count: usize) -> &'static str {
    let rows: Vec<String> = (0..count)
        .map(|index| format!(r#"{{"text": "row {index}", "fingerprint": "f{index}"}}"#))
        .collect();
    Box::leak(format!("{{\"value\": 1, \"discoveries\": [{}]}}", rows.join(", ")).into_boxed_str())
}

fn discovery_texts(harness: &Harness) -> Vec<String> {
    harness
        .inbox
        .lock()
        .map(|inbox| {
            inbox
                .iter()
                .filter(|text| text.contains("discovery"))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_discovery_the_runtime_cannot_adjudicate_holds_the_result_back() -> TestResult {
    let cwd = Scratch::new("yi-adjudication-no-plan")?;
    let harness = harness_with(HarnessOptions {
        child_errors: false,
        depth: 0,
        max_depth: 1,
        child_answer: ONE_DISCOVERY,
        tool_command: None,
        cwd: Some(cwd.to_path_buf()),
        wake_parent: None,
    })?;
    harness
        .host
        .spawn(
            "patch the retry loop".to_owned(),
            protocol_kwargs("finder", &[("check", json!("true"))]),
        )
        .map_err(|error| error.to_string())?;
    assert!(wait_for_status_named(&harness, "finder").await);
    let error =
        harness.host.result("finder", None).err().ok_or(
            "a row naming an ancestor check must not pass while nothing can adjudicate it",
        )?;
    assert!(
        error.contains("result held back") && error.contains("canonical plan cannot be read"),
        "the refusal names the adjudication failure rather than downgrading the row: {error}"
    );
    assert!(
        discovery_texts(&harness).is_empty(),
        "no row is reported as deferred when criticality could not be derived: {:?}",
        discovery_texts(&harness)
    );
    Ok(())
}

#[tokio::test]
async fn a_high_row_that_cannot_reach_the_ledger_holds_the_result_back() -> TestResult {
    let cwd = Scratch::new("yi-adjudication-no-ledger")?;
    write_canonical_plan(&cwd, &[("t1", "exit 4")])?;
    let harness = harness_with(HarnessOptions {
        child_errors: false,
        depth: 0,
        max_depth: 1,
        child_answer: ONE_DISCOVERY,
        tool_command: None,
        cwd: Some(cwd.to_path_buf()),
        wake_parent: None,
    })?;
    harness
        .host
        .spawn(
            "patch the retry loop".to_owned(),
            protocol_kwargs("finder", &[("check", json!("true"))]),
        )
        .map_err(|error| error.to_string())?;
    assert!(wait_for_status_named(&harness, "finder").await);
    let error = harness
        .host
        .result("finder", None)
        .err()
        .ok_or("a HIGH row the ledger never took must not pass as a delivered result")?;
    assert!(
        error.contains("could not be recorded") && error.contains("No goal exists"),
        "the refusal names the ledger failure, so the completion gate is never armed silently: {error}"
    );
    Ok(())
}

#[tokio::test]
async fn an_oversized_discovery_list_is_refused_before_any_check_runs() -> TestResult {
    let harness = harness(0, 1, discovery_rows(17))?;
    harness
        .host
        .spawn(
            "patch the retry loop".to_owned(),
            protocol_kwargs("flooder", &[("check", json!("true"))]),
        )
        .map_err(|error| error.to_string())?;
    assert!(wait_for_status_named(&harness, "flooder").await);
    let error = harness
        .host
        .result("flooder", None)
        .err()
        .ok_or("an unbounded discovery list must be refused")?;
    assert!(
        error.contains("reported 17 discoveries") && error.contains("at most 16"),
        "the refusal names the cap and what the child reported: {error}"
    );
    assert!(
        discovery_texts(&harness).is_empty(),
        "a refused list routes nothing: {:?}",
        discovery_texts(&harness)
    );
    Ok(())
}

#[tokio::test]
async fn criticality_is_derived_by_re_running_the_ancestors_check() -> TestResult {
    let cwd = Scratch::new("yi-adjudication-criticality")?;
    write_canonical_plan(&cwd, &[("t1", "exit 4"), ("t2", "true")])?;
    let harness = harness_with(HarnessOptions {
        child_errors: false,
        depth: 0,
        max_depth: 1,
        child_answer: TWO_DISCOVERIES,
        tool_command: None,
        cwd: Some(cwd.to_path_buf()),
        wake_parent: None,
    })?;
    yi_session::lock_session(&harness.store).set_goal(yi_types::goal::Goal {
        objective: "ship the retry fix".to_owned(),
        status: yi_types::goal::GoalStatus::Active,
        token_budget: None,
        tokens_used: 0,
        time_used_seconds: 0,
        created: 0,
        updated: 0,
        check: None,
        check_timeout_ms: None,
        check_failure: None,
        discoveries: Vec::new(),
        extra: Map::new(),
    })?;
    harness
        .host
        .spawn(
            "patch the retry loop".to_owned(),
            protocol_kwargs("finder", &[("check", json!("true"))]),
        )
        .map_err(|error| error.to_string())?;
    assert!(wait_for_status_named(&harness, "finder").await);
    let reply = harness
        .host
        .result("finder", None)
        .map_err(|error| error.to_string())?;
    assert_eq!(
        reply["value"], "patched",
        "a green protocol child still hands back its value"
    );
    assert_eq!(
        reply["discoveries"]
            .as_array()
            .map(std::vec::Vec::len)
            .unwrap_or_default(),
        2
    );

    let inbox = harness.inbox.lock().map_err(|_| "poisoned")?.clone();
    assert!(
        inbox
            .iter()
            .any(|text| text.starts_with("HIGH discovery from finder")
                && text.contains("task t1 is red")
                && text.contains("exited 4")),
        "a discovery naming a task whose check is red pauses the parent with the evidence: {inbox:?}"
    );
    assert!(
        inbox
            .iter()
            .any(|text| text.starts_with("deferred discovery from finder")
                && text.contains("README example")),
        "a discovery whose named check is green is deferred, never dropped: {inbox:?}"
    );

    let rows: Vec<(String, Value)> = harness
        .entries
        .lock()
        .map_err(|_| "poisoned")?
        .iter()
        .filter_map(|message| match message {
            AgentMessage::Custom {
                custom_type,
                details: Some(details),
                ..
            } => Some((custom_type.clone(), details.clone())),
            _ => None,
        })
        .collect();
    let criticalities: Vec<&Value> = rows
        .iter()
        .filter(|(custom_type, _)| custom_type == "discovery")
        .map(|(_, details)| &details["criticality"])
        .collect();
    assert_eq!(
        criticalities,
        vec![&json!("high"), &json!("deferred")],
        "both rows land in the session as typed discovery entries the mining board reads: {rows:?}"
    );
    assert_eq!(
        rows.first()
            .map(|(_, details)| details["discovery"]["fingerprint"].clone()),
        Some(json!("aaa")),
        "the row carries the child's fingerprint, so the board can key on it"
    );

    let ledger = yi_session::lock_session(&harness.store)
        .goal()
        .ok_or("goal")?
        .discoveries;
    assert_eq!(
        ledger
            .iter()
            .map(|row| row.fingerprint.as_str())
            .collect::<Vec<_>>(),
        vec!["aaa"],
        "only the HIGH row enters the goal ledger the completion gate drains: {ledger:?}"
    );
    Ok(())
}

struct FireAtTurnEnd {
    notice: Arc<dyn Fn(&str) + Send + Sync>,
    running: Arc<dyn Fn() -> bool + Send + Sync>,
    fired_while_running: Arc<std::sync::atomic::AtomicBool>,
    fired: bool,
}

impl yi_runtime::ext::Extension for FireAtTurnEnd {
    fn name(&self) -> &'static str {
        "fire-at-turn-end"
    }

    fn interests(&self) -> yi_runtime::ext::EventMask {
        yi_runtime::ext::EventMask::TURN_END
    }

    fn on(&mut self, _event: &yi_runtime::ext::Event, _out: &mut Vec<yi_runtime::ext::Effect>) {
        if self.fired {
            return;
        }
        self.fired = true;
        self.fired_while_running
            .store((self.running)(), Ordering::SeqCst);
        (self.notice)("[subagent helper (sub-1) finished]\nLast answer: done at the seam");
    }
}

/// Guards `wake_idle_hook` over `notice_hook` at `wiring::lifecycle_notice`: the notice
/// fires at TurnEnd while the status is still Running, where a steer queues for nobody.
#[tokio::test]
async fn notice_arriving_at_parent_turn_end_is_not_lost() -> TestResult {
    let parent = parent_session(&["first turn", "woken by the notice"]);
    let fired_while_running = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut host = yi_runtime::ext::Host::new(std::env::temp_dir());
    host.register(Box::new(FireAtTurnEnd {
        notice: yi_runtime::wiring::lifecycle_notice(&parent),
        running: parent.activity_handle(),
        fired_while_running: Arc::clone(&fired_while_running),
        fired: false,
    }));
    parent.install_extensions(host);
    parent.prompt("start the first turn")?;
    let mut messages = Vec::new();
    for _ in 0..POLL_ATTEMPTS {
        messages = parent.messages();
        if assistant_count(&messages) >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    assert!(
        fired_while_running.load(Ordering::SeqCst),
        "the interleaving under test fires while the parent is still Running"
    );
    assert_eq!(
        notice_texts(&messages).len(),
        1,
        "the notice starts the next turn exactly once: {messages:?}"
    );
    assert_eq!(assistant_count(&messages), 2, "{messages:?}");
    Ok(())
}

/// Guards `wake_idle_hook`'s follow-up queue: two children finish in milliseconds while the
/// parent's turn sleeps two seconds in a tool, so both notices ride the open turn's one drain;
/// the old `run(message)` drop lost the one that answered Busy, and a wake per notice would
/// start a fourth turn the script cannot answer.
#[tokio::test]
async fn concurrent_notices_preserve_updates_and_coalesce_wakes() -> TestResult {
    let mut sleep = Map::new();
    sleep.insert("command".to_owned(), Value::String("sleep 2".to_owned()));
    let mut parent = parent_session_scripted(
        vec![faux_assistant_message(
            vec![yi_ai::faux::faux_tool_call("hold-1", "bash", sleep)],
            StopReason::ToolUse,
        )],
        &["after the sleep", "heard them both"],
    );
    parent.use_tools(yi_tools::builtin_tools(), std::env::temp_dir(), None);
    let parent = Arc::new(parent);
    let harness = harness_with(HarnessOptions {
        child_errors: false,
        depth: 0,
        max_depth: 1,
        child_answer: "done",
        tool_command: None,
        cwd: None,
        wake_parent: Some(Arc::clone(&parent)),
    })?;
    parent.prompt("hold the turn open")?;
    for name in ["alpha", "beta"] {
        harness
            .host
            .spawn("finish now".to_owned(), kwargs(&[("name", name)]))
            .map_err(|error| error.to_string())?;
    }
    let mut messages = Vec::new();
    for _ in 0..POLL_ATTEMPTS {
        messages = parent.messages();
        let notices = notice_texts(&messages);
        let both = notices
            .iter()
            .any(|text| text.starts_with("[subagent alpha"))
            && notices
                .iter()
                .any(|text| text.starts_with("[subagent beta"));
        if both && parent.status() == yi_runtime::Status::Idle {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    let notices = notice_texts(&messages);
    assert_eq!(
        notices.len(),
        2,
        "each finish reaches the parent once: {notices:?}"
    );
    assert!(
        notices
            .iter()
            .any(|text| text.starts_with("[subagent alpha"))
            && notices
                .iter()
                .any(|text| text.starts_with("[subagent beta")),
        "{notices:?}"
    );
    assert_eq!(
        assistant_count(&messages),
        3,
        "the tool call, its reply, and one turn for both notices: {messages:?}"
    );
    Ok(())
}

/// Guards the tool slot read at run start: `attach_runtime` builds the child host's wake
/// before it installs the session's tools, and a handle that copied the table at that point
/// woke a turn in which every tool call answered "not found".
#[tokio::test]
async fn a_wake_built_before_the_tools_runs_the_woken_turn_with_them() -> TestResult {
    let root = Scratch::new("yi-wake-tools")?;
    let marker = root.join("woken");
    let mut touch = Map::new();
    touch.insert(
        "command".to_owned(),
        Value::String(format!("touch {}", marker.display())),
    );
    let mut parent = parent_session_scripted(
        vec![faux_assistant_message(
            vec![yi_ai::faux::faux_tool_call("wake-1", "bash", touch)],
            StopReason::ToolUse,
        )],
        &["the tool ran"],
    );
    let wake = yi_runtime::wiring::lifecycle_notice(&parent);
    parent.use_tools(yi_tools::builtin_tools(), std::env::temp_dir(), None);
    wake("[subagent helper (sub-1) finished]\nLast answer: done");
    let mut messages = Vec::new();
    for _ in 0..POLL_ATTEMPTS {
        messages = parent.messages();
        if assistant_count(&messages) >= 2 && parent.status() == yi_runtime::Status::Idle {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    assert_eq!(assistant_count(&messages), 2, "{messages:?}");
    assert!(
        marker.exists(),
        "the woken turn ran bash with the tools installed after the wake was built: {messages:?}"
    );
    Ok(())
}

/// Guards the cursor over the old `take_pending` drain: with the drain, the first waiter
/// zeroed the counter and the second returned empty at its deadline.
#[tokio::test]
async fn two_waiters_observe_their_own_child_completion() -> TestResult {
    let harness = harness(0, 1, "done")?;
    let reply = harness
        .host
        .spawn("finish now".to_owned(), kwargs(&[("name", "solo")]))
        .map_err(|error| error.to_string())?;
    let child_id = reply["rlm_child_id"]
        .as_str()
        .ok_or("missing child id")?
        .to_owned();
    let (first, second) = tokio::join!(
        harness.host.wait(1_000, None),
        harness.host.wait(1_000, None)
    );
    for reply in [&first, &second] {
        assert_eq!(
            reply["changed"],
            json!(["solo"]),
            "the spawn moved it: {reply:?}"
        );
    }
    assert!(
        wait_for_status(&harness.host, &child_id, "completed").await,
        "the child completes"
    );
    let (first, second) = tokio::join!(
        harness.host.wait(1_000, None),
        harness.host.wait(1_000, None)
    );
    for reply in [&first, &second] {
        assert_eq!(reply["changed"], json!(["solo"]), "{reply:?}");
        assert_eq!(reply["updated"], reply["changed"]);
        assert_eq!(reply["states"]["solo"], json!("finished"), "{reply:?}");
        assert!(reply["cursor"].as_u64().is_some_and(|cursor| cursor > 0));
    }
    let cursor = first["cursor"].as_u64().ok_or("cursor missing")?;
    let quiet = harness.host.wait(1_000, Some(cursor)).await;
    assert_eq!(quiet["changed"], json!([]), "nothing moved past the cursor");
    assert_eq!(quiet["states"]["solo"], json!("finished"));
    Ok(())
}

/// Guards the wake on a delete, the path `rlm.delete_subagent` takes: the removed record
/// carries no epoch, so a waiter that only returned on a named change slept to its deadline
/// while `states` already lacked the child.
#[tokio::test]
async fn a_delete_wakes_a_waiter_with_the_child_gone() -> TestResult {
    let harness = harness(0, 1, "done")?;
    let reply = harness
        .host
        .spawn("finish now".to_owned(), kwargs(&[("name", "gone")]))
        .map_err(|error| error.to_string())?;
    let child_id = reply["rlm_child_id"]
        .as_str()
        .ok_or("missing child id")?
        .to_owned();
    assert!(wait_for_status(&harness.host, &child_id, "completed").await);
    let settled = harness.host.wait(0, None).await;
    let cursor = settled["cursor"].as_u64();
    let host = Arc::clone(&harness.host);
    let waiter = tokio::spawn(async move { host.wait(300_000, cursor).await });
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let started = std::time::Instant::now();
    harness.host.delete("gone")?;
    let woken = waiter.await.map_err(|error| error.to_string())?;
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "a delete wakes the waiter, not the deadline"
    );
    assert_eq!(woken["changed"], json!([]), "{woken:?}");
    assert!(
        woken["states"].get("gone").is_none(),
        "the reaped child is off the state map: {woken:?}"
    );
    assert!(
        woken["cursor"].as_u64() > cursor,
        "the delete moved the cursor: {woken:?}"
    );
    Ok(())
}

/// Guards the cursor over the drain: a waiter arriving after another collected the
/// completion still sees the terminal state at once.
#[tokio::test]
async fn late_wait_observes_already_completed_child() -> TestResult {
    let harness = harness(0, 1, "done")?;
    let reply = harness
        .host
        .spawn("finish now".to_owned(), kwargs(&[("name", "early")]))
        .map_err(|error| error.to_string())?;
    let child_id = reply["rlm_child_id"]
        .as_str()
        .ok_or("missing child id")?
        .to_owned();
    assert!(wait_for_status(&harness.host, &child_id, "completed").await);
    let collected = harness.host.wait(1_000, None).await;
    assert_eq!(collected["changed"], json!(["early"]));
    let started = std::time::Instant::now();
    let late = harness.host.wait(300_000, None).await;
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "a late waiter returns at once, not at the deadline"
    );
    assert_eq!(late["changed"], json!(["early"]), "{late:?}");
    assert_eq!(late["states"]["early"], json!("finished"));
    Ok(())
}

/// One exit, by the road named: what the parent bus must carry for it.
async fn exit_updates(road: &str) -> Result<(Vec<ChildUpdate>, Vec<ChildUpdate>), Box<dyn Error>> {
    let running = matches!(road, "interrupt" | "delete while running");
    let harness = harness_with(HarnessOptions {
        child_errors: road == "error",
        depth: 0,
        max_depth: 1,
        child_answer: "the answer",
        tool_command: running.then_some("sleep 30"),
        cwd: None,
        wake_parent: None,
    })?;
    let mut events = harness.events.subscribe();
    let reply = harness
        .host
        .spawn("work".to_owned(), kwargs(&[("name", "exiting")]))
        .map_err(|error| error.to_string())?;
    let child_id = reply["rlm_child_id"].as_str().ok_or("no id")?.to_owned();
    let mut seen = Vec::new();
    if running {
        // Held inside its `sleep`: the bus says so before the exit is asked for.
        while !seen
            .last()
            .is_some_and(|update: &ChildUpdate| update.activity == ChildActivity::Executing)
        {
            let event =
                tokio::time::timeout(std::time::Duration::from_secs(20), events.recv()).await??;
            if let AgentEvent::ChildUpdate { update } = event {
                assert_eq!(update.status, ChildStatus::Running, "{road}: ended early");
                seen.push(update);
            }
        }
    } else {
        seen = collect_updates(&mut events).await;
    }
    match road {
        "interrupt" => drop(harness.host.interrupt("exiting")?),
        "delete while running" | "delete after end" => drop(harness.host.delete("exiting")?),
        "reap" => drop(harness.host.reap("exiting")?),
        _ => {}
    }
    // The bus alone, never `host.list()`: what arrives once the exit was asked for.
    let mut after = Vec::new();
    while let Ok(Ok(event)) =
        tokio::time::timeout(std::time::Duration::from_millis(700), events.recv()).await
    {
        if let AgentEvent::ChildUpdate { update } = event {
            after.push(update);
        }
    }
    seen.retain(|update| update.id.as_str() == child_id);
    after.retain(|update| update.id.as_str() == child_id);
    Ok((seen, after))
}

/// Guards the terminal update: a removal took the record without a publish, and the run it cut
/// short then found no record and skipped its own, so a client's card ran forever.
#[tokio::test]
async fn every_exit_publishes_one_terminal_update() -> TestResult {
    let roads = [
        ("complete", ChildStatus::Completed, None),
        (
            "error",
            ChildStatus::Error,
            Some("child run ended with an error"),
        ),
        ("interrupt", ChildStatus::Error, Some("interrupted")),
        (
            "delete while running",
            ChildStatus::Error,
            Some("interrupted"),
        ),
        ("delete after end", ChildStatus::Completed, None),
        ("reap", ChildStatus::Completed, None),
    ];
    for (road, status, cause) in roads {
        let (before, after) = exit_updates(road).await?;
        let asked = !matches!(road, "complete" | "error");
        let terminal: Vec<&ChildUpdate> = before
            .iter()
            .chain(&after)
            .filter(|update| update.status != ChildStatus::Running)
            .collect();
        assert!(!terminal.is_empty(), "{road}: no terminal update at all");
        for update in &terminal {
            assert_eq!(
                (update.status, update.error.as_deref()),
                (status, cause),
                "{road}: one machine-readable cause, never two stories: {terminal:?}"
            );
        }
        if asked {
            let told = after
                .iter()
                .filter(|update| update.status != ChildStatus::Running)
                .count();
            assert!(
                told >= 1,
                "{road}: the exit itself published nothing: {after:?}"
            );
            if road == "delete while running" {
                assert_eq!(
                    told, 1,
                    "the record is gone, so nothing may follow: {after:?}"
                );
            }
        }
    }
    Ok(())
}

/// Pins what a parent's end did to its children before leases (plan section 7.4): nothing. The
/// run task holds the host, so a child outlives every handle its parent dropped, runs to its
/// own end unbounded and reports to a parent that is gone. A dropped handle still does that;
/// `close` is the control, and its default is pinned below.
#[tokio::test]
async fn children_at_parent_close_today() -> TestResult {
    let (harness, feed) = busy_child().await?;
    let Harness { host, notices, .. } = harness;
    drop(host);
    assert_eq!(
        feed.status(),
        yi_runtime::Status::Running,
        "the drop stopped nothing"
    );
    feed.wait_idle().await;
    let answered = feed.messages().iter().any(|message| {
        matches!(
            message,
            AgentMessage::Assistant {
                stop_reason: StopReason::Stop,
                ..
            }
        )
    });
    assert!(answered, "the orphan ran to its own end");
    for _ in 0..POLL_ATTEMPTS {
        if !notices.lock().map_err(|_| "poisoned")?.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    let told = notices.lock().map_err(|_| "poisoned")?.clone();
    assert!(
        told.iter().any(|text| text.contains("finished")),
        "{told:?}"
    );

    // The default since leases: a close revokes each live child with the thirty second grace.
    let (harness, _feed) = busy_child().await?;
    assert_eq!(harness.host.close(), ["busy"]);
    let journaled =
        yi_session::lock_session(&harness.store).find_entries(&yi_session::EntryQuery {
            custom_type: Some("lease".to_owned()),
            ..yi_session::EntryQuery::default()
        })?;
    let text = serde_json::to_string(&journaled)?;
    assert!(
        text.contains(r#""graceMs":30000"#) && text.contains("the parent closed"),
        "{text}"
    );
    assert!(
        harness.host.close().is_empty(),
        "a revoked child is not revoked twice"
    );
    Ok(())
}

/// Dies with the stop road: a client that aborts the child's session itself leaves the host's
/// record to guess, and the feed a client holds has no abort to call. The TUI sends the id.
#[tokio::test]
async fn a_tui_stop_is_a_host_interrupt() -> TestResult {
    let (harness, _feed) = busy_child().await?;
    let mut events = harness.events.subscribe();
    let id = harness.host.children_view()[0].update.id.clone();
    harness.host.interrupt(id.as_str())?;
    let ended = loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(20), events.recv()).await;
        if let AgentEvent::ChildUpdate { update } = event??
            && update.status != ChildStatus::Running
        {
            break update;
        }
    };
    assert_eq!(ended.exit, Some(yi_types::subagent::ChildExit::Interrupted));
    assert_eq!(
        (ended.status, ended.error.as_deref()),
        (ChildStatus::Error, Some("interrupted"))
    );
    assert_eq!(harness.host.status()["members"][0]["state"], "failed");
    for _ in 0..POLL_ATTEMPTS {
        if !harness.notices.lock().map_err(|_| "poisoned")?.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    let notices = harness.notices.lock().map_err(|_| "poisoned")?.clone();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].ends_with("interrupted]"), "{notices:?}");
    Ok(())
}

/// Guards the abort-before-first-poll race: admission read the interrupt's epoch after the
/// delete had moved it, cleared the abort, and a child with no record ran to the end unseen.
#[tokio::test]
async fn a_child_deleted_before_its_first_poll_never_runs() -> TestResult {
    let marker = Scratch::new("yi-zombie")?;
    let ran = marker.join("ran");
    let command: &'static str = Box::leak(format!("touch {}", ran.display()).into_boxed_str());
    let harness = harness_with(HarnessOptions {
        child_errors: false,
        depth: 0,
        max_depth: 1,
        child_answer: "done",
        tool_command: Some(command),
        cwd: None,
        wake_parent: None,
    })?;
    // No await between the two: the child's run task has not been polled once.
    harness
        .host
        .spawn("work".to_owned(), kwargs(&[("name", "zombie")]))
        .map_err(|error| error.to_string())?;
    harness.host.delete("zombie")?;
    tokio::time::sleep(std::time::Duration::from_millis(1_500)).await;
    assert!(!ran.exists(), "the deleted child ran its tool call anyway");
    assert_eq!(harness.attributed.load(Ordering::SeqCst), 0, "it billed");
    let notices = harness.notices.lock().map_err(|e| e.to_string())?.clone();
    assert!(
        notices.is_empty(),
        "a retired child tells nothing: {notices:?}"
    );
    Ok(())
}

/// Guards the kernel delete journey on a real kernel: `delete_subagent` refused the handle
/// `rlm.run` had just returned, and a removal it did make published nothing to any client.
#[tokio::test]
async fn a_kernel_cell_that_spawns_and_deletes_tells_why() -> TestResult {
    let harness = harness_with(HarnessOptions {
        child_errors: false,
        depth: 0,
        max_depth: 1,
        child_answer: "never reached",
        tool_command: Some("sleep 30"),
        cwd: None,
        wake_parent: None,
    })?;
    let mut events = harness.events.subscribe();
    let mut registry = HostRegistry::default();
    registry.register_mcp_stubs();
    harness.host.register(&mut registry);
    let service = Arc::new(KernelService::new(KernelServiceOptions {
        cwd: std::env::temp_dir(),
        home: std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default(),
        session_dir: Some(harness.root.to_path_buf()),
        family_dir: None,
        host: Arc::new(registry),
        on_restore: None,
        sandbox: None,
        snapshot_key: None,
        cell_ceiling: None,
    }));
    let cancelled: yi_tools::CancelFlag = Arc::new(|| false);
    let cell = tokio::task::spawn_blocking({
        let service = Arc::clone(&service);
        move || {
            yi_tools::KernelBridge::execute_cell(
                service.as_ref(),
                "h = await rlm.run('hold on', name='doomed')\nd = await rlm.delete_subagent(h)\nprint(d.session_name, d.status)",
                &cancelled,
            )
        }
    })
    .await??;
    assert!(
        cell.result.stdout.contains("doomed error"),
        "the delete takes the handle and says the run did not finish: {} {}",
        cell.result.stdout,
        cell.result.stderr
    );
    let mut last = None;
    while let Ok(Ok(event)) =
        tokio::time::timeout(std::time::Duration::from_millis(700), events.recv()).await
    {
        if let AgentEvent::ChildUpdate { update } = event {
            last = Some(update);
        }
    }
    let last = last.ok_or("the bus carried no update for the child")?;
    assert_eq!(
        (last.status, last.error.as_deref()),
        (ChildStatus::Error, Some("interrupted")),
        "the last word on the bus is the exit, with its cause: {last:?}"
    );
    assert!(harness.host.children_view().is_empty());
    Ok(())
}

type Ticks = Arc<std::sync::atomic::AtomicU64>;

/// A family whose one child holds its turn open on `hold`, with the lease clock in hand.
async fn leased(
    label: &str,
    hold: &'static str,
    cwd: Option<PathBuf>,
) -> Result<(Scratch, support::Family, Ticks), Box<dyn Error>> {
    let root = Scratch::new(label)?;
    let store = support::memory_store(label);
    let cwd = cwd.unwrap_or_else(std::env::temp_dir);
    let family = support::family(root.to_path_buf(), cwd, store, Some(hold));
    let ticks: Ticks = Arc::new(std::sync::atomic::AtomicU64::new(1_000_000));
    let clock = Arc::clone(&ticks);
    let now = Arc::new(move || clock.load(Ordering::SeqCst));
    family.host.set_lease_clock(Some(now), None);
    Ok((root, family, ticks))
}

async fn executing(family: &support::Family) -> Result<(), Box<dyn Error>> {
    for _ in 0..POLL_ATTEMPTS {
        let views = family.host.children_view();
        if views
            .first()
            .is_some_and(|view| view.update.activity == ChildActivity::Executing)
        {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    Err("the child never reached its tool".into())
}

fn terminal(events: &mut tokio::sync::broadcast::Receiver<AgentEvent>) -> Vec<ChildUpdate> {
    let mut ended = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::ChildUpdate { update } = event
            && update.status != ChildStatus::Running
        {
            ended.push(update);
        }
    }
    ended
}

/// Dies with the order in `revoke` and `retire_as`: send the cancel before the journal write,
/// or publish before the repossession record commits, and a crash between the two leaves a
/// child told to stop, or reported gone, with nothing on record saying why.
#[tokio::test]
async fn revoke_delivers_cancel_then_repossesses_after_grace_with_the_record_first() -> TestResult {
    use yi_types::lease::{Disposition, LeaseRecord};
    let (_root, family, ticks) = leased("yi-lease-revoke", "sleep 30", None).await?;
    let graces: Arc<Mutex<Vec<u64>>> = Arc::default();
    let seen = Arc::clone(&graces);
    let timer = Arc::new(move |grace: std::time::Duration| {
        if let Ok(mut seen) = seen.lock() {
            seen.push(u64::try_from(grace.as_millis()).unwrap_or(0));
        }
    });
    family.host.set_lease_clock(None, Some(timer));
    family
        .host
        .spawn("hold".to_owned(), kwargs(&[("name", "held")]))?;
    executing(&family).await?;
    let mut events = family.events.subscribe();

    let reply = family.host.revoke("held", 30_000, "scope changed")?;
    assert_eq!(
        state_of(&reply),
        "queued",
        "the cancel reached a live turn: {reply:?}"
    );
    assert_eq!(graces.lock().map_err(|_| "poisoned")?[..], [30_000]);
    let journal = family.journal();
    let [LeaseRecord::Revoked(lease)] = &journal[..] else {
        return Err(format!("the revocation is journaled first: {journal:?}").into());
    };
    assert_eq!(
        lease.revoked.as_ref().map(|revoked| revoked.at),
        Some(1_000_000)
    );
    let inbox = family.host.transcript("held").ok_or("no transcript")?;
    let cancel = serde_json::to_string(&yi_runtime::family::recent_entries(&inbox))?;
    assert!(cancel.contains(r#""kind":"cancel""#), "{cancel}");
    let sent = family.host.route("parent", "held", "more work", true)?;
    assert_eq!(
        state_of(&sent),
        "inboxed",
        "a revoked child is admitted no new work"
    );

    // Inside the grace nothing is taken, however often the timer's job runs.
    ticks.store(1_029_999, Ordering::SeqCst);
    assert!(family.host.expire().await.is_empty());
    assert_eq!(family.state_of("held").as_deref(), Some("running"));

    // The grace is over, and the record cannot be written: nothing is released or published.
    ticks.store(1_030_000, Ordering::SeqCst);
    family.unplugged.store(true, Ordering::SeqCst);
    assert!(family.host.expire().await.is_empty());
    assert!(
        terminal(&mut events).is_empty(),
        "no record, so no clean report"
    );
    assert_eq!(
        family.state_of("held").as_deref(),
        Some("repossession_pending")
    );

    family.unplugged.store(false, Ordering::SeqCst);
    let asked = std::time::Instant::now();
    assert_eq!(family.host.expire().await, ["held"]);
    assert!(
        asked.elapsed() < std::time::Duration::from_secs(5),
        "the latency is observed"
    );
    let journal = family.journal();
    let Some(LeaseRecord::Repossessed(record)) = journal.get(1) else {
        return Err(format!("no repossession record: {journal:?}").into());
    };
    assert_eq!(record.disposition, Disposition::Settled);
    assert_eq!(
        record
            .kept
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["history://held"]
    );
    let ended = terminal(&mut events);
    assert_eq!(
        ended.len(),
        1,
        "one terminal update, after the record: {ended:?}"
    );
    assert_eq!(
        ended[0].exit,
        Some(yi_types::subagent::ChildExit::Repossessed)
    );
    assert_eq!(ended[0].error.as_deref(), Some("repossessed"));
    assert!(!family.host.holds("held"), "the record is released last");
    assert!(
        family.host.kept_transcript("held").is_some(),
        "its history stays readable"
    );
    Ok(())
}

/// Dies with `resume_revocations`: without it a host that restarts inside a grace forgets the
/// revocation, and the journal says a child was told to stop and never says what became of it.
#[tokio::test]
async fn restart_during_grace_completes_the_repossession() -> TestResult {
    use yi_types::lease::LeaseRecord;
    let (root, family, _ticks) = leased("yi-lease-restart", "sleep 30", None).await?;
    family
        .host
        .spawn("hold".to_owned(), kwargs(&[("name", "held")]))?;
    executing(&family).await?;
    family.host.revoke("held", 30_000, "scope changed")?;
    family.host.interrupt("held")?;
    let store = family.store.clone();
    drop(family);

    let next = support::family(root.to_path_buf(), std::env::temp_dir(), store, None);
    assert!(
        next.host.expire().await.is_empty(),
        "nothing live is left to stop"
    );
    let journal = next.journal();
    assert!(
        matches!(&journal[..], [LeaseRecord::Revoked(_), LeaseRecord::Repossessed(done)] if done.lease.holder == "held"),
        "the first expiry after a restart completes the record: {journal:?}"
    );
    let told = next.notices.lock().map_err(|_| "poisoned")?.clone();
    assert!(
        told.iter().any(|text| text.contains("held repossessed")),
        "{told:?}"
    );
    assert_eq!(
        next.host.resume_revocations()?,
        Vec::<String>::new(),
        "completed once"
    );
    Ok(())
}

/// Dies with the restore in `retire_as` and `leave_pending`: report the child gone when its
/// lane would not settle and the only copy of its work is a checkout nothing points at.
#[tokio::test]
async fn a_failed_settle_reports_repossession_pending_with_references_kept() -> TestResult {
    use yi_types::lease::LeaseRecord;
    let repo = git_repo("lease-pending")?;
    let hold = "echo kept > work.txt; sleep 30";
    let (_root, family, _ticks) =
        leased("yi-lease-pending", hold, Some(repo.to_path_buf())).await?;
    let asked = kwargs(&[("name", "walled"), ("isolation", "worktree")]);
    family.host.spawn("hold".to_owned(), asked)?;
    executing(&family).await?;
    let mut events = family.events.subscribe();
    family.host.revoke("walled", 0, "scope changed")?;

    // A settle past the run's own end is refused, and the lane stays where it is.
    family.host.set_deadline(Some(std::time::Instant::now()));
    assert!(family.host.expire().await.is_empty());
    assert!(
        terminal(&mut events).is_empty(),
        "never a clean report: nothing is published"
    );
    assert_eq!(
        family.state_of("walled").as_deref(),
        Some("repossession_pending")
    );
    let tree = family
        .host
        .cwd_of("walled")
        .ok_or("the record lost its worktree")?;
    assert_eq!(std::fs::read_to_string(tree.join("work.txt"))?, "kept\n");
    assert_eq!(
        family.journal().len(),
        1,
        "only the revocation is on record"
    );

    // The cause removed, the next run of the timer's job finishes it.
    family.host.set_deadline(None);
    assert_eq!(family.host.expire().await, ["walled"]);
    let journal = family.journal();
    let Some(LeaseRecord::Repossessed(record)) = journal.last() else {
        return Err(format!("no repossession record: {journal:?}").into());
    };
    let kept: Vec<String> = record.kept.iter().map(ToString::to_string).collect();
    assert!(
        kept.iter().any(|url| url.starts_with("branch://")),
        "{kept:?}"
    );
    assert_eq!(terminal(&mut events).len(), 1);
    Ok(())
}

/// Dies with the cancel flag (`Shared::winding_down`) and the `cancelled` arm of `exit_of`:
/// without them a cancel is one more message, the run goes on to its answer, and reads finished.
#[tokio::test]
async fn a_cancel_ends_the_run_at_its_next_message_boundary() -> TestResult {
    let (_root, family, ticks) = leased("yi-lease-cancel", "sleep 1", None).await?;
    family
        .host
        .spawn("hold".to_owned(), kwargs(&[("name", "polite")]))?;
    executing(&family).await?;
    family.host.revoke("polite", 60_000, "scope changed")?;
    assert!(
        family.reaches("polite", "failed").await,
        "{:?}",
        family.state_of("polite")
    );
    let view = family
        .host
        .children_view()
        .pop()
        .ok_or("the record is kept")?;
    assert_eq!(
        view.update.exit,
        Some(yi_types::subagent::ChildExit::Interrupted)
    );
    assert_eq!(view.update.error.as_deref(), Some("cancelled"));
    let answered = view.session.messages().iter().any(|message| {
        matches!(
            message,
            AgentMessage::Assistant {
                stop_reason: StopReason::Stop,
                ..
            }
        )
    });
    assert!(!answered, "no request followed the cancelled turn");
    ticks.store(2_000_000, Ordering::SeqCst);
    assert!(
        family.host.expire().await.is_empty(),
        "it stopped inside its grace"
    );
    assert!(
        family.host.holds("polite"),
        "so its record is the parent's to reap"
    );
    Ok(())
}

/// Dies with `Desk::drop_respondent`: without it a request to a child that was then removed
/// waits out its whole timeout and blames the clock.
#[tokio::test]
async fn a_terminated_respondent_refuses_its_waiters_by_name() -> TestResult {
    let (harness, _feed) = busy_child().await?;
    let host = Arc::clone(&harness.host);
    let asking =
        tokio::spawn(async move { host.request("parent", "busy", "which suite?", 60_000).await });
    for _ in 0..POLL_ATTEMPTS {
        if !inbox_of(&harness, "busy")?.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    harness.host.delete("busy")?;
    let waited = tokio::time::timeout(std::time::Duration::from_secs(5), asking).await;
    let refused = waited??.err().ok_or("a removed child answered")?;
    assert!(refused.contains("\"busy\" was reaped"), "{refused}");
    Ok(())
}

/// Dies with the `Kind::Failure` arm of `deliver_to_parent`: without it a child that said it
/// failed reads `finished` the moment its turn ends.
#[tokio::test]
async fn a_failure_from_a_child_reads_failed_in_wait() -> TestResult {
    let harness = harness(0, 1, "done")?;
    finished(&harness, "sorry").await?;
    let mut payload = Map::new();
    payload.insert("target".to_owned(), json!("parent"));
    payload.insert("message".to_owned(), json!("the fixture is missing"));
    payload.insert("kind".to_owned(), json!("failure"));
    harness.host.send("sorry", &payload)?;
    let waited = harness.host.wait(1_000, None).await;
    assert_eq!(waited["states"]["sorry"], json!("failed"), "{waited:?}");
    assert_eq!(waited["notes"]["sorry"], json!("the fixture is missing"));
    Ok(())
}
