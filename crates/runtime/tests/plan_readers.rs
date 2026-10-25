//! A plan Reader delegation dispatched as a reader child, the delegations that stay root, and
//! the one line each plan child's ending leaves its owner.

use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use yi_ai::faux::{faux_assistant_message, faux_text};
use yi_runtime::fetch::{FetchLog, Resolver};
use yi_runtime::plan::dispatch::SessionDelegate;
use yi_runtime::plan::ops::{Actor, Delegate, PlanEngine};
use yi_runtime::plan::store::PlanStore;
use yi_runtime::plan::tool::PlanTool;
use yi_runtime::{AgentSession, ProviderStream, SessionConfig, SubagentHost, SubagentHostOptions};
use yi_tools::{Tool, ToolContext};
use yi_types::message::{AgentMessage, StopReason, UserContent};
use yi_types::model::Effort;
use yi_types::plan::contract::Resolution;
use yi_types::plan::doc::{Delegation, PlanId, TodoAddr, TodoLabel, TodoState};

type TestResult = Result<(), Box<dyn Error>>;
/// Per built child: its name, whether it was built a reader, and its system prompt.
type Built = Arc<Mutex<Vec<(String, bool, String)>>>;

const READER_PROMPT: &str = concat!(
    include_str!("../src/prompts/reader.md"),
    include_str!("../src/prompts/reader_fenced.md")
);

struct Rig {
    root: Scratch,
    host: Arc<SubagentHost>,
    built: Built,
    delivered: Arc<Mutex<Vec<String>>>,
}

fn reply(text: &str) -> AgentMessage {
    faux_assistant_message(vec![faux_text(text)], StopReason::Stop)
}

/// A reader is built by the runtime's own `reader::session`; a root child is a plain session.
fn rig(answer: &'static str) -> Result<Rig, Box<dyn Error>> {
    let root = crate::node::git_repo("yi-plan-readers")?;
    let ws = root.join("ws");
    std::fs::create_dir_all(&ws)?;
    std::fs::write(ws.join("notes.txt"), "red sun\nblue sky\ngreen sea\n")?;
    let built: Built = Arc::default();
    let sink = Arc::clone(&built);
    let (cwd, home) = (ws.clone(), root.join("home"));
    let host = Arc::new(SubagentHost::new(SubagentHostOptions {
        depth: 0,
        max_depth: 1,
        max_children: 4,
        parent_session_dir: root.join("family"),
        cwd: ws.clone(),
        home: home.clone(),
        lane_slots: 1,
        provider: Arc::new(ProviderStream::new(None)),
        defaults: Arc::new(|| (crate::support::faux_model(), Effort::Medium)),
        factory: Arc::new(move |build: yi_runtime::ChildBuild<'_>| {
            let provider = Arc::new(ProviderStream::new(None));
            provider.queue_faux(vec![reply(answer)]);
            let (name, reader) = (build.link.child_name.clone(), build.reader.is_some());
            let child = match build.reader.clone() {
                Some(reader) => yi_runtime::subagent::reader::session(
                    provider,
                    build,
                    &reader,
                    yi_tools::builtin_tools(),
                    (cwd.clone(), &home),
                    None,
                    None,
                ),
                None => AgentSession::new(
                    SessionConfig {
                        system_prompt: "root sys".to_owned(),
                        model: build.model,
                        thinking_level: build.thinking,
                        tool_execution: yi_loop::ExecutionMode::Sequential,
                    },
                    provider,
                ),
            };
            if let Ok(mut sink) = sink.lock() {
                sink.push((name, reader, child.system_prompt()));
            }
            Ok(child)
        }),
        notice: Arc::new(|_, _| {}),
        events: tokio::sync::broadcast::channel(64).0,
        report: Arc::new(|_, _| {}),
        parent_messages: Arc::new(Vec::new),
        attribute: Arc::new(|_| {}),
        store: Arc::new(|| None),
        plans_dir: root.join("plans"),
        family_live: yi_runtime::fetch::KernelServiceMap::new(),
    }));
    host.set_resolver(Arc::new(Resolver::new(ws, yi_runtime::Wall::default())));
    Ok(Rig {
        root,
        host,
        built,
        delivered: Arc::default(),
    })
}

fn delegate(rig: &Rig) -> SessionDelegate {
    let sink = Arc::clone(&rig.delivered);
    SessionDelegate::new(
        Arc::clone(&rig.host),
        Arc::new(move |message, _mode| {
            if let Ok(mut sink) = sink.lock() {
                sink.push(format!("{message:?}"));
            }
        }),
        Arc::new(FetchLog::new()),
    )
}

/// The user messages a child's own session file holds, in order.
fn user_texts(rig: &Rig) -> Result<Vec<String>, Box<dyn Error>> {
    let mut texts = Vec::new();
    for entry in std::fs::read_dir(rig.root.join("family"))? {
        for file in std::fs::read_dir(entry?.path())? {
            for line in std::fs::read_to_string(file?.path())?.lines() {
                let value: Value = serde_json::from_str(line)?;
                if let Ok(AgentMessage::User { content, .. }) =
                    serde_json::from_value(value["message"].clone())
                {
                    texts.push(match content {
                        UserContent::Text(text) => text,
                        UserContent::Blocks(_) => String::new(),
                    });
                }
            }
        }
    }
    Ok(texts)
}

fn at(label: &str) -> Result<TodoAddr, Box<dyn Error>> {
    Ok(TodoAddr {
        plan: PlanId::slug("review the notes")?,
        todo: TodoLabel::new(label)?,
    })
}

/// Dies with `kwargs_of` inserting `role="root"` for every delegation: the Reader is built a
/// root child, whose prompt is the root's and whose partition is a prose list of URLs. Dies
/// with the reap measuring the inlined partition: the parent is told its context went unread.
#[tokio::test]
async fn a_plan_reader_with_a_schema_contract_runs_as_a_reader_and_is_accepted() -> TestResult {
    let rig = rig(
        r#"{"answer": "line 3 names the sea", "quotes": [{"url": "local://notes.txt", "line": 3, "text": "green sea"}]}"#,
    )?;
    let store = PlanStore::open(rig.root.join("plans"))?;
    let resolver = Resolver::new(rig.root.join("ws"), yi_runtime::Wall::default())
        .with_plans_dir(rig.root.join("plans"));
    let engine = Arc::new(
        PlanEngine::new(store.clone(), Arc::new(delegate(&rig)))
            .with_output_resolve(Arc::new(resolver))
            .with_cwd(rig.root.join("ws")),
    );
    yi_runtime::plan::finish::install(&rig.host, &engine, Arc::new(|_message, _mode| {}));
    let id = PlanId::slug("review the notes")?;
    // The bytes Python's `contract.schema(ANSWER)` freezes: shapes.py's ANSWER, canonical.
    let answer = r#"{"properties":{"quotes":{"type":"array"}},"required":["answer","quotes"],"type":"object"}"#;
    let schema = store
        .artifacts(&id)
        .put(answer.as_bytes(), "application/json", &store.nonce())?;
    // roles.py `Reader(partition=..., note=...).delegation()` and review_pod's reader contract.
    let todo = json!({
        "label": "read-correctness",
        "delegation": {
            "spec": {"role": "reader", "wall": {"deny_write": ["."]}},
            "accept": {"stated": "the todo's contract decides"},
            "context": ["local://notes.txt"],
            "note": "Review for behavior that is wrong. Answer null when you find nothing; otherwise quote the line that shows it.",
        },
        "contract": {"class": "reader", "threshold": 1000, "min_coverage": 1000, "items": [
            {"id": "schema1", "critical": true, "weight": 1, "decider": {"schema": {"schema": schema}}}
        ]},
    });
    let tool = PlanTool::new(Arc::clone(&engine), Actor::Owner);
    let args = json!({"op": "init", "goal": "review the notes", "todos": [todo]});
    let output = tool.execute(
        args.as_object().cloned().ok_or("args")?,
        &ToolContext::new(rig.root.join("ws")),
    );
    assert!(!output.is_error, "{:?}", output.result.content);
    let label = TodoLabel::new("read-correctness")?;
    let mut state = None;
    for _ in 0..500 {
        state = store.read(&id)?.todo(&label).map(|todo| todo.state.clone());
        if state.as_ref().is_some_and(TodoState::is_terminal) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        matches!(
            state,
            Some(TodoState::Done {
                resolution: Some(Resolution::VerifiedDone),
                ..
            })
        ),
        "{state:?}"
    );
    let built = rig.built.lock().map_err(|_| "poisoned")?.clone();
    let (_, reader, prompt) = built.first().ok_or("no child built")?;
    assert!(*reader && prompt == READER_PROMPT, "{prompt}");
    let delivered = rig.delivered.lock().map_err(|_| "poisoned")?.clone();
    assert!(
        !delivered
            .iter()
            .any(|note| note.contains("context relevance")),
        "{delivered:?}"
    );
    let texts = user_texts(&rig)?;
    let partition = texts.first().ok_or("no first message")?;
    assert!(
        partition.contains("source=\"local://notes.txt\" trust=\"untrusted\"")
            && partition.contains("3:green sea"),
        "{partition}"
    );
    let question = texts.get(1).ok_or("no brief")?;
    assert!(
        !question.contains("Context:")
            && question
                .contains("Reply with one JSON object matching this schema:\n{\"properties\""),
        "{question}"
    );
    Ok(())
}

fn reader_delegation(accept: Value, context: &str) -> Result<Delegation, Box<dyn Error>> {
    Ok(serde_json::from_value(json!({
        "spec": {"role": "reader"}, "accept": accept, "context": [context]
    }))?)
}

/// Dies with routing by the role alone: the reader kwargs carry the accept command as a check,
/// and the host refuses the spawn because a reader answers once and runs no check.
#[tokio::test]
async fn a_reader_with_a_command_accept_stays_a_root_child() -> TestResult {
    let rig = rig("done")?;
    let delegation = reader_delegation(json!({"command": "true"}), "local://notes.txt")?;
    delegate(&rig).spawn(&at("read-and-check")?, &delegation)?;
    let built = rig.built.lock().map_err(|_| "poisoned")?.clone();
    assert!(matches!(built.first(), Some((_, false, _))), "{built:?}");
    Ok(())
}

/// Dies with the scheme clause dropped: a kernel value cannot be a reader's partition, so the
/// routed spawn fails where the root child it was would have spawned.
#[tokio::test]
async fn a_reader_given_a_kernel_value_stays_a_root_child() -> TestResult {
    let rig = rig("done")?;
    let delegation = reader_delegation(json!({"stated": "read it"}), "kernel://x")?;
    delegate(&rig).spawn(&at("read-the-value")?, &delegation)?;
    let built = rig.built.lock().map_err(|_| "poisoned")?.clone();
    assert!(matches!(built.first(), Some((_, false, _))), "{built:?}");
    Ok(())
}

/// Dies with the directory clause dropped: a reader's partition reads each entry as one file,
/// so `local://crates/auth` is refused at spawn where the root child would list and read it.
#[tokio::test]
async fn a_reader_given_a_directory_stays_a_root_child() -> TestResult {
    let rig = rig("done")?;
    let auth = rig.root.join("ws/crates/auth/src");
    std::fs::create_dir_all(&auth)?;
    std::fs::write(auth.join("lib.rs"), "pub fn login() {}\n")?;
    let stated = json!({"stated": "read it"});
    delegate(&rig).spawn(
        &at("read-auth")?,
        &reader_delegation(stated.clone(), "local://crates/auth")?,
    )?;
    let mut beside = reader_delegation(stated, "local://notes.txt")?;
    beside.context.push("local://crates".parse()?);
    delegate(&rig).spawn(&at("read-beside")?, &beside)?;
    let built = rig.built.lock().map_err(|_| "poisoned")?.clone();
    assert!(
        built.len() == 2 && built.iter().all(|(_, reader, _)| !reader),
        "{built:?}"
    );
    Ok(())
}

/// Dies with any one clause of the routing test dropped: each case differs from the routed
/// Reader in that one part, and a reader child would refuse the spawn the root child takes.
#[tokio::test]
async fn each_part_a_reader_cannot_take_keeps_a_root_child() -> TestResult {
    let rig = rig("done")?;
    let cases = [
        ("writer", "role", json!("writer")),
        ("worktree", "isolation", json!("worktree")),
        ("bash", "tools", json!(["read", "bash"])),
        (
            "family",
            "context",
            json!(["local://notes.txt", "family://main/notes"]),
        ),
    ];
    for (part, key, value) in cases {
        let mut wire = json!({"spec": {"role": "reader"}, "accept": "stated: read it",
            "context": ["local://notes.txt"]});
        match key {
            "context" => wire[key] = value,
            _ => wire["spec"][key] = value,
        }
        let delegation: Delegation = serde_json::from_value(wire)?;
        delegate(&rig).spawn(&at(&format!("read-{part}"))?, &delegation)?;
    }
    let built = rig.built.lock().map_err(|_| "poisoned")?.clone();
    assert!(
        built.len() == 4 && built.iter().all(|(_, reader, _)| !reader),
        "{built:?}"
    );
    Ok(())
}

/// A failed child's product as the incident's children left it: past the 2,000-char block the
/// reap once inlined, multi-byte throughout, the needle in its last sentence.
fn paragraph() -> String {
    let sentence = "Die Prüfung des Parsers schlug fehl: über die Grenze geschoben, naïve Bytes. ";
    let mut text = sentence.repeat(39);
    text.push_str("Zuletzt stand der Prüfstand beim Lexer.");
    text
}

fn hung_up(text: &str) -> AgentMessage {
    let mut message = faux_assistant_message(vec![faux_text(text)], StopReason::Error);
    if let AgentMessage::Assistant { error_message, .. } = &mut message {
        *error_message = Some("the provider hung up".to_owned());
    }
    message
}

/// A root wired as `yi` wires one, its kernel on the real HOME's venv.
fn wired(root: &Scratch) -> Result<(AgentSession, Arc<ProviderStream>), Box<dyn Error>> {
    let provider = Arc::new(ProviderStream::new(None));
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: crate::support::faux_model(),
            thinking_level: None,
            tool_execution: yi_loop::ExecutionMode::Sequential,
        },
        Arc::clone(&provider),
    );
    session.attach_store(crate::support::memory_store("plan-endings"))?;
    let home = std::env::var_os("HOME").ok_or("HOME is unset")?;
    yi_runtime::attach_runtime(
        &mut session,
        yi_runtime::RuntimeWiring {
            provider: Arc::clone(&provider),
            system_prompt: "sys".to_owned(),
            tool_execution: yi_loop::ExecutionMode::Sequential,
            cwd: root.to_path_buf(),
            home: home.into(),
            lane_slots: 1,
            broker: None,
            tools: Arc::new(yi_tools::builtin_tools),
            depth: 0,
            max_depth: 1,
            rlm_dir: root.join("rlm"),
            family_dir: None,
            summarizer: None,
            advisor: None,
            auto_review: None,
            plan_stale_turns: None,
            plans_dir: Some(root.join("plans")),
            parent_link: None,
            wall: yi_runtime::Wall::default(),
            auto_background: None,
            deadline: None,
            kernel_prewarm: false,
            mcp_read: None,
            sessions_dir: None,
            kernels: yi_runtime::fetch::KernelServiceMap::new(),
        },
    );
    Ok((session, provider))
}

/// One session tool call, as the model makes it; its text and whether it was refused.
async fn call(
    session: &AgentSession,
    name: &str,
    args: Value,
) -> Result<(String, bool), Box<dyn Error>> {
    let tool = session
        .tools()
        .into_iter()
        .find(|tool| tool.definition().name == name);
    let tool = tool.ok_or(format!("the root has no {name} tool"))?;
    let args = args.as_object().cloned().ok_or("args")?;
    let signal = yi_loop::interrupt::InterruptSignal::default();
    let out = tool.execute(name, args, &signal).await;
    Ok((
        yi_types::message::join_text(&out.result.content, "\n"),
        out.is_error,
    ))
}

/// The child of `label` runs on `reply`; what the owner was told when it ended, once idle.
async fn ended(
    session: &AgentSession,
    provider: &ProviderStream,
    op: Value,
    label: &str,
    reply: AgentMessage,
) -> Result<String, Box<dyn Error>> {
    provider.queue_faux(vec![reply, self::reply("noted")]);
    let (said, refused) = call(session, "plan", op).await?;
    if refused {
        return Err(said.into());
    }
    let named = format!("{label:?}");
    for _ in 0..500 {
        let line = session
            .messages()
            .into_iter()
            .find_map(|message| match message {
                AgentMessage::Custom {
                    custom_type,
                    content: UserContent::Text(text),
                    ..
                } if custom_type == "plan_relevance" && text.contains(&named) => Some(text),
                _ => None,
            });
        if let Some(line) = line {
            session.wait_idle().await;
            return Ok(line);
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    Err(format!("no ending line for {label}").into())
}

fn delegated(label: &str, check: &str) -> Value {
    let item = json!({"id": "tests", "critical": true, "weight": 1, "decider": {"cmd": check}});
    json!({"label": label, "delegation": {"spec": {"role": "worker"}, "accept": {"command": "true"}},
        "contract": {"class": "inline", "items": [item]}})
}

/// Dies with the reap block back on the verdict: the owner's line carries 2,000 chars of the
/// child's product, every later request pays for it, and no address says where the rest lives.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_plan_childs_ending_is_one_short_line_naming_its_transcript() -> TestResult {
    let root = Scratch::new("yi-plan-ending-failed")?;
    let (session, provider) = wired(&root)?;
    let op = json!({"op": "init", "goal": "land the patch", "todos": [delegated("write the patch", "true")]});
    let line = ended(
        &session,
        &provider,
        op,
        "write the patch",
        hung_up(&paragraph()),
    )
    .await?;
    let address = format!(
        "history://{}/write-the-patch",
        PlanId::slug("land the patch")?
    );
    assert!(
        !line.contains('\n') && line.len() < 300 && line.contains(&address),
        "{} bytes: {line}",
        line.len()
    );
    let (read, refused) = call(&session, "read", json!({"path": address})).await?;
    assert!(!refused && read.contains("Prüfstand beim Lexer"), "{read}");
    Ok(())
}

async fn fetched(session: &AgentSession, url: &str) -> Result<String, Box<dyn Error>> {
    use yi_tools::KernelBridge;
    let kernel = session
        .kernel_service()
        .ok_or("the wiring installs a kernel")?;
    let code = format!("print(await rlm.fetch({url:?}))");
    let cancelled: yi_tools::CancelFlag = Arc::new(|| false);
    let ran =
        tokio::task::spawn_blocking(move || kernel.execute_cell(&code, &cancelled, None)).await??;
    Ok(format!("{}{:?}", ran.result.stdout, ran.result.error))
}

/// Dies with any address form a finish line prints that `read`, `grep` or `rlm.fetch` cannot
/// open: the owner is told where the product is and the next call it makes is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_address_a_finish_line_prints_opens_in_read_grep_and_fetch() -> TestResult {
    let root = Scratch::new("yi-plan-ending-addresses")?;
    let (session, provider) = wired(&root)?;
    let init = json!({"op": "init", "goal": "ship the widget", "todos": [delegated("cut the seam", "echo 12 passed")]});
    let accepted = ended(
        &session,
        &provider,
        init,
        "cut the seam",
        reply("the cut holds at the parser seam"),
    )
    .await?;
    let append = json!({"op": "append", "todos": [delegated("write the patch", "true")]});
    let failed = ended(
        &session,
        &provider,
        append,
        "write the patch",
        hung_up(&paragraph()),
    )
    .await?;
    let append =
        json!({"op": "append", "todos": [delegated("check the seam", "echo broken; exit 1")]});
    let rejected = ended(
        &session,
        &provider,
        append,
        "check the seam",
        reply("the seam check found a gap"),
    )
    .await?;
    let mut schemes = Vec::new();
    let lines = [
        (&accepted, "parser seam"),
        (&failed, "beim Lexer"),
        (&rejected, "found a gap"),
    ];
    for (line, needle) in lines {
        let addresses = line
            .split([' ', '\n', '(', ')', ',', ';'])
            .filter(|word| word.contains("://"));
        for address in addresses {
            schemes.extend(
                address
                    .split_once("://")
                    .map(|(scheme, _)| scheme.to_owned()),
            );
            let (read, refused) = call(&session, "read", json!({"path": address})).await?;
            assert!(!refused && read.contains(needle), "read {address}: {read}");
            let (hits, refused) = call(
                &session,
                "grep",
                json!({"path": address, "pattern": needle}),
            )
            .await?;
            let at = hits.lines().find(|row| !row.starts_with('['));
            let at = at.and_then(|row| row.split(' ').next()).unwrap_or_default();
            let opens = match address.starts_with("plan://") {
                true => at.starts_with(address),
                false if at.starts_with("history://") => {
                    let (entry, _) = call(&session, "read", json!({"path": at})).await?;
                    entry.contains(needle)
                }
                false => false,
            };
            assert!(!refused && opens, "grep {address}: {hits}");
            let text = fetched(&session, address).await?;
            assert!(text.contains(needle), "rlm.fetch {address}: {text}");
        }
    }
    schemes.sort();
    assert_eq!(
        schemes,
        ["agent", "history", "history", "plan"],
        "{accepted}\n{failed}\n{rejected}"
    );
    if let Some(kernel) = session.kernel_service() {
        kernel.dispose().await;
    }
    Ok(())
}
