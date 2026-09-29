use crate::own::own;
use crate::scratch;
use crate::support;
use scratch::Scratch;

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
    /// One per build, oldest first: 1 ends that child's first reply on a provider error, 2 its
    /// second; a build that finds none answers.
    faults: Arc<Mutex<std::collections::VecDeque<u8>>>,
    /// Run once, inside the next build: the window a respawn holds no lock in.
    during_build: BuildHook,
    /// Each child's `rlm.receive`, by name, as its kernel would reach it.
    receivers: Arc<Mutex<std::collections::HashMap<String, HostRegistry>>>,
}

type BuildHook = Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>>;

struct HarnessOptions {
    /// The child's one reply ends on a provider error instead of an answer.
    child_errors: bool,
    depth: u8,
    max_depth: u8,
    child_answer: &'static str,
    /// Some(command) makes the child call `bash` before answering, which is
    /// what moves the status tool counter and activity.
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
    let provider = Arc::new(ProviderStream::new(None));
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
    let faults: Arc<Mutex<std::collections::VecDeque<u8>>> = Arc::default();
    let fault_source = Arc::clone(&faults);
    let during_build: BuildHook = Arc::default();
    let hook_source = Arc::clone(&during_build);
    let receivers: Arc<Mutex<std::collections::HashMap<String, HostRegistry>>> = Arc::default();
    let receiver_sink = Arc::clone(&receivers);
    let host = Arc::new(SubagentHost::new(SubagentHostOptions {
        provider: Arc::new(ProviderStream::new(None)),
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
            let hook = hook_source.lock().ok().and_then(|mut slot| slot.take());
            if let Some(hook) = hook {
                hook();
            }
            let provider = Arc::new(ProviderStream::new(None));
            let mut script = Vec::new();
            if let Some(command) = tool_command {
                let mut args = serde_json::Map::new();
                args.insert("command".to_owned(), Value::String(command.to_owned()));
                script.push(faux_assistant_message(
                    vec![yi_ai::faux::faux_tool_call("call-1", "bash", args)],
                    StopReason::ToolUse,
                ));
            }
            let fault = fault_source
                .lock()
                .ok()
                .and_then(|mut faults| faults.pop_front());
            let error = faux_assistant_message(vec![faux_text(child_answer)], StopReason::Error);
            if child_errors || fault == Some(1) {
                script.push(error.clone());
            }
            script.push(child_reply(child_answer));
            if fault == Some(2) {
                script.push(error);
            }
            // A second scripted reply so a mailbox followup has a turn to run.
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
            if let (Some(host), Ok(mut sink)) = (build.link.host.upgrade(), receiver_sink.lock()) {
                let mut registry = HostRegistry::default();
                yi_runtime::mailbox::register_receive(&child, &host, &mut registry);
                sink.insert(build.link.child_name.clone(), registry);
            }
            Ok(child)
        }),
        notice: match wake_parent {
            Some(parent) => yi_runtime::wiring::lifecycle_notice(&parent),
            None => Arc::new(move |text: &str, _| {
                if let Ok(mut sink) = notice_sink.lock() {
                    sink.push(text.to_owned());
                }
            }),
        },
        events: events.clone(),
        report: Arc::new(move |message, _| {
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
        faults,
        during_build,
        receivers,
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

    harness.host.wait(0, None).await?;
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
        entries.len() + 1,
        "progress is reported to the parent"
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

/// Where `needle` first appears in what a session was handed, and where its last answer is.
fn order_of(messages: &[AgentMessage], needle: &str) -> (Option<usize>, Option<usize>) {
    let at = messages.iter().position(|message| {
        inbound_texts(std::slice::from_ref(message))
            .iter()
            .any(|text| text.contains(needle))
    });
    let answer = messages
        .iter()
        .rposition(|message| matches!(message, AgentMessage::Assistant { .. }));
    (at, answer)
}

/// Incident: `mbx-steer` sent BANANA plain, then CHERRY with `followup=True`, to a busy child;
/// CHERRY rode the next boundary and BANANA came after the child's answer, so it answered
/// twice. Dies with a plain send on the turn-end queue.
#[tokio::test]
async fn sends_to_a_busy_child_reach_its_next_boundary_in_send_order() -> TestResult {
    let (harness, busy) = busy_child().await?;
    for (text, followup) in [("BANANA", false), ("CHERRY", true)] {
        let sent = harness.host.route("parent", "busy", text, followup)?;
        assert_eq!(state_of(&sent), "queued", "{text}");
    }
    busy.wait_idle().await;
    let messages = busy.messages();
    let (banana, answer) = order_of(&messages, "BANANA");
    let (cherry, _) = order_of(&messages, "CHERRY");
    assert!(
        banana < cherry && cherry < answer && banana.is_some(),
        "both before the one answer, in send order: {messages:?}"
    );
    assert_eq!(
        assistant_count(&messages),
        2,
        "the tool call and one answer"
    );
    Ok(())
}

/// Incident: `mbx-service` sent "10" plain, then requested "0", and the service answered 12
/// instead of 22: the request started its turn and "10" came after the reply.
#[tokio::test]
async fn a_woken_turn_presents_what_waited_before_what_woke_it() -> TestResult {
    let harness = harness(0, 1, "ok")?;
    finished(&harness, "tally").await?;
    let plain = harness.host.route("parent", "tally", "10", false)?;
    assert_eq!(state_of(&plain), "inboxed");
    let woke = harness.host.route("parent", "tally", "0", true)?;
    assert_eq!(state_of(&woke), "woken");
    assert!(child_sees(&harness, "tally", ">\n0\n<").await);
    let bodies: Vec<String> = presented(&harness, "tally")
        .into_iter()
        .map(|(_, _, body)| body)
        .collect();
    assert_eq!(bodies, ["10", "0"], "send order, whatever woke the turn");
    Ok(())
}

/// A tool whose sixth call hands its session a waking message, as child mail lands mid-turn.
struct Poke {
    calls: AtomicU32,
    deliver: Arc<dyn Fn(AgentMessage, yi_types::schedule::DeliveryMode) + Send + Sync>,
}

impl yi_loop::AgentTool for Poke {
    fn definition(&self) -> yi_types::model::ToolDef {
        yi_types::model::ToolDef {
            name: "poke".to_owned(),
            description: "pokes".to_owned(),
            parameters: json!({"type": "object"}),
            freeform: None,
        }
    }

    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        _args: Map<String, Value>,
        _signal: &'a yi_loop::interrupt::InterruptSignal,
    ) -> yi_loop::tool::ToolFuture<'a> {
        Box::pin(async move {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 5 {
                let mail = AgentMessage::Custom {
                    custom_type: "agent_message".to_owned(),
                    content: yi_types::message::UserContent::Text("mail from kid".to_owned()),
                    display: true,
                    details: None,
                    timestamp: 0,
                };
                (self.deliver)(mail, yi_types::schedule::DeliveryMode::Steer);
            }
            yi_loop::ToolOutcome {
                result: yi_loop::tool::error_tool_result("poked"),
                is_error: false,
            }
        })
    }
}

/// Dies with the run's early stop going idle unchecked: mail queued in the turn the repeat
/// breaker ended was never presented, and no turn was started for it.
#[tokio::test]
async fn mail_queued_as_a_run_stops_early_starts_the_next_turn() -> TestResult {
    let call = faux_assistant_message(
        vec![yi_ai::faux::faux_tool_call("p", "poke", Map::new())],
        StopReason::ToolUse,
    );
    let mut session = parent_session_scripted(vec![call; 6], &["heard the kid"]);
    let poke = Poke {
        calls: AtomicU32::new(0),
        deliver: session.heartbeat_hook(),
    };
    session.set_tools(vec![Arc::new(poke)]);
    session.prompt("poke until told")?;
    let mut messages = Vec::new();
    for _ in 0..POLL_ATTEMPTS {
        messages = session.messages();
        if assistant_count(&messages) == 7 && session.status() == yi_runtime::Status::Idle {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    let (mail, answer) = order_of(&messages, "mail from kid");
    assert!(
        mail.is_some() && mail < answer && assistant_count(&messages) == 7,
        "the mail starts a turn of its own: {messages:?}"
    );
    Ok(())
}

/// Dies with no redelivery at attach: an envelope inboxed before a crash, never presented,
/// is lost to every later turn; one presented before it is shown once, never twice.
#[tokio::test]
async fn an_envelope_a_crash_left_unpresented_is_presented_once_after_it() -> TestResult {
    let store = support::memory_store("restart");
    let envelope = json!({"id": "kid-1", "from": "kid", "to": "parent", "kind": "inform",
        "conversation": "kid-1", "seq": 1, "sentAt": 1, "body": "the tests are green"});
    yi_session::lock_session(&store).append_custom("main", "agent_message", Some(envelope))?;
    for (restart, reply) in [(1, "heard it"), (2, "nothing new")] {
        let parent = parent_session(&[reply]);
        parent.attach_store(store.clone())?;
        parent.prompt("carry on")?;
        parent.wait_idle().await;
        let shown = parent
            .messages()
            .iter()
            .filter(|message| {
                inbound_texts(std::slice::from_ref(message))
                    .iter()
                    .any(|text| text.contains("the tests are green"))
            })
            .count();
        assert_eq!(shown, 1, "restart {restart}: {:?}", parent.messages());
    }
    Ok(())
}

/// Incident: in `mbx-ask` the parent's followup woke the finished child, whose status read
/// `finished` all through its second turn; `rlm.wait(60)` slept 60 s though it answered in 3.
/// Dies with a woken run left outside the child's lifecycle.
#[tokio::test]
async fn a_woken_childs_turn_is_a_run_the_parent_can_wait_on() -> TestResult {
    let harness = harness(0, 1, "the file is greeting.txt")?;
    finished(&harness, "asker").await?;
    let cursor = harness.host.wait(1_000, Some(0)).await?["cursor"]
        .as_u64()
        .ok_or("cursor")?;
    let sent = harness
        .host
        .route("parent", "asker", "use greeting.txt", true)?;
    assert_eq!(state_of(&sent), "woken");
    let members = harness.host.status()["members"].clone();
    assert_eq!(members[0]["state"], "running", "{members}");
    let started = harness.host.wait(1_000, Some(cursor)).await?;
    let cursor = started["cursor"].as_u64().ok_or("cursor")?;
    let wait = harness.host.wait(60_000, Some(cursor));
    let ended = json!(tokio::time::timeout(std::time::Duration::from_secs(10), wait).await??);
    assert_eq!(ended["states"]["asker"], "finished", "{ended}");
    assert_eq!(ended["causes"]["asker"], "finished", "{ended}");
    for _ in 0..POLL_ATTEMPTS {
        if harness.notices.lock().map_err(|_| "poisoned")?.len() == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    let notices = harness.notices.lock().map_err(|_| "poisoned")?.clone();
    assert_eq!(notices.len(), 2, "one notice per answer: {notices:?}");
    assert_eq!(
        harness.attributed.load(Ordering::SeqCst),
        2,
        "each run bills its own reply once"
    );
    Ok(())
}

/// Dies with the 100 ms poll back in the family wait: a child's end was seen a poll late, so
/// five spawn-and-wait rounds took half a second or more.
#[tokio::test]
async fn a_family_wait_wakes_on_the_move_not_on_a_poll() -> TestResult {
    let harness = harness(0, 1, "done")?;
    let started = std::time::Instant::now();
    for round in 0..5 {
        let name = format!("c{round}");
        harness
            .host
            .spawn(format!("work {round}"), kwargs(&[("name", &name)]))?;
        let mut cursor = 0;
        loop {
            let reply = harness.host.wait(5_000, Some(cursor)).await?;
            if reply["state"] == "settled" {
                break;
            }
            cursor = reply["cursor"].as_u64().ok_or("cursor")?;
        }
    }
    let took = started.elapsed();
    assert!(
        took < std::time::Duration::from_millis(250),
        "five rounds took {took:?}"
    );
    Ok(())
}

/// Dies with the 100 ms poll back in `rlm.receive`: mail sent just after the wait began sat a
/// poll in the queue, so five sends took half a second or more.
#[tokio::test]
async fn a_receive_wakes_on_the_mail_not_on_a_poll() -> TestResult {
    use yi_kernel::client::HostHandlers as _;
    let harness = harness(0, 1, "done")?;
    finished(&harness, "beta").await?;
    let beta = harness
        .receivers
        .lock()
        .map_err(|_| "poisoned")?
        .remove("beta");
    let beta = beta.ok_or("beta was built without its receive")?;
    let payload = json!({"timeout_ms": 5_000});
    let payload = payload.as_object().cloned().ok_or("payload")?;
    let started = std::time::Instant::now();
    for round in 0..5 {
        let waiting = beta.dispatch("rlm.receive", payload.clone());
        let waiting = tokio::spawn(waiting.ok_or("rlm.receive is not registered")?);
        tokio::task::yield_now().await;
        harness
            .host
            .route("parent", "beta", &format!("note {round}"), false)?;
        let got = waiting.await??;
        assert_eq!(
            got["envelopes"].as_array().map(Vec::len),
            Some(1),
            "{got:?}"
        );
    }
    let took = started.elapsed();
    assert!(
        took < std::time::Duration::from_millis(250),
        "five receives took {took:?}"
    );
    Ok(())
}

/// A family whose one child asks its parent a question with `ask_user`, then answers; the
/// parent's mail rides its own queue, as the root's does.
type AskingFamily = (Scratch, Arc<SubagentHost>, yi_session::SharedSession);

fn asking_family(parent: &Arc<AgentSession>) -> std::io::Result<AskingFamily> {
    let root = Scratch::new("yi-ask-parent")?;
    let store = support::memory_store("ask-parent");
    let kept = store.clone();
    let host = Arc::new(SubagentHost::new(SubagentHostOptions {
        provider: Arc::new(ProviderStream::new(None)),
        depth: 0,
        max_depth: 1,
        max_children: 8,
        parent_session_dir: root.to_path_buf(),
        cwd: root.to_path_buf(),
        home: root.join("home"),
        lane_slots: 1,
        defaults: Arc::new(|| (faux_model(), yi_types::model::Effort::Medium)),
        factory: Arc::new(|build: yi_runtime::ChildBuild<'_>| {
            let question = json!({"question": "Which file name?", "options": ["notes.md", "hello.txt"], "default": "hello.txt"});
            let question = question.as_object().cloned().unwrap_or_default();
            let call = yi_ai::faux::faux_tool_call("ask-1", "ask_user", question);
            let provider = Arc::new(ProviderStream::new(None));
            provider.queue_faux(vec![
                faux_assistant_message(vec![call], StopReason::ToolUse),
                child_reply("wrote the file the parent named"),
            ]);
            let config = SessionConfig {
                system_prompt: "child sys".to_owned(),
                model: build.model,
                thinking_level: build.thinking,
                tool_execution: ExecutionMode::Sequential,
            };
            let mut child = AgentSession::new(config, provider);
            let ask = yi_runtime::auto_review::AskUserTool::new(None).asking(Some(build.link));
            child.use_tools(vec![Arc::new(ask)], std::env::temp_dir(), None);
            Ok(child)
        }),
        notice: yi_runtime::wiring::lifecycle_notice(parent),
        events: parent.events_sender(),
        parent_messages: Arc::new(Vec::new),
        report: parent.deliver_hook(),
        attribute: Arc::new(|_usage| {}),
        store: Arc::new(move || Some(kept.clone())),
        plans_dir: root.join(".yi/plans"),
        family_live: Arc::new(|| 0),
    }));
    Ok((root, host, store))
}

/// Dies with the ask ending the child's turn: in `mbx-ask` the question met the empty-stop
/// intercept, the child answered itself on the default, and the parent never saw a question.
#[tokio::test]
async fn a_childs_question_is_a_request_its_parent_answers_with_one_call() -> TestResult {
    let parent = parent_session(&["writer asks for a file name", "answered"]);
    let (_root, host, _store) = asking_family(&parent)?;
    host.spawn(
        "write a greeting file".to_owned(),
        kwargs(&[("name", "writer")]),
    )?;
    let mut asked = None;
    for _ in 0..POLL_ATTEMPTS {
        asked = parent
            .messages()
            .into_iter()
            .find_map(|message| match message {
                AgentMessage::Custom {
                    details: Some(mail),
                    ..
                } if mail["kind"] == "request" => Some(mail),
                _ => None,
            });
        if asked.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    let asked = asked.ok_or("the parent was never woken with the question")?;
    let body = asked["body"].as_str().unwrap_or_default();
    assert!(
        body.contains("Which file name?") && body.contains("notes.md"),
        "{asked}"
    );
    let id = asked["id"].as_str().ok_or("no request id")?;
    let members = host.status()["members"].clone();
    assert_eq!(members[0]["state"], "needs_you", "{members}");
    assert!(
        members[0]["note"]
            .as_str()
            .is_some_and(|note| note.contains(id)),
        "{members}"
    );
    let answer = json!({"target": "writer", "message": "notes.md", "reply_to": id});
    host.send("parent", answer.as_object().ok_or("answer")?)?;
    for _ in 0..POLL_ATTEMPTS {
        if host.status()["members"][0]["state"] == "finished" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    let child = host.children_view().pop().ok_or("no child")?;
    let transcript = serde_json::to_string(&child.session.messages())?;
    assert!(
        transcript.contains("Your parent answered: notes.md"),
        "{transcript}"
    );
    assert!(!transcript.contains("produced no output"), "{transcript}");
    assert!(
        !presented_mail(&child.session.messages()).contains("notes.md"),
        "the reply the ask returned is never presented a second time"
    );
    assert_eq!(host.status()["members"][0]["state"], "finished");
    Ok(())
}

fn presented_mail(messages: &[AgentMessage]) -> String {
    messages
        .iter()
        .filter(|message| {
            matches!(message, AgentMessage::Custom { custom_type, .. } if custom_type == "agent_message")
        })
        .map(|message| serde_json::to_string(message).unwrap_or_default())
        .collect()
}

/// The id of the question `writer` is blocked on, once its parent can see it.
async fn asked_id(host: &SubagentHost) -> Result<String, Box<dyn Error>> {
    for _ in 0..POLL_ATTEMPTS {
        let note = host.status()["members"][0]["note"].clone();
        if let Some(id) = note
            .as_str()
            .and_then(|note| note.strip_prefix("asks "))
            .and_then(|rest| rest.split(':').next())
        {
            return Ok(id.to_owned());
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    Err("writer never asked".into())
}

/// Dies with no human road to a child's question, and with both answers landing: the parent's
/// second reply to an answered request was presented to the child as fresh mail. Dies too with
/// the human's answer unrecorded: the envelope, the parent's journal and its model never said so.
#[tokio::test]
async fn the_humans_answer_resolves_a_question_and_the_parents_is_refused() -> TestResult {
    let parent = parent_session(&["writer asks for a file name", "answered"]);
    let (_root, host, store) = asking_family(&parent)?;
    host.spawn(
        "write a greeting file".to_owned(),
        kwargs(&[("name", "writer")]),
    )?;
    let id = asked_id(&host).await?;
    let flag = host.children_view().pop().ok_or("no child")?.update.flag;
    assert!(
        matches!(&flag, Some(yi_types::subagent::ChildFlag::NeedsYou { note }) if note.contains("Which file name?")),
        "the card's update carries the question: {flag:?}"
    );
    let sent = host.answer("writer", &id, "notes.md")?;
    assert_eq!(state_of(&sent), "answered", "{sent:?}");
    let second = json!({"target": "writer", "message": "hello.txt", "reply_to": id});
    let refused = host.send("parent", second.as_object().ok_or("second")?);
    let refused = refused
        .err()
        .ok_or("the parent's second answer was delivered")?;
    assert!(
        refused.contains("already answered by the human"),
        "{refused}"
    );
    for _ in 0..POLL_ATTEMPTS {
        if host.status()["members"][0]["state"] == "finished" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    let child = host.children_view().pop().ok_or("no child")?;
    let transcript = serde_json::to_string(&child.session.messages())?;
    assert!(transcript.contains("answered: notes.md"), "{transcript}");
    assert!(
        presented_mail(&child.session.messages()).is_empty(),
        "neither answer is presented as mail"
    );
    let inbox = child.session.store().ok_or("no child store")?;
    let query = |kind: &str| yi_session::EntryQuery {
        custom_type: Some(kind.to_owned()),
        ..yi_session::EntryQuery::default()
    };
    let inbox = yi_session::lock_session(&inbox).find_entries(&query("agent_message"))?;
    let inbox = serde_json::to_string(&inbox)?;
    assert!(inbox.contains(r#""answeredBy":"human""#), "{inbox}");
    let journal = yi_session::lock_session(&store).find_entries(&query("human_answer"))?;
    let journal = serde_json::to_string(&journal)?;
    assert!(
        journal.contains(&format!(r#""inReplyTo":"{id}""#)) && journal.contains("notes.md"),
        "{journal}"
    );
    let told = format!("The human answered writer's question {id} for you: notes.md");
    for _ in 0..POLL_ATTEMPTS {
        if serde_json::to_string(&parent.messages())?.contains(&told) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    let root = serde_json::to_string(&parent.messages())?;
    assert!(root.contains(&told), "the parent's model is told: {root}");
    // A reply to a request nobody waits on any more is history: it wakes no finished child.
    let stray = json!({"target": "writer", "message": "late", "reply_to": "writer-99"});
    let stray = host.send("parent", stray.as_object().ok_or("stray")?)?;
    assert_ne!(state_of(&stray), "woken", "{stray:?}");
    assert_eq!(host.status()["members"][0]["state"], "finished");
    Ok(())
}

/// Dies with the human told nothing when the parent answered first: the human's answer must be
/// refused by name, never sent as a second reply.
#[tokio::test]
async fn the_parents_answer_first_refuses_the_humans() -> TestResult {
    let parent = parent_session(&["writer asks for a file name", "answered"]);
    let (_root, host, _store) = asking_family(&parent)?;
    host.spawn(
        "write a greeting file".to_owned(),
        kwargs(&[("name", "writer")]),
    )?;
    let id = asked_id(&host).await?;
    let answer = json!({"target": "writer", "message": "notes.md", "reply_to": id});
    host.send("parent", answer.as_object().ok_or("answer")?)?;
    let refused = host
        .answer("writer", &id, "hello.txt")
        .err()
        .ok_or("the human's answer was sent")?;
    assert!(
        refused.contains(&format!("{id} was already answered by \"parent\"")),
        "{refused}"
    );
    Ok(())
}

/// Dies with the ask deaf to its session: a parent's `cancel` left the child blocked on its
/// question for the whole 300 s wait.
#[tokio::test]
async fn a_cancel_ends_a_childs_open_question() -> TestResult {
    let parent = parent_session(&["writer asks for a file name", "answered"]);
    let (_root, host, _store) = asking_family(&parent)?;
    host.spawn(
        "write a greeting file".to_owned(),
        kwargs(&[("name", "writer")]),
    )?;
    asked_id(&host).await?;
    let cancel = json!({"target": "writer", "message": "stop", "kind": "cancel"});
    host.send("parent", cancel.as_object().ok_or("cancel")?)?;
    for _ in 0..POLL_ATTEMPTS {
        if host.status()["members"][0]["state"] != "needs_you" {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    Err("the cancelled child still waits on its question".into())
}

/// Dies with the headless hold read off plan children only: in `mbx-detach` `yi ask` exited 0
/// mid-way through the model's own `rlm.run` child, and its `done.txt` was never written.
#[tokio::test]
async fn a_live_rlm_run_child_holds_the_owner_until_it_ends() -> TestResult {
    let (harness, busy) = busy_child().await?;
    assert!(
        harness.host.holds_owner(),
        "a child mid-work holds a headless run"
    );
    busy.wait_idle().await;
    assert!(wait_for_status_named(&harness, "busy").await);
    assert!(
        !harness.host.holds_owner(),
        "a finished child holds nothing"
    );
    Ok(())
}

/// Dies with the reap only aborting: the waking send queued in the streaming child's turn
/// started another run after its record was gone, never concluded or billed.
#[tokio::test]
async fn a_reaped_child_starts_no_run_for_mail_queued_before_the_reap() -> TestResult {
    let (harness, busy) = busy_child().await?;
    let sent = harness.host.route("parent", "busy", "go on", true)?;
    assert_eq!(state_of(&sent), "queued");
    harness.host.delete("busy")?;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    busy.wait_idle().await;
    let messages = busy.messages();
    assert!(
        !inbound_texts(&messages)
            .iter()
            .any(|text| text.contains("go on")),
        "{messages:?}"
    );
    Ok(())
}

/// Dies with a notice for a transition the parent already read: `mbx-*` trials ended 7 of 8
/// times on a turn that said "nothing new in that notification". Dies too with a library's
/// cursor wait (another handle's `result`) marking the finish read: the kid went uncollected.
#[tokio::test]
async fn a_finish_the_parent_already_collected_is_not_presented() -> TestResult {
    for (road, notices) in [("result", 0), ("wait", 1), ("status", 1), ("none", 1)] {
        let slot: Arc<std::sync::OnceLock<Arc<SubagentHost>>> = Arc::default();
        let call = faux_assistant_message(
            vec![yi_ai::faux::faux_tool_call("c", "collect", Map::new())],
            StopReason::ToolUse,
        );
        let mut parent = parent_session_scripted(vec![call], &["kid said done", "spare"]);
        let collect = Collect {
            host: Arc::clone(&slot),
            road,
        };
        parent.set_tools(vec![Arc::new(collect)]);
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
        slot.set(Arc::clone(&harness.host)).map_err(|_| "set")?;
        parent.prompt("spawn kid and collect it")?;
        parent.wait_idle().await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let messages = parent.messages();
        let shown = notice_texts(&messages).len();
        assert_eq!(shown, notices, "{road}: {messages:?}");
        assert_eq!(assistant_count(&messages), 2, "{road}: {messages:?}");
    }
    Ok(())
}

/// A cell that spawns `kid` and blocks until it ends, collecting its answer or only watching.
struct Collect {
    host: Arc<std::sync::OnceLock<Arc<SubagentHost>>>,
    road: &'static str,
}

impl yi_loop::AgentTool for Collect {
    fn definition(&self) -> yi_types::model::ToolDef {
        yi_types::model::ToolDef {
            name: "collect".to_owned(),
            description: "collects".to_owned(),
            parameters: json!({"type": "object"}),
            freeform: None,
        }
    }

    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        _args: Map<String, Value>,
        _signal: &'a yi_loop::interrupt::InterruptSignal,
    ) -> yi_loop::tool::ToolFuture<'a> {
        Box::pin(async move {
            let host = self.host.get();
            let spawned =
                host.map(|host| host.spawn("work".to_owned(), kwargs(&[("name", "kid")])));
            let mut text = format!("spawned: {:?}", spawned.map(|reply| reply.is_ok()));
            for _ in 0..POLL_ATTEMPTS {
                let ended = host.is_some_and(|host| {
                    let view = host.children_view();
                    view.iter()
                        .any(|child| child.update.status == ChildStatus::Completed)
                });
                if let (true, Some(host)) = (ended, host) {
                    match self.road {
                        "result" => {
                            let reply = host.result("kid", None);
                            text = format!("{:?}", reply.map(|reply| reply["text"].clone()));
                        }
                        "wait" => drop(host.wait(1_000, Some(0)).await),
                        "status" => {
                            use yi_kernel::client::HostHandlers as _;
                            let mut registry = HostRegistry::default();
                            host.register(&mut registry);
                            let named = kwargs(&[("name", "someone-else")]);
                            if let Some(asked) = registry.dispatch("rlm.status", named) {
                                drop(asked.await);
                            }
                        }
                        _ => {}
                    }
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
            }
            // The notice has been queued by now; the next boundary reads it or drops it.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            yi_loop::ToolOutcome {
                result: yi_loop::tool::error_tool_result(&text),
                is_error: false,
            }
        })
    }
}

/// Dies with `wait` saying only that a child moved: the cause of each move is named, and every
/// envelope up moves the epoch, so a cell blocked in `wait` reads a child's second message.
#[tokio::test]
async fn wait_names_why_each_child_moved() -> TestResult {
    let harness = harness(0, 1, "ok")?;
    finished(&harness, "kid").await?;
    let mut cursor = 0;
    let mut moved = Vec::new();
    for step in ["finished", "mail", "progress", "reaped"] {
        match step {
            "mail" | "progress" => {
                let mut payload = kwargs(&[("target", "parent"), ("message", step)]);
                if step == "progress" {
                    payload.insert("kind".to_owned(), json!("progress"));
                }
                harness.host.send("kid", &payload)?;
            }
            "reaped" => drop(harness.host.delete("kid")?),
            _ => {}
        }
        let reply = harness.host.wait(2_000, Some(cursor)).await?;
        cursor = reply["cursor"].as_u64().ok_or("cursor")?;
        moved.push(json!(reply)["causes"]["kid"].clone());
    }
    assert_eq!(
        moved,
        [
            json!("finished"),
            json!("mail"),
            json!("progress"),
            json!("reaped")
        ]
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
    let settled = harness.host.wait(0, None).await?;
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

    let woken = harness.host.wait(60_000, cursor).await?;
    assert_eq!(woken["updated"], serde_json::json!(["scout"]));
    let quiet = harness.host.wait(0, woken["cursor"].as_u64()).await;
    assert!(
        quiet.is_err_and(|said| said.contains("settled")),
        "an update is behind the cursor it was read at, not reported forever"
    );
    assert_eq!(settled["timeout_ms"], 1000, "the clamp is applied");
    assert_eq!(settled["clamped"], true, "and reported");
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

fn service_row(harness: &Harness, name: &str) -> Value {
    let status = harness.host.status();
    let members = status["members"].as_array().cloned().unwrap_or_default();
    let found = members.into_iter().find(|member| member["name"] == name);
    found.unwrap_or_default()
}

async fn serves(harness: &Harness, name: &str, incarnation: u64, state: &str) -> bool {
    for _ in 0..POLL_ATTEMPTS {
        let row = service_row(harness, name);
        if row["incarnation"] == incarnation && row["state"] == state {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    false
}

/// F3c. Dies with the control: retire the crashed record and the name is free for `rlm.run`
/// and the inbox is a new file; keep the waiter and incarnation 2 answers a request put to 1;
/// drop the stamps and nothing in the kept inbox says whose conversation a message was.
#[tokio::test]
async fn a_service_respawns_under_its_name_and_keeps_its_inbox() -> TestResult {
    let harness = harness_with(HarnessOptions {
        child_errors: false,
        depth: 0,
        max_depth: 1,
        child_answer: "serving",
        tool_command: Some("sleep 1"),
        cwd: None,
        wake_parent: None,
    })?;
    harness.faults.lock().map_err(|_| "poisoned")?.push_back(1);
    let mut updates = harness.events.subscribe();
    let brief = "serve the index".to_owned();
    let handle = harness
        .host
        .service("index", brief.clone(), Map::new(), 3)?;
    assert!(serves(&harness, "index", 1, "running").await);
    assert_eq!(service_row(&harness, "index")["service"], true);

    let first = harness
        .host
        .route("parent", "index", "for the first", false)?;
    assert_eq!(first["receipts"][0]["target"], "index");
    let host = Arc::clone(&harness.host);
    let parked = tokio::spawn(async move { host.request("parent", "index", "ping", 8_000).await });
    let taken = harness
        .host
        .spawn("an ordinary child".to_owned(), kwargs(&[("name", "index")]));
    assert!(taken.is_err_and(|refusal| refusal.contains("already taken")));
    // The kernel's road: `rlm.service` on the registry, which is what `rlm.service(...)` sends.
    use yi_kernel::client::HostHandlers as _;
    let mut registry = yi_runtime::HostRegistry::default();
    harness.host.register(&mut registry);
    let payload = serde_json::json!({"name": "index", "prompt": brief, "restart": 3, "kwargs": {}});
    let asked = registry.dispatch(
        "rlm.service",
        payload.as_object().cloned().ok_or("payload")?,
    );
    let again = asked.ok_or("rlm.service is not registered")?.await?;
    assert_eq!(
        (&again["rlm_child_id"], &again["attached"]),
        (&handle["rlm_child_id"], &Value::Bool(true)),
        "the same brief attaches"
    );
    let other = harness
        .host
        .service("index", "serve something else".to_owned(), Map::new(), 3);
    assert!(other.is_err_and(|refusal| refusal.contains("another brief")));

    assert!(
        serves(&harness, "index", 2, "finished").await,
        "{:?}",
        service_row(&harness, "index")
    );
    let refused = parked.await?.err().unwrap_or_default();
    assert!(
        refused.contains("\"index\" was respawned before it replied"),
        "{refused}"
    );
    // Dies with the hand-off in `respawn`: the send the first run never drained is owed a turn.
    let views = harness.host.children_view();
    let texts = serde_json::to_string(&views.first().ok_or("no view")?.session.messages())?;
    assert!(texts.contains("for the first"), "{texts}");
    assert_eq!(
        harness.host.children_view().len(),
        1,
        "a respawn never duplicates"
    );
    harness
        .host
        .route("parent", "index", "for the second", false)?;
    let inbox = inbox_of(&harness, "index")?;
    let addressed = |body: &str, incarnation: u32| {
        let stamp = format!("\"toIncarnation\":{incarnation}");
        inbox
            .iter()
            .any(|entry| entry.contains(body) && entry.contains(&stamp))
    };
    assert!(
        addressed("for the first", 1) && addressed("ping", 1),
        "{inbox:?}"
    );
    assert!(addressed("for the second", 2), "{inbox:?}");

    let desk =
        yi_runtime::fetch::SessionTranscripts::new(Arc::clone(&harness.host), None, &harness.root);
    let resolver =
        yi_runtime::fetch::Resolver::new(harness.root.to_path_buf(), yi_runtime::Wall::default())
            .with_transcripts(Arc::new(desk));
    let history = resolver.fetch(&"history://index".parse()?)?.text;
    assert_eq!(
        history.matches("serve the index").count(),
        2,
        "one chain, both runs: {history}"
    );
    assert!(history.contains("[incarnation 2 of service"), "{history}");
    let told = harness.notices.lock().map_err(|_| "poisoned")?.join("\n");
    assert!(
        !told.contains("index"),
        "a respawn and an idle service are no ending: {told}"
    );
    // A card the crash committed out of the chrome would never come back as incarnation 2.
    while let Ok(AgentEvent::ChildUpdate { update }) = updates.try_recv() {
        assert_ne!(
            (update.status, update.error.is_some()),
            (ChildStatus::Error, true),
            "a respawned run published an ending: {update:?}"
        );
    }
    Ok(())
}

/// Dies with `FailClass::KernelDeath` matched but never built: a service whose kernel died under
/// a cell kept its run, lost its state, and stayed incarnation 1.
#[tokio::test]
async fn a_service_whose_kernel_dies_respawns_with_its_pending_mail() -> TestResult {
    let root = Scratch::new("yi-kernel-death")?;
    let builds = Arc::new(AtomicU32::new(0));
    let (counted, dir) = (Arc::clone(&builds), root.to_path_buf());
    let store = support::memory_store("kernel-death");
    let (events, _keep) = tokio::sync::broadcast::channel(256);
    let host = Arc::new(SubagentHost::new(SubagentHostOptions {
        provider: Arc::new(ProviderStream::new(None)),
        depth: 0,
        max_depth: 1,
        max_children: 8,
        parent_session_dir: root.to_path_buf(),
        cwd: root.to_path_buf(),
        home: root.join("home"),
        lane_slots: 1,
        defaults: Arc::new(|| (faux_model(), yi_types::model::Effort::Medium)),
        factory: Arc::new(move |build: yi_runtime::ChildBuild<'_>| {
            let first = counted.fetch_add(1, Ordering::SeqCst) == 0;
            let provider = Arc::new(ProviderStream::new(None));
            let config = SessionConfig {
                system_prompt: "child sys".to_owned(),
                model: build.model,
                thinking_level: build.thinking,
                tool_execution: ExecutionMode::Sequential,
            };
            if !first {
                provider.queue_faux(vec![child_reply("serving"), child_reply("serving")]);
                return Ok(AgentSession::new(config, provider));
            }
            let code = json!({"code": "import os\nos._exit(3)"});
            let cell = yi_ai::faux::faux_tool_call(
                "die",
                "ipython",
                code.as_object().cloned().unwrap_or_default(),
            );
            provider.queue_faux(vec![
                faux_assistant_message(vec![cell], StopReason::ToolUse),
                child_reply("carried on without my state"),
            ]);
            let mut child = AgentSession::new(config, provider);
            let mut registry = HostRegistry::default();
            registry.register_mcp_stubs();
            let kernel = Arc::new(KernelService::new(KernelServiceOptions {
                cwd: dir.clone(),
                home: std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_default(),
                session_dir: Some(dir.join("kernel")),
                family_dir: None,
                host: Arc::new(registry),
                on_restore: None,
                on_boot: None,
                sandbox: None,
                snapshot_key: None,
                per_session_state: false,
                cell_ceiling: None,
            }));
            child.use_tools(
                vec![yi_runtime::kernel::ipython_tool(Arc::clone(&kernel))],
                dir.clone(),
                None,
            );
            child.set_kernel_service(kernel);
            Ok(child)
        }),
        notice: Arc::new(|_text: &str, _| {}),
        events,
        parent_messages: Arc::new(Vec::new),
        report: Arc::new(|_message, _| {}),
        attribute: Arc::new(|_usage| {}),
        store: Arc::new(move || Some(store.clone())),
        plans_dir: root.join(".yi/plans"),
        family_live: Arc::new(|| 0),
    }));
    host.service("index", "serve the index".to_owned(), Map::new(), 3)?;
    let first = host.children_view().pop().ok_or("no service")?;
    while first.session.status() != yi_runtime::Status::Running {
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    let sent = host.route("parent", "index", "for the next run", false)?;
    assert_eq!(state_of(&sent), "queued", "{sent:?}");
    let row = || host.status()["members"][0].clone();
    for _ in 0..1_000 {
        if row()["incarnation"] == 2 && row()["state"] == "finished" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    assert_eq!(
        (row()["incarnation"].clone(), row()["state"].clone()),
        (json!(2), json!("finished")),
        "{}",
        row()
    );
    let now = host.children_view().pop().ok_or("no service")?;
    let texts = serde_json::to_string(&now.session.messages())?;
    assert!(texts.contains("its kernel died under a cell"), "{texts}");
    assert!(
        texts.contains("for the next run"),
        "the queued send moved on: {texts}"
    );
    Ok(())
}

/// The stop lands in the one window a respawn holds no lock in, between the draw and the
/// record taking the new session. Dies with the control: read the mark only before the build
/// and a revoked service comes back as incarnation 2.
#[tokio::test]
async fn a_service_revoked_while_its_next_run_is_built_never_comes_back() -> TestResult {
    let harness = harness(0, 1, "serving")?;
    harness.faults.lock().map_err(|_| "poisoned")?.push_back(1);
    harness
        .host
        .service("index", "serve".to_owned(), Map::new(), 3)?;
    let host = Arc::downgrade(&harness.host);
    *harness.during_build.lock().map_err(|_| "poisoned")? = Some(Box::new(move || {
        if let Some(host) = host.upgrade() {
            let _the_dead_run_may_refuse_the_cancel = host.revoke("index", 30_000, "enough");
        }
    }));
    assert!(
        serves(&harness, "index", 1, "failed").await,
        "{:?}",
        service_row(&harness, "index")
    );
    let told = harness.notices.lock().map_err(|_| "poisoned")?.join("\n");
    assert!(
        told.contains("not respawned: it was stopped while its next run was being built"),
        "{told}"
    );
    Ok(())
}

/// Dies with the control: bill the kept transcript whole and the crashed run's unknown usage
/// spends the successor's reservation too, leaving the parent nothing to lend.
#[tokio::test]
async fn a_respawned_service_is_billed_from_its_own_first_turn() -> TestResult {
    let harness = harness(0, 1, "serving")?;
    harness
        .host
        .set_grant(yi_runtime::Wall::default(), Some(1_000));
    harness.faults.lock().map_err(|_| "poisoned")?.push_back(1);
    let mut asked = Map::new();
    asked.insert("tokens".to_owned(), Value::from(400));
    harness
        .host
        .service("index", "serve".to_owned(), asked, 3)?;
    assert!(
        serves(&harness, "index", 2, "finished").await,
        "{:?}",
        service_row(&harness, "index")
    );
    harness.host.reap("index")?;
    // The crashed run spent its own 400 whole, on usage it could not report; the successor
    // spends only the 120 of its own turn, so 480 of the 1,000 are still there to lend.
    let mut big = Map::new();
    big.insert("tokens".to_owned(), Value::from(400));
    big.insert("name".to_owned(), Value::String("after".to_owned()));
    harness.host.spawn("work".to_owned(), big)?;
    Ok(())
}

/// Dies with the control: drop the intensity and the third crash respawns; clamp the lease
/// and a parent out of clock still gets a fresh incarnation.
#[tokio::test]
async fn a_service_out_of_restarts_or_lease_ends_failed_and_says_so() -> TestResult {
    let harness = harness(0, 1, "serving")?;
    harness
        .faults
        .lock()
        .map_err(|_| "poisoned")?
        .extend([1, 1]);
    harness
        .host
        .service("index", "serve".to_owned(), Map::new(), 1)?;
    assert!(
        serves(&harness, "index", 2, "failed").await,
        "{:?}",
        service_row(&harness, "index")
    );
    let told = harness.notices.lock().map_err(|_| "poisoned")?.join("\n");
    assert!(
        told.contains("not respawned: 1 restarts within 600 s are spent"),
        "{told}"
    );

    let harness = harness_with(HarnessOptions {
        child_errors: true,
        depth: 0,
        max_depth: 1,
        child_answer: "serving",
        tool_command: Some("sleep 1"),
        cwd: None,
        wake_parent: None,
    })?;
    harness
        .host
        .service("index", "serve".to_owned(), Map::new(), 3)?;
    assert!(serves(&harness, "index", 1, "running").await);
    harness.host.set_deadline(Some(std::time::Instant::now()));
    assert!(
        serves(&harness, "index", 1, "failed").await,
        "{:?}",
        service_row(&harness, "index")
    );
    let told = harness.notices.lock().map_err(|_| "poisoned")?.join("\n");
    assert!(
        told.contains("not respawned: the parent's own deadline has passed"),
        "{told}"
    );

    Ok(())
}

/// A crash on a turn that reported no usage spends the whole reservation, so the parent has
/// nothing left for the next incarnation. Dies with the control: mint the lease instead of
/// drawing it and the service comes back on tokens nobody holds.
#[tokio::test]
async fn a_service_the_parent_cannot_relend_ends_failed_and_says_so() -> TestResult {
    let harness = harness(0, 1, "serving")?;
    harness
        .host
        .set_grant(yi_runtime::Wall::default(), Some(200));
    harness.faults.lock().map_err(|_| "poisoned")?.push_back(1);
    let mut asked = Map::new();
    asked.insert("tokens".to_owned(), Value::from(200));
    harness
        .host
        .service("index", "serve".to_owned(), asked, 3)?;
    assert!(
        serves(&harness, "index", 1, "failed").await,
        "{:?}",
        service_row(&harness, "index")
    );
    let told = harness.notices.lock().map_err(|_| "poisoned")?.join("\n");
    assert!(
        told.contains("not respawned: tokens asks for 200 and the parent has 0"),
        "{told}"
    );
    Ok(())
}

/// Dies with the control: leave woken turns unread and the first half never reaches
/// incarnation 2; let a close leave the service standing and the second half does.
#[tokio::test]
async fn a_woken_crash_respawns_and_a_parent_close_ends_a_service_for_good() -> TestResult {
    for closes in [false, true] {
        let harness = harness(0, 1, "serving")?;
        harness.faults.lock().map_err(|_| "poisoned")?.push_back(2);
        harness
            .host
            .service("index", "serve".to_owned(), Map::new(), 3)?;
        assert!(serves(&harness, "index", 1, "finished").await);
        if closes {
            harness.host.close();
        }
        harness.host.route("parent", "index", "wake up", true)?;
        let (incarnation, state) = if closes {
            (1, "failed")
        } else {
            (2, "finished")
        };
        let row = service_row(&harness, "index");
        assert!(
            serves(&harness, "index", incarnation, state).await,
            "{closes}: {row:?}"
        );
    }
    Ok(())
}

/// Dies with the control: count a service as a worker and the eighth child is refused; seat
/// it like a juror and a leaf host spawns one past its depth.
#[tokio::test]
async fn a_service_is_outside_the_worker_cap_and_under_the_depth_limit() -> TestResult {
    let leaf = harness(1, 1, "serving")?;
    let deep = leaf
        .host
        .service("index", "serve".to_owned(), Map::new(), 3);
    assert!(deep.is_err_and(|refusal| refusal.contains("depth limit")));
    let harness = harness(0, 1, "serving")?;
    let lane = kwargs(&[("isolation", "worktree")]);
    let walled = harness.host.service("index", "serve".to_owned(), lane, 3);
    assert!(walled.is_err_and(|refusal| refusal.contains("parent's tree")));
    let greedy = harness
        .host
        .service("index", "serve".to_owned(), Map::new(), 99);
    assert!(greedy.is_err_and(|refusal| refusal.contains("nothing is clamped")));
    harness
        .host
        .service("index", "serve".to_owned(), Map::new(), 3)?;
    for number in 0..8 {
        let name = format!("worker-{number}");
        harness
            .host
            .spawn("work".to_owned(), kwargs(&[("name", &name)]))?;
    }
    let ninth = harness
        .host
        .spawn("work".to_owned(), kwargs(&[("name", "worker-8")]));
    assert!(ninth.is_err_and(|refusal| refusal.contains("child limit")));
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
            // The refusal names the two calls that do it; naming only the rule cost four
            // F0e sessions a turn apiece (#475).
            .is_some_and(|error| {
                error.contains("rlm.merge_worktree(\"mutator\")")
                    && error.contains("rlm.discard_worktree(\"mutator\")")
            }),
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
    // Dies with the reserved names in `reserve`: `main` is the owner's inline agent.
    for word in ["main", "host"] {
        let reserved = harness
            .host
            .spawn("impostor".to_owned(), kwargs(&[("name", word)]));
        assert!(
            reserved.is_err_and(|error| error.contains("is reserved")),
            "{word}"
        );
    }

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
        on_boot: None,
        sandbox: None,
        snapshot_key: None,
        per_session_state: false,
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

/// Dies with the check run in the process cwd: under `yi ask --cwd` the `mbx-discovery` trial
/// held back 5 of 5 collects while `notes.txt` sat in the session's cwd.
#[tokio::test]
async fn a_checked_childs_check_runs_in_the_session_cwd() -> TestResult {
    let cwd = Scratch::new("yi-check-cwd")?;
    std::fs::write(cwd.join("notes.txt"), "kept")?;
    let harness = harness_with(HarnessOptions {
        child_errors: false,
        depth: 0,
        max_depth: 1,
        child_answer: "{\"value\": 1, \"discoveries\": []}",
        tool_command: None,
        cwd: Some(cwd.to_path_buf()),
        wake_parent: None,
    })?;
    let check = json!("test -f notes.txt");
    let kwargs = protocol_kwargs("auditor", &[("check", check)]);
    harness
        .host
        .spawn("write the notes".to_owned(), kwargs)
        .map_err(|error| error.to_string())?;
    assert!(wait_for_status_named(&harness, "auditor").await);
    let reply = harness.host.result("auditor", None)?;
    assert_eq!(reply.get("value"), Some(&json!(1)), "{reply:?}");
    Ok(())
}

/// Dies with no `rlm.receive`: in the `mbx-siblings` trial beta, told to wait for alpha's
/// number, polled the filesystem and read alpha's kernel, and alpha's mail came after its answer.
#[tokio::test]
async fn a_sibling_receives_its_mail_instead_of_polling() -> TestResult {
    use yi_kernel::client::HostHandlers as _;
    let harness = harness_with(HarnessOptions {
        child_errors: false,
        depth: 0,
        max_depth: 1,
        child_answer: "ok",
        tool_command: Some("sleep 2"),
        cwd: None,
        wake_parent: None,
    })?;
    harness
        .host
        .spawn("tell beta 391".to_owned(), kwargs(&[("name", "alpha")]))?;
    harness
        .host
        .spawn("wait for alpha".to_owned(), kwargs(&[("name", "beta")]))?;
    let beta = harness
        .receivers
        .lock()
        .map_err(|_| "poisoned")?
        .remove("beta");
    let beta = beta.ok_or("beta was built without its receive")?;
    let payload = json!({"timeout_ms": 10_000})
        .as_object()
        .cloned()
        .ok_or("payload")?;
    let waiting = beta
        .dispatch("rlm.receive", payload)
        .ok_or("rlm.receive is not registered")?;
    let waiting = tokio::spawn(waiting);
    let feed = harness.host.children_view();
    let feed = feed
        .iter()
        .find(|child| child.update.name == "beta")
        .ok_or("no beta")?;
    while feed.session.status() != yi_runtime::Status::Running {
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    let sent = harness.host.route("alpha", "beta", "391", false)?;
    assert_eq!(state_of(&sent), "queued", "{sent:?}");
    let got = waiting.await??;
    let envelopes = got["envelopes"].as_array().ok_or("no envelopes")?;
    let read: Vec<(&str, &str)> = envelopes
        .iter()
        .filter_map(|envelope| Some((envelope["from"].as_str()?, envelope["body"].as_str()?)))
        .collect();
    assert_eq!(read, [("alpha", "391")], "{got:?}");
    assert!(wait_for_status_named(&harness, "beta").await);
    assert!(
        presented(&harness, "beta").is_empty(),
        "received mail is never shown twice"
    );
    let reread = parent_session(&[]);
    reread.attach_store(harness.host.transcript("beta").ok_or("no transcript")?)?;
    assert_eq!(
        reread.pending_count(),
        0,
        "the read mark is durable across a restart"
    );
    Ok(())
}

/// Adjudication reads the canonical plan file, never the session fact: the
/// named todo's delegation carries the runnable acceptance.
fn write_canonical_plan(cwd: &std::path::Path, todos: &[(&str, &str)]) -> TestResult {
    use yi_types::plan::doc::{
        Check, Delegation, GoalText, Plan, PlanId, PlanTier, SpawnSpec, Todo, TodoLabel,
    };
    let store = yi_runtime::plan::store::PlanStore::open(cwd.join(".yi/plans"))?;
    let todos = todos
        .iter()
        .map(|(label, check)| {
            Ok(Todo {
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
                ..Todo::pending(TodoLabel::new(*label)?)
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
    own(&harness.store, "adjudication")?;
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
    own(&harness.store, "adjudication")?;
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
    notice: Arc<yi_runtime::subagent::NoticeFn>,
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
        (self.notice)(
            "[subagent helper (sub-1) finished]\nLast answer: done at the seam",
            None,
        );
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

/// Guards the one queue: two children finish in milliseconds while the parent's turn sleeps two
/// seconds in a tool, so both notices ride that turn's next boundary and its one reply reads
/// them; the old `run(message)` drop lost the one that answered Busy, and the old follow-up
/// queue presented them after the answer and owed a third turn that said nothing new.
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
        2,
        "the tool call and one reply that read both notices: {messages:?}"
    );
    let answer = messages
        .iter()
        .rposition(|message| matches!(message, AgentMessage::Assistant { .. }))
        .ok_or("no reply")?;
    let read = notice_texts(messages.get(..answer).unwrap_or_default());
    assert_eq!(
        read.len(),
        2,
        "both notices come before the reply: {messages:?}"
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
    wake(
        "[subagent helper (sub-1) finished]\nLast answer: done",
        None,
    );
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
    let (first, second) = (first?, second?);
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
    let (first, second) = (first?, second?);
    for reply in [&first, &second] {
        assert_eq!(reply["changed"], json!(["solo"]), "{reply:?}");
        assert_eq!(reply["updated"], reply["changed"]);
        assert_eq!(reply["states"]["solo"], json!("finished"), "{reply:?}");
        assert!(reply["cursor"].as_u64().is_some_and(|cursor| cursor > 0));
    }
    let cursor = first["cursor"].as_u64().ok_or("cursor missing")?;
    let quiet = harness.host.wait(1_000, Some(cursor)).await;
    assert!(
        quiet.is_err_and(|said| said.contains("settled")),
        "nothing moved past the cursor, and the family is settled"
    );
    Ok(())
}

/// Guards the wake on a delete, the path `rlm.delete_subagent` takes: the removed record
/// carries no epoch, so a waiter that only returned on a named change slept to its deadline
/// while `states` already lacked the child.
#[tokio::test]
async fn a_delete_wakes_a_waiter_with_the_child_gone() -> TestResult {
    // A family with nothing live answers a wait at once; this one holds its child in a call.
    let harness = harness_with(HarnessOptions {
        child_errors: false,
        depth: 0,
        max_depth: 1,
        child_answer: "done",
        tool_command: Some("sleep 30"),
        cwd: None,
        wake_parent: None,
    })?;
    let reply = harness
        .host
        .spawn("finish now".to_owned(), kwargs(&[("name", "gone")]))
        .map_err(|error| error.to_string())?;
    let child_id = reply["rlm_child_id"]
        .as_str()
        .ok_or("missing child id")?
        .to_owned();
    assert!(wait_for_status(&harness.host, &child_id, "running").await);
    // Its start and its first tool call move the epoch too; the cursor is taken after them.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let seen = harness.host.wait(0, None).await?;
    let cursor = seen["cursor"].as_u64();
    let host = Arc::clone(&harness.host);
    let waiter = tokio::spawn(async move { host.wait(300_000, cursor).await });
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let started = std::time::Instant::now();
    harness.host.delete("gone")?;
    let woken = waiter.await.map_err(|error| error.to_string())??;
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "a delete wakes the waiter, not the deadline"
    );
    assert_eq!(
        woken["changed"],
        json!(["gone"]),
        "the reap is a named move: {woken:?}"
    );
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
    let collected = harness.host.wait(1_000, None).await?;
    assert_eq!(collected["changed"], json!(["early"]));
    let started = std::time::Instant::now();
    let late = harness.host.wait(300_000, None).await?;
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
        on_boot: None,
        sandbox: None,
        snapshot_key: None,
        per_session_state: false,
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

    // Dies with `ChildRecord::running`: a child that files its own exit by declaring failure
    // reads `failed` and its turn keeps writing, so `exit` alone buys it out of its grace.
    let mut own = Map::new();
    own.insert("target".to_owned(), json!("parent"));
    own.insert("message".to_owned(), json!("I gave up"));
    own.insert("kind".to_owned(), json!("failure"));
    family.host.send("held", &own)?;
    assert_eq!(family.state_of("held").as_deref(), Some("failed"));

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
    // One lease finished before the restart, so the resume must read the journal oldest first:
    // newest first and its `revoked` line outlives the `repossessed` that answered it.
    family
        .host
        .spawn("hold".to_owned(), kwargs(&[("name", "earlier")]))?;
    executing(&family).await?;
    family.host.revoke("earlier", 0, "scope changed")?;
    assert_eq!(family.host.expire().await, ["earlier"]);
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
    let holders: Vec<(&str, &str)> = journal
        .iter()
        .map(|record| match record {
            LeaseRecord::Revoked(lease) => ("revoked", lease.holder.as_str()),
            LeaseRecord::Repossessed(done) => ("repossessed", done.lease.holder.as_str()),
            LeaseRecord::Returned(back) => ("returned", back.lease.holder.as_str()),
        })
        .collect();
    assert_eq!(
        holders,
        [
            ("revoked", "earlier"),
            ("repossessed", "earlier"),
            ("revoked", "held"),
            ("repossessed", "held"),
        ],
        "the first expiry after a restart completes only the open record"
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

/// Dies with the refusal in `resume_revocations`: skip a lease line this build cannot read and
/// the revocation it may have closed is completed again, a kept branch called an orphan.
#[tokio::test]
async fn a_lease_record_this_build_cannot_read_refuses_the_resume() -> TestResult {
    use yi_types::lease::LeaseRecord;
    let (root, family, _ticks) = leased("yi-lease-unknown", "sleep 30", None).await?;
    family
        .host
        .spawn("hold".to_owned(), kwargs(&[("name", "held")]))?;
    executing(&family).await?;
    family.host.revoke("held", 30_000, "scope changed")?;
    family.host.interrupt("held")?;
    let newer = json!({"event": "forfeited", "lease": {"holder": "held", "parent": "parent", "grantedAt": 1}});
    yi_session::lock_session(&family.store).append_custom("main", "lease", Some(newer))?;
    let store = family.store.clone();
    drop(family);

    let next = support::family(root.to_path_buf(), std::env::temp_dir(), store, None);
    assert!(next.host.expire().await.is_empty());
    let journal = next.journal();
    assert!(
        !journal
            .iter()
            .any(|record| matches!(record, LeaseRecord::Repossessed(_))),
        "no repossession is invented past a line it cannot read: {journal:?}"
    );
    let told = next.notices.lock().map_err(|_| "poisoned")?.clone();
    assert!(
        told.iter()
            .any(|text| text.contains("lease resume refused")),
        "{told:?}"
    );
    Ok(())
}

/// Dies with the exit check in `repossess`: `expire` reads its due list once, so a sibling
/// whose run ends while the first is joined is repossessed too, on top of its own ending,
/// and the journal closes that lease twice. This is also the expire-versus-conclude pin.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_child_that_ends_while_expire_joins_a_sibling_keeps_its_one_ending() -> TestResult {
    use yi_types::lease::LeaseRecord;
    let (_root, family, _ticks) = leased("yi-lease-race", "sleep 30", None).await?;
    for name in ["a", "b"] {
        family
            .host
            .spawn("hold".to_owned(), kwargs(&[("name", name)]))?;
    }
    for _ in 0..POLL_ATTEMPTS {
        let views = family.host.children_view();
        if views.len() == 2
            && views
                .iter()
                .all(|view| view.update.activity == ChildActivity::Executing)
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    for name in ["a", "b"] {
        family.host.revoke(name, 0, "scope changed")?;
    }
    // `expire` runs on this thread; its second clock read is the first repossession's, after
    // its join, and the other run ends there, after the due list was read. A read a child's
    // own task makes, under the roster lock, is left alone.
    let (reads, host) = (AtomicU32::new(0), Arc::downgrade(&family.host));
    let expiring = std::thread::current().id();
    let clock = Arc::new(move || {
        if std::thread::current().id() == expiring
            && reads.fetch_add(1, Ordering::SeqCst) == 1
            && let Some(host) = host.upgrade()
        {
            for view in host.children_view() {
                let _ = host.interrupt(&view.update.name);
            }
            for _ in 0..POLL_ATTEMPTS {
                let views = host.children_view();
                if views.iter().any(|view| view.update.exit.is_some()) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS));
            }
        }
        1_000_000
    });
    family.host.set_lease_clock(Some(clock), None);
    let mut events = family.events.subscribe();

    let taken = family.host.expire().await;
    let [first] = &taken[..] else {
        return Err(format!("only the joined child is repossessed: {taken:?}").into());
    };
    let other = if first == "a" { "b" } else { "a" };
    let ended = terminal(&mut events);
    // A late event may republish an ending unchanged; what may not happen is a second one.
    let exits = |name: &str| {
        let mut seen: Vec<_> = ended
            .iter()
            .filter(|update| update.name == name)
            .map(|update| update.exit)
            .collect();
        seen.dedup();
        seen
    };
    let repossessed = Some(yi_types::subagent::ChildExit::Repossessed);
    assert_eq!(exits(first), [repossessed], "{ended:?}");
    assert_eq!(
        exits(other),
        [Some(yi_types::subagent::ChildExit::Interrupted)],
        "{ended:?}"
    );
    family.host.delete(other)?;
    let journal = family.journal();
    let closing: Vec<(&str, &str)> = journal
        .iter()
        .filter_map(|record| match record {
            LeaseRecord::Revoked(_) => None,
            LeaseRecord::Repossessed(done) => Some(("repossessed", done.lease.holder.as_str())),
            LeaseRecord::Returned(back) => Some(("returned", back.lease.holder.as_str())),
        })
        .map(|(kind, holder)| (kind, if holder == first { "first" } else { "other" }))
        .collect();
    assert_eq!(
        closing,
        [("repossessed", "first"), ("returned", "other")],
        "each lease closes once"
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
    let waited = harness.host.wait(1_000, None).await?;
    assert_eq!(waited["states"]["sorry"], json!("failed"), "{waited:?}");
    assert_eq!(waited["notes"]["sorry"], json!("the fixture is missing"));
    Ok(())
}
