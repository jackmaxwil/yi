#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
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
}

struct HarnessOptions {
    depth: u8,
    max_depth: u8,
    child_answer: &'static str,
    /// Some(command) makes the child call `bash` before answering, which is
    /// what moves the B7 tool counter and activity.
    tool_command: Option<&'static str>,
    /// The repository worktree children branch from.
    cwd: Option<PathBuf>,
}

fn harness(depth: u8, max_depth: u8, child_answer: &'static str) -> std::io::Result<Harness> {
    harness_with(HarnessOptions {
        depth,
        max_depth,
        child_answer,
        tool_command: None,
        cwd: None,
    })
}

fn harness_with(options: HarnessOptions) -> std::io::Result<Harness> {
    let HarnessOptions {
        depth,
        max_depth,
        child_answer,
        tool_command,
        cwd,
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
        notice: Arc::new(move |text: &str| {
            if let Ok(mut sink) = notice_sink.lock() {
                sink.push(text.to_owned());
            }
        }),
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
        depth: 0,
        max_depth: 1,
        child_answer: "swept the logs",
        tool_command: Some("echo probing"),
        cwd: None,
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

    let queued = harness
        .host
        .route("parent", "beta", "no rush", false)
        .map_err(|error| error.to_string())?;
    assert_eq!(queued["receipts"][0]["state"], "queued");
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

#[tokio::test]
async fn a_child_reports_upward_and_the_parent_waits_for_it() -> TestResult {
    let harness = harness(0, 1, "ok")?;
    harness
        .host
        .spawn("do it".to_owned(), kwargs(&[("name", "scout")]))
        .map_err(|error| error.to_string())?;
    assert!(child_sees(&harness, "scout", "[task from parent]").await);
    // Drains the terminal transition so the wait below observes the report only.
    harness.host.wait(0).await;

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

    let woken = harness.host.wait(60_000).await;
    assert_eq!(woken["updated"], serde_json::json!(["scout"]));
    let quiet = harness.host.wait(0).await;
    assert_eq!(
        quiet["updated"],
        serde_json::json!([]),
        "an update is collected once, not reported forever"
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
        depth: 0,
        max_depth: 1,
        child_answer: "isolated",
        tool_command: None,
        cwd: Some(repo.to_path_buf()),
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
        depth: 0,
        max_depth: 1,
        child_answer: "discarded",
        tool_command: None,
        cwd: Some(repo.to_path_buf()),
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
                "agents = await rlm.list_agents()\nreceipt = await rlm.send('helper', 'status?')\nprint([a['name'] for a in agents], receipt['receipts'][0]['state'])",
                &cancelled,
            )
        }
    })
    .await??;
    assert!(
        roster_cell
            .result
            .stdout
            .contains("['parent', 'helper'] queued"),
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
    store.write(&yi_runtime::plan::store::PlanFile {
        plan,
        body: String::new(),
    })?;
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
        depth: 0,
        max_depth: 1,
        child_answer: ONE_DISCOVERY,
        tool_command: None,
        cwd: Some(cwd.to_path_buf()),
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
        depth: 0,
        max_depth: 1,
        child_answer: ONE_DISCOVERY,
        tool_command: None,
        cwd: Some(cwd.to_path_buf()),
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
        depth: 0,
        max_depth: 1,
        child_answer: TWO_DISCOVERIES,
        tool_command: None,
        cwd: Some(cwd.to_path_buf()),
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
