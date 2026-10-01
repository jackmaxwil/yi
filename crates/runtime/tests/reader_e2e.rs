use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_runtime::{AgentSession, ProviderStream, SessionConfig, SubagentHost, SubagentHostOptions};
use yi_types::message::{AgentMessage, Content, StopReason, UserContent};
use yi_types::model::{Effort, Model, ModelCost, Reuse};

type TestResult = Result<(), Box<dyn Error>>;
type Script = Arc<Mutex<Vec<AgentMessage>>>;

fn faux_model(context_window: u64) -> Model {
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
        context_window,
        max_tokens: 16_384,
        compat: None,
        thinking_level_map: None,
        headers: None,
    }
}

struct Family {
    root: Scratch,
    host: Arc<SubagentHost>,
    children: Arc<Mutex<Vec<Vec<String>>>>,
    notices: Arc<Mutex<Vec<String>>>,
    shapes: Arc<Mutex<Vec<Shape>>>,
    turns: Arc<Mutex<Vec<u32>>>,
}

type Shape = (Option<yi_runtime::session::RequestShape>, Reuse);

type Rules = Option<Arc<yi_runtime::rules::RuleEngine>>;
type Hook = Arc<dyn Fn() + Send + Sync>;

/// `during` runs inside a reader's build, while its reservation is held.
struct Setup {
    rules: Rules,
    during: Option<Hook>,
    broker: Option<Arc<yi_runtime::PermissionBroker>>,
    window: u64,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            rules: None,
            during: None,
            broker: None,
            window: 128_000,
        }
    }
}

fn family(max_children: usize, script: Script) -> std::io::Result<Family> {
    family_with(max_children, script, Setup::default())
}

/// A reader is built by the runtime's own `reader::session`; any other child is a plain one.
fn family_with(max_children: usize, script: Script, setup: Setup) -> std::io::Result<Family> {
    let Setup {
        rules,
        during,
        broker,
        window,
    } = setup;
    let root = Scratch::new("yi-reader")?;
    let workspace = root.join("ws");
    std::fs::create_dir_all(&workspace)?;
    std::fs::write(
        workspace.join("notes.txt"),
        "red sun\nblue sky\ngreen sea\n",
    )?;
    let (events, _keep) = tokio::sync::broadcast::channel(64);
    let tool_names: Arc<Mutex<Vec<Vec<String>>>> = Arc::default();
    let names_sink = Arc::clone(&tool_names);
    let notices: Arc<Mutex<Vec<String>>> = Arc::default();
    let notice_sink = Arc::clone(&notices);
    let shapes: Arc<Mutex<Vec<Shape>>> = Arc::default();
    let shape_sink = Arc::clone(&shapes);
    let turns: Arc<Mutex<Vec<u32>>> = Arc::default();
    let turns_sink = Arc::clone(&turns);
    let (cwd, home) = (workspace.clone(), root.join("home"));
    let host = Arc::new(SubagentHost::new(SubagentHostOptions {
        depth: 0,
        max_depth: 1,
        max_children,
        parent_session_dir: root.join("family"),
        cwd: workspace.clone(),
        home: home.clone(),
        lane_slots: 1,
        provider: Arc::new(ProviderStream::new(None)),
        defaults: Arc::new(move || (faux_model(window), Effort::Medium)),
        factory: Arc::new(move |build: yi_runtime::ChildBuild<'_>| {
            let provider = Arc::new(ProviderStream::new(None));
            let queued = script.lock().map(|mut s| std::mem::take(&mut *s));
            provider.queue_faux(queued.unwrap_or_default());
            let Some(reader) = build.reader.clone() else {
                return Ok(AgentSession::new(
                    SessionConfig {
                        system_prompt: "worker sys".to_owned(),
                        model: build.model,
                        thinking_level: build.thinking,
                        tool_execution: yi_loop::ExecutionMode::Sequential,
                    },
                    provider,
                ));
            };
            if let Some(during) = &during {
                during();
            }
            let child = yi_runtime::subagent::reader::session(
                provider,
                build,
                &reader,
                yi_tools::builtin_tools(),
                (cwd.clone(), &home),
                broker.clone(),
                rules.clone(),
            );
            let names = child
                .tools()
                .iter()
                .map(|tool| tool.definition().name.as_str().to_owned())
                .collect();
            if let Ok(mut sink) = names_sink.lock() {
                sink.push(names);
            }
            if let Ok(mut sink) = turns_sink.lock() {
                sink.push(reader.turns);
            }
            if let Ok(mut sink) = shape_sink.lock() {
                sink.push((child.request_shape(), child.reuse()));
            }
            Ok(child)
        }),
        notice: Arc::new(move |text, _| {
            if let Ok(mut sink) = notice_sink.lock() {
                sink.push(text.to_owned());
            }
        }),
        events,
        report: Arc::new(|_, _| {}),
        parent_messages: Arc::new(Vec::new),
        attribute: Arc::new(|_| {}),
        store: Arc::new(|| None),
        plans_dir: workspace.join(".yi/plans"),
        family_live: yi_runtime::fetch::KernelServiceMap::new(),
    }));
    host.set_resolver(Arc::new(yi_runtime::fetch::Resolver::new(
        workspace,
        yi_runtime::Wall::default(),
    )));
    Ok(Family {
        root,
        host,
        children: tool_names,
        notices,
        shapes,
        turns,
    })
}

fn kwargs(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

fn reply(text: &str) -> AgentMessage {
    faux_assistant_message(vec![faux_text(text)], StopReason::Stop)
}

/// The child's whole transcript as the texts of its messages, read from its own file.
async fn transcript(family: &Family, name: &str) -> Result<Vec<(String, String)>, Box<dyn Error>> {
    for _ in 0..400 {
        let states = family.host.states();
        let settled = states
            .iter()
            .any(|view| view.name == name && matches!(view.state.as_str(), "finished" | "failed"));
        if settled {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
    let dir = family.root.join("family");
    let mut rows = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        for file in std::fs::read_dir(entry?.path())? {
            let text = std::fs::read_to_string(file?.path())?;
            for line in text.lines().filter(|line| line.contains("\"message\"")) {
                let value: Value = serde_json::from_str(line)?;
                let message: AgentMessage = serde_json::from_value(value["message"].clone())?;
                let (role, text) = match &message {
                    AgentMessage::User { content, .. } => ("user", user_text(content)),
                    AgentMessage::Assistant { content, .. } => ("assistant", content_text(content)),
                    AgentMessage::ToolResult { content, .. } => ("tool", content_text(content)),
                    _ => ("other", String::new()),
                };
                rows.push((role.to_owned(), text));
            }
        }
    }
    Ok(rows)
}

fn user_text(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Blocks(blocks) => content_text(blocks),
    }
}

fn content_text(blocks: &[Content]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn a_reader_brief_carries_its_partition_numbered_and_fenced() -> TestResult {
    let script: Script = Arc::new(Mutex::new(vec![reply("{\"answer\": \"line 2\"}")]));
    let family = family(4, script)?;
    family.host.spawn(
        "Which line names the sky?".to_owned(),
        kwargs(json!({"name": "q1", "role": "reader", "partition": ["local://notes.txt"]})),
    )?;
    let rows = transcript(&family, "q1").await?;
    let briefs: Vec<&String> = rows
        .iter()
        .filter(|(role, _)| role == "user")
        .map(|(_, text)| text)
        .collect();
    let (partition, question) = (
        briefs.first().ok_or("no partition")?,
        briefs.get(1).ok_or("no question")?,
    );
    assert!(
        partition.contains("source=\"local://notes.txt\" trust=\"untrusted\""),
        "{partition}"
    );
    assert!(partition.contains("2:blue sky"), "{partition}");
    assert!(question.starts_with("[task from parent]"), "{question}");
    assert!(
        question.trim_end().ends_with("Which line names the sky?"),
        "{question}"
    );
    let tools = family.children.lock().map_err(|_| "poisoned")?;
    let names = tools.first().ok_or("no reader built")?.clone();
    assert_eq!(names, vec!["read".to_owned(), "grep".to_owned()]);
    Ok(())
}

#[tokio::test]
async fn a_reader_answers_without_tools_at_its_turn_cap() -> TestResult {
    let mut args = Map::new();
    args.insert("path".to_owned(), Value::from("notes.txt"));
    let call = faux_assistant_message(
        vec![faux_tool_call("c1", "read", args)],
        StopReason::ToolUse,
    );
    let script: Script = Arc::new(Mutex::new(vec![call, reply("green sea is line 3")]));
    let family = family(4, script)?;
    family.host.spawn(
        "Which line names the sea?".to_owned(),
        kwargs(json!({"name": "q2", "role": "reader", "turns": 2})),
    )?;
    let rows = transcript(&family, "q2").await?;
    let assistants = rows.iter().filter(|(role, _)| role == "assistant").count();
    assert_eq!(assistants, 2, "{rows:?}");
    assert!(
        rows.iter()
            .any(|(role, text)| role == "user" && text.starts_with("[turns] No more tool calls")),
        "the capped turn is told its tools are off: {rows:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_reader_is_refused_what_a_reader_cannot_use() -> TestResult {
    let family = family(4, Arc::default())?;
    let refused = |args: Value| family.host.spawn("q".to_owned(), kwargs(args)).err();
    let fork = refused(json!({"role": "reader", "fork": "all"})).ok_or("fork admitted")?;
    assert!(fork.contains("not a fork"), "{fork}");
    let bare = refused(json!({"fork": "all"})).ok_or("a bare fork admitted as a reader")?;
    assert!(bare.contains("role=\"root\" spawns a full child"), "{bare}");
    let check = refused(json!({"check": "test -f notes.txt"})).ok_or("a bare check admitted")?;
    assert!(check.contains("runs no check; role=\"root\""), "{check}");
    let bash = refused(json!({"role": "reader", "tools": ["bash"]})).ok_or("bash admitted")?;
    assert_eq!(
        bash,
        "a reader may call read and grep, not bash; role=\"root\" spawns a full child"
    );
    let kernel = refused(json!({"role": "reader", "partition": ["kernel://main/x"]}))
        .ok_or("kernel partition admitted")?;
    assert!(kernel.contains("context_keys"), "{kernel}");
    let turns =
        refused(json!({"role": "root", "turns": 2})).ok_or("turns without a reader admitted")?;
    assert!(turns.contains("role=\"reader\""), "{turns}");
    Ok(())
}

#[tokio::test]
async fn readers_stand_outside_the_worker_cap() -> TestResult {
    let script: Script = Arc::new(Mutex::new(vec![reply("a"), reply("b"), reply("c")]));
    let family = family(1, script)?;
    family.host.spawn(
        "work".to_owned(),
        kwargs(json!({"name": "w", "role": "root"})),
    )?;
    for name in ["r1", "r2"] {
        family.host.spawn(
            "ask".to_owned(),
            kwargs(json!({"name": name, "role": "reader"})),
        )?;
    }
    let refused = family.host.spawn(
        "work".to_owned(),
        kwargs(json!({"name": "w2", "role": "root"})),
    );
    assert!(refused.is_err_and(|error| error.contains("child limit")));
    Ok(())
}

/// Dies with the control: count the family by its kernels alone and a parent holds readers
/// past the family cap, since a reader has no kernel; reaping one frees its seat.
#[tokio::test]
async fn held_readers_fill_the_family_cap() -> TestResult {
    let family = family(1, Arc::default())?;
    let cap = yi_runtime::levers::get().family_cap;
    for index in 0..cap {
        let name = format!("r{index}");
        family.host.spawn(
            "ask".to_owned(),
            kwargs(json!({"name": name, "role": "reader"})),
        )?;
    }
    let refused = family.host.spawn(
        "ask".to_owned(),
        kwargs(json!({"name": "over", "role": "reader"})),
    );
    let refusal = refused
        .err()
        .ok_or("a reader past the family cap was admitted")?;
    assert!(
        refusal.contains(&format!("the family holds {cap} live sessions")),
        "{refusal}"
    );
    family.host.delete("r0")?;
    let mut seated = Err(refusal);
    for _ in 0..100 {
        seated = family.host.spawn(
            "ask".to_owned(),
            kwargs(json!({"name": "over", "role": "reader"})),
        );
        if seated.is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    seated.map_err(|error| format!("the refusal's remedy freed no seat: {error}"))?;
    Ok(())
}

fn read_call(path: &str) -> AgentMessage {
    let mut args = Map::new();
    args.insert("path".to_owned(), Value::from(path));
    faux_assistant_message(
        vec![faux_tool_call("c1", "read", args)],
        StopReason::ToolUse,
    )
}

/// Sixty-four numbered lines of 1,023 bytes and their newlines fill the cap to the byte.
fn cap_filling(extra: usize) -> String {
    let mut text = String::new();
    for number in 1..=64 {
        let width = 1023 - format!("{number}:").len() + if number == 64 { extra } else { 0 };
        text.push_str(&"x".repeat(width));
        text.push('\n');
    }
    text
}

async fn first_brief(family: &Family, name: &str) -> Result<String, Box<dyn Error>> {
    transcript(family, name)
        .await?
        .into_iter()
        .find(|(role, _)| role == "user")
        .map(|(_, text)| text)
        .ok_or_else(|| "no brief".into())
}

#[tokio::test]
async fn a_partition_at_its_cap_is_whole_and_one_byte_over_is_cut_loudly() -> TestResult {
    let brief = |extra: usize| async move {
        let family = family(4, Arc::new(Mutex::new(vec![reply("a")])))?;
        std::fs::write(family.root.join("ws/big.txt"), cap_filling(extra))?;
        let partition = json!(["local://big.txt", "local://notes.txt"]);
        family.host.spawn(
            "Summarize.".to_owned(),
            kwargs(json!({"name": "q", "role": "reader", "partition": partition})),
        )?;
        first_brief(&family, "q").await
    };
    let whole = brief(0).await?;
    assert!(whole.contains("\n64:x"), "every line at the cap is kept");
    assert!(!whole.contains("of 64 lines"), "nothing is cut at the cap");
    assert!(
        whole.contains("[… kept 1 of 2 partition entries: partition cap 65536 bytes; entries 2 to 2 (from local://notes.txt) go in another reader's partition]"),
        "the entry past the cap is named"
    );
    let cut = brief(1).await?;
    assert!(
        !cut.contains("\n64:x"),
        "the line past the cap is dropped whole"
    );
    assert!(
        cut.contains("[… kept 63 of 64 lines of local://big.txt: partition cap 65536 bytes; lines 64-64: read path=big.txt offset=64]"),
        "the cut names kept, total, cap and the rest"
    );
    let (fences, closed) = (
        cut.matches("yi-external ").count(),
        cut.matches("end-yi-external").count(),
    );
    assert_eq!(fences, 2 * closed, "every fence the cut touched is closed");
    Ok(())
}

#[tokio::test]
async fn a_finished_reader_nobody_waits_on_tells_its_parent() -> TestResult {
    let script: Script = Arc::new(Mutex::new(vec![reply("line 2")]));
    let family = family(4, script)?;
    family.host.spawn(
        "Which line names the sky?".to_owned(),
        kwargs(json!({"name": "told", "role": "reader"})),
    )?;
    transcript(&family, "told").await?;
    for _ in 0..100 {
        if !family.notices.lock().map_err(|_| "poisoned")?.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let notices = family.notices.lock().map_err(|_| "poisoned")?.clone();
    assert!(
        notices
            .iter()
            .any(|text| text.contains("[subagent told") && text.contains("line 2")),
        "{notices:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_reader_is_gated_by_the_users_rules() -> TestResult {
    use yi_runtime::rules::{RuleDoc, RuleEngine, RuleGap, RuleMode, RuleScope};
    let rule = RuleDoc {
        name: "no-notes".to_owned(),
        body: "notes are private".to_owned(),
        path: std::path::PathBuf::from("/rules/no-notes.md"),
        needles: vec!["notes.txt".to_owned()],
        scope: RuleScope::Tool("read".to_owned()),
        gap: RuleGap::Once,
        mode: RuleMode::Gate,
        paths: Vec::new(),
        after: 1,
    };
    let script: Script = Arc::new(Mutex::new(vec![read_call("notes.txt"), reply("denied")]));
    let rules = Some(Arc::new(RuleEngine::new(vec![rule])));
    let family = family_with(
        4,
        script,
        Setup {
            rules,
            ..Setup::default()
        },
    )?;
    family.host.spawn(
        "Read the notes.".to_owned(),
        kwargs(json!({"name": "gated", "role": "reader"})),
    )?;
    let rows = transcript(&family, "gated").await?;
    let result = rows
        .iter()
        .find(|(role, _)| role == "tool")
        .ok_or("no tool result")?;
    assert!(result.1.contains("Denied by rule `no-notes`"), "{rows:?}");
    assert!(!result.1.contains("blue sky"), "{rows:?}");
    Ok(())
}

#[tokio::test]
async fn a_one_turn_reader_is_one_request_with_no_tools() -> TestResult {
    let script: Script = Arc::new(Mutex::new(vec![reply("x")]));
    let family = family(4, script)?;
    family.host.spawn(
        "q".to_owned(),
        kwargs(json!({"name": "one", "role": "reader", "turns": 1})),
    )?;
    let rows = transcript(&family, "one").await?;
    let tools = family.children.lock().map_err(|_| "poisoned")?.clone();
    assert_eq!(tools.first().map(Vec::len), Some(0), "{tools:?}");
    assert_eq!(
        rows.iter().filter(|(role, _)| role == "assistant").count(),
        1,
        "{rows:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_reader_being_built_holds_no_worker_slot() -> TestResult {
    let script: Script = Arc::new(Mutex::new(vec![reply("a"), reply("b")]));
    let host_cell: Arc<std::sync::OnceLock<std::sync::Weak<SubagentHost>>> = Arc::default();
    let (cell, admitted) = (Arc::clone(&host_cell), Arc::new(Mutex::new(None)));
    let seen = Arc::clone(&admitted);
    let during: Hook = Arc::new(move || {
        let Some(host) = cell.get().and_then(std::sync::Weak::upgrade) else {
            return;
        };
        let spawned = host.spawn(
            "work".to_owned(),
            kwargs(json!({"name": "w", "role": "root"})),
        );
        if let Ok(mut slot) = seen.lock() {
            slot.get_or_insert(spawned.map(|_| ()));
        }
    });
    let family = family_with(
        1,
        script,
        Setup {
            during: Some(during),
            ..Setup::default()
        },
    )?;
    let _ = host_cell.set(Arc::downgrade(&family.host));
    family.host.spawn(
        "ask".to_owned(),
        kwargs(json!({"name": "r", "role": "reader"})),
    )?;
    let outcome = admitted
        .lock()
        .map_err(|_| "poisoned")?
        .clone()
        .ok_or("hook never ran")?;
    assert_eq!(
        outcome,
        Ok(()),
        "a worker is admitted while a reader builds"
    );
    Ok(())
}

#[tokio::test]
async fn a_link_into_a_read_walled_tree_is_refused_as_a_partition() -> TestResult {
    let family = family(4, Arc::default())?;
    let ws = family.root.join("ws");
    std::fs::create_dir_all(ws.join("secrets"))?;
    std::fs::write(ws.join("secrets/key"), "hunter2\n")?;
    std::os::unix::fs::symlink(ws.join("secrets/key"), ws.join("link"))?;
    let refused = family.host.spawn(
        "q".to_owned(),
        kwargs(json!({"name": "l", "role": "reader", "deny_read": ["secrets"], "partition": ["local://link"]})),
    );
    let error = refused.err().ok_or("a linked secret was inlined")?;
    assert!(error.contains("deny_read"), "{error}");
    Ok(())
}

#[tokio::test]
async fn a_partition_of_many_empty_entries_stays_under_its_cap() -> TestResult {
    let family = family(4, Arc::new(Mutex::new(vec![reply("a")])))?;
    std::fs::write(family.root.join("ws/empty.txt"), "")?;
    let partition: Vec<String> = (0..2_000).map(|_| "local://empty.txt".to_owned()).collect();
    family.host.spawn(
        "q".to_owned(),
        kwargs(json!({"name": "q", "role": "reader", "partition": partition})),
    )?;
    let brief = first_brief(&family, "q").await?;
    assert!(brief.len() < 65_536 + 1_024, "{} bytes", brief.len());
    assert!(brief.contains("of 2000 partition entries: partition cap 65536 bytes"));
    Ok(())
}

#[tokio::test]
async fn a_readers_schema_and_a_shared_partition_shape_its_requests() -> TestResult {
    let script: Script = Arc::new(Mutex::new(vec![reply("{}"), reply("{}")]));
    let family = family(4, script)?;
    let schema = json!({"type": "object", "properties": {"line": {"type": "integer"}},
                        "required": ["line"], "additionalProperties": false});
    for name in ["s1", "s2"] {
        family.host.spawn(
            "Which line names the sky?".to_owned(),
            kwargs(
                json!({"name": name, "role": "reader", "partition": ["local://notes.txt"],
                          "schema": schema}),
            ),
        )?;
    }
    let shapes = family.shapes.lock().map_err(|_| "poisoned")?.clone();
    let shape = |shared_through| {
        let shape = yi_runtime::session::RequestShape {
            schema: Some(schema.clone()),
            shared_through,
        };
        (Some(shape), Reuse::Loop)
    };
    assert_eq!(
        shapes,
        vec![shape(None), shape(Some(0))],
        "the second sibling over the same partition marks it; both ask for the shape"
    );
    let rows = transcript(&family, "s1").await?;
    let asked = rows.iter().any(|(role, text)| {
        role == "user" && text.contains("Reply with one JSON object matching this schema")
    });
    assert!(asked, "{rows:?}");
    Ok(())
}

#[tokio::test]
async fn a_reader_without_tools_is_one_request() -> TestResult {
    let script: Script = Arc::new(Mutex::new(vec![reply("a"), reply("b")]));
    let family = family(4, script)?;
    for (name, tools) in [("bare", json!([])), ("reads", json!(["read"]))] {
        family.host.spawn(
            "Which line names the sky?".to_owned(),
            kwargs(json!({"name": name, "role": "reader", "tools": tools})),
        )?;
    }
    let shapes = family.shapes.lock().map_err(|_| "poisoned")?.clone();
    let reuses: Vec<Reuse> = shapes.iter().map(|(_, reuse)| *reuse).collect();
    assert_eq!(reuses, vec![Reuse::OneShot, Reuse::Loop]);
    Ok(())
}

#[tokio::test]
async fn a_refused_sibling_marks_no_partition_shared() -> TestResult {
    let script: Script = Arc::new(Mutex::new(vec![reply("a")]));
    let family = family(4, script)?;
    let ask = |name: &str| {
        family.host.spawn(
            "q".to_owned(),
            kwargs(json!({"name": name, "role": "reader", "partition": ["local://notes.txt"]})),
        )
    };
    assert!(ask("host").is_err(), "a reserved name is refused");
    ask("first")?;
    let shapes = family.shapes.lock().map_err(|_| "poisoned")?.clone();
    let marks: Vec<Option<usize>> = shapes
        .iter()
        .map(|(shape, _)| shape.as_ref().and_then(|shape| shape.shared_through))
        .collect();
    assert_eq!(marks, vec![None], "the refused spawn sent nothing to share");
    Ok(())
}

#[tokio::test]
async fn a_worker_writes_where_a_reader_is_walled_and_holds_a_worker_slot() -> TestResult {
    let mut args = Map::new();
    args.insert("path".to_owned(), Value::from("out.txt"));
    args.insert("content".to_owned(), Value::from("done\n"));
    let call = faux_assistant_message(
        vec![faux_tool_call("c1", "write", args)],
        StopReason::ToolUse,
    );
    let script: Script = Arc::new(Mutex::new(vec![call, reply("wrote out.txt")]));
    use yi_runtime::rules::{RuleDoc, RuleEngine, RuleGap, RuleMode, RuleScope};
    let rule = RuleDoc {
        name: "tell-owner".to_owned(),
        body: "tell the owner what you wrote".to_owned(),
        path: std::path::PathBuf::from("/rules/tell-owner.md"),
        needles: vec!["out.txt".to_owned()],
        scope: RuleScope::Tool("write".to_owned()),
        gap: RuleGap::Once,
        mode: RuleMode::Remind,
        paths: Vec::new(),
        after: 1,
    };
    let rules = Some(Arc::new(RuleEngine::new(vec![rule])));
    let family = family_with(
        1,
        script,
        Setup {
            rules,
            ..Setup::default()
        },
    )?;
    family.host.spawn(
        "Write done to out.txt".to_owned(),
        kwargs(json!({"name": "w", "role": "worker"})),
    )?;
    let refused = family.host.spawn(
        "work".to_owned(),
        kwargs(json!({"name": "w2", "role": "root"})),
    );
    assert!(
        refused.is_err_and(|error| error.contains("child limit")),
        "a worker stands inside the worker cap"
    );
    let rows = transcript(&family, "w").await?;
    let written = std::fs::read_to_string(family.root.join("ws").join("out.txt"));
    assert_eq!(written.ok().as_deref(), Some("done\n"), "{rows:?}");
    let reminded = std::fs::read_dir(family.root.join("family"))?
        .flatten()
        .flat_map(|dir| {
            std::fs::read_dir(dir.path())
                .into_iter()
                .flatten()
                .flatten()
        })
        .filter_map(|file| std::fs::read_to_string(file.path()).ok())
        .any(|text| text.contains("tell the owner what you wrote"));
    assert!(reminded, "the user's reminder reached the worker: {rows:?}");
    let tools = family.children.lock().map_err(|_| "poisoned")?;
    let names = tools.first().ok_or("no worker built")?.clone();
    assert_eq!(names, ["read", "edit", "write", "grep"]);
    let turns = family.turns.lock().map_err(|_| "poisoned")?.clone();
    assert_eq!(turns, [12], "a worker's default turn cap");
    Ok(())
}

#[tokio::test]
async fn a_worker_is_refused_a_fork_a_check_and_the_kernel() -> TestResult {
    let family = family(4, Arc::default())?;
    let refused = |args: Value| family.host.spawn("w".to_owned(), kwargs(args)).err();
    let fork = refused(json!({"role": "worker", "fork": "all"})).ok_or("fork admitted")?;
    assert_eq!(
        fork,
        "a worker gets a partition, not a fork; role=\"root\" spawns a full child"
    );
    let check =
        refused(json!({"role": "worker", "check": "true"})).ok_or("a worker check admitted")?;
    assert!(
        check.contains("a worker answers once and runs no check"),
        "{check}"
    );
    let kernel =
        refused(json!({"role": "worker", "tools": ["ipython"]})).ok_or("ipython admitted")?;
    assert_eq!(
        kernel,
        "a worker may call read, grep, edit, write, bash and get_context, not ipython; \
         role=\"root\" spawns a full child"
    );
    let turns = refused(json!({"role": "worker", "turns": 41})).ok_or("41 turns admitted")?;
    assert_eq!(turns, "rlm.run turns must be 1 to 40, got 41");
    Ok(())
}

fn write_call(id: &str, path: &str, content: &str) -> AgentMessage {
    let mut args = Map::new();
    args.insert("path".to_owned(), Value::from(path));
    args.insert("content".to_owned(), Value::from(content));
    faux_assistant_message(vec![faux_tool_call(id, "write", args)], StopReason::ToolUse)
}

#[tokio::test]
async fn a_worker_in_ask_mode_is_refused_the_write_nobody_approved() -> TestResult {
    let script: Script = Arc::new(Mutex::new(vec![
        write_call("c1", "out.txt", "done\n"),
        reply("could not write"),
    ]));
    let broker = Arc::new(yi_runtime::PermissionBroker::new(
        yi_permission::PermissionMode::Ask,
        std::env::temp_dir(),
        Vec::new(),
        None,
        tokio::sync::broadcast::channel(8).0,
    ));
    let family = family_with(
        1,
        script,
        Setup {
            broker: Some(broker),
            ..Setup::default()
        },
    )?;
    family.host.spawn(
        "Write done to out.txt".to_owned(),
        kwargs(json!({"name": "asked", "role": "worker"})),
    )?;
    let rows = transcript(&family, "asked").await?;
    assert!(
        !family.root.join("ws").join("out.txt").exists(),
        "an unapproved write landed: {rows:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_workers_reminder_fires_again_after_it_compacts() -> TestResult {
    use yi_runtime::rules::{RuleDoc, RuleEngine, RuleGap, RuleMode, RuleScope};
    let summary = faux_assistant_message(
        vec![faux_text("## Goal\nwrite out.txt three times")],
        StopReason::Stop,
    );
    // The first request's prompt latches the window's prefix; a zero usage no longer does.
    let mut first = write_call("c1", "out.txt", &"x".repeat(120_000));
    if let AgentMessage::Assistant { usage, .. } = &mut first {
        (usage.input, usage.total_tokens) = (1_000, 1_000);
    }
    let mut second = write_call("c2", "out.txt", &"y".repeat(120_000));
    if let AgentMessage::Assistant { usage, .. } = &mut second {
        (usage.input, usage.total_tokens) = (60_000, 60_000);
    }
    let script: Script = Arc::new(Mutex::new(vec![
        first,
        second,
        summary,
        write_call("c3", "out.txt", "three\n"),
        reply("wrote all three"),
    ]));
    let rule = RuleDoc {
        name: "tell-owner".to_owned(),
        body: "tell the owner what you wrote".to_owned(),
        path: std::path::PathBuf::from("/rules/tell-owner.md"),
        needles: vec!["out".to_owned()],
        scope: RuleScope::Tool("write".to_owned()),
        gap: RuleGap::Once,
        mode: RuleMode::Remind,
        paths: Vec::new(),
        after: 1,
    };
    let family = family_with(
        1,
        script,
        Setup {
            rules: Some(Arc::new(RuleEngine::new(vec![rule]))),
            window: 40_000,
            ..Setup::default()
        },
    )?;
    family.host.spawn(
        "Write out.txt three times".to_owned(),
        kwargs(json!({"name": "long", "role": "worker"})),
    )?;
    let rows = transcript(&family, "long").await?;
    let files: Vec<String> = std::fs::read_dir(family.root.join("family"))?
        .flatten()
        .flat_map(|dir| {
            std::fs::read_dir(dir.path())
                .into_iter()
                .flatten()
                .flatten()
        })
        .filter_map(|file| std::fs::read_to_string(file.path()).ok())
        .collect();
    let text = files.join("\n");
    assert!(
        text.contains("\"compaction\""),
        "the worker compacted: {rows:?}"
    );
    assert_eq!(
        text.matches("tell the owner what you wrote").count(),
        2,
        "the reminder fires once before the compaction and once after: {rows:?}"
    );
    Ok(())
}
