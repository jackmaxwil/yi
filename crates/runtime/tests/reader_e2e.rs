use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_runtime::{AgentSession, ProviderStream, SessionConfig, SubagentHost, SubagentHostOptions};
use yi_types::message::{AgentMessage, Content, StopReason, UserContent};
use yi_types::model::{Effort, Model, ModelCost};

type TestResult = Result<(), Box<dyn Error>>;
type Script = Arc<Mutex<Vec<AgentMessage>>>;

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

struct Family {
    root: Scratch,
    host: Arc<SubagentHost>,
    children: Arc<Mutex<Vec<Vec<String>>>>,
    shapes: Arc<Mutex<Vec<Shape>>>,
}

type Shape = Option<yi_runtime::session::RequestShape>;

/// A reader is built by the runtime's own `reader::session`; any other child is a plain one.
fn family(max_children: usize, script: Script) -> std::io::Result<Family> {
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
    let shapes: Arc<Mutex<Vec<Shape>>> = Arc::default();
    let shape_sink = Arc::clone(&shapes);
    let cwd = workspace.clone();
    let host = Arc::new(SubagentHost::new(SubagentHostOptions {
        depth: 0,
        max_depth: 1,
        max_children,
        parent_session_dir: root.join("family"),
        cwd: workspace.clone(),
        home: root.join("home"),
        lane_slots: 1,
        defaults: Arc::new(|| (faux_model(), Effort::Medium)),
        factory: Arc::new(move |build: yi_runtime::ChildBuild<'_>| {
            let provider = Arc::new(ProviderStream::new(None, None));
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
            let child = yi_runtime::subagent::reader::session(
                provider,
                build,
                &reader,
                yi_tools::builtin_tools(),
                cwd.clone(),
                None,
            );
            let names = child
                .tools()
                .iter()
                .map(|tool| tool.definition().name.as_str().to_owned())
                .collect();
            if let Ok(mut sink) = names_sink.lock() {
                sink.push(names);
            }
            if let Ok(mut sink) = shape_sink.lock() {
                sink.push(child.request_shape());
            }
            Ok(child)
        }),
        notice: Arc::new(|_, _| {}),
        events,
        report: Arc::new(|_, _| {}),
        parent_messages: Arc::new(Vec::new),
        attribute: Arc::new(|_| {}),
        store: Arc::new(|| None),
        plans_dir: workspace.join(".yi/plans"),
        family_live: Arc::new(|| 0),
    }));
    host.set_resolver(Arc::new(yi_runtime::fetch::Resolver::new(
        workspace,
        yi_runtime::Wall::default(),
    )));
    Ok(Family {
        root,
        host,
        children: tool_names,
        shapes,
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
            .any(|view| view.name == name && view.state.as_str() == "finished");
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
    let bash = refused(json!({"role": "reader", "tools": ["bash"]})).ok_or("bash admitted")?;
    assert_eq!(bash, "a reader may call read and grep, not bash");
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

#[tokio::test]
async fn held_readers_are_a_fuse_not_a_leak() -> TestResult {
    let family = family(1, Arc::default())?;
    let cap = yi_runtime::subagent::reader::HELD_CAP;
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
    assert!(refused.is_err_and(|error| error.contains("readers are held")));
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
        Some(yi_runtime::session::RequestShape {
            schema: Some(schema.clone()),
            shared_through,
            one_shot: false,
        })
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
    let script: Script = Arc::new(Mutex::new(vec![reply("line 2")]));
    let family = family(4, script)?;
    family.host.spawn(
        "Which line names the sky?".to_owned(),
        kwargs(json!({"name": "one", "role": "reader", "tools": []})),
    )?;
    let shapes = family.shapes.lock().map_err(|_| "poisoned")?.clone();
    let shape = shapes.first().cloned().flatten().ok_or("no reader built")?;
    assert!(shape.one_shot, "{shape:?}");
    Ok(())
}
