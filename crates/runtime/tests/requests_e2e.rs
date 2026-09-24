//! Stage M4 of the mailbox: every open request is answered or chased, a wait never blocks on
//! a family that cannot move without its caller, and a receipt says when it is presented.
#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;
#[path = "support/family.rs"]
mod support;

use std::error::Error;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_runtime::{AgentSession, ProviderStream, SessionConfig, SubagentHost, SubagentHostOptions};
use yi_types::message::{AgentMessage, StopReason};

type TestResult = Result<(), Box<dyn Error>>;

fn said(text: &str) -> AgentMessage {
    faux_assistant_message(vec![faux_text(text)], StopReason::Stop)
}

fn session(script: Vec<AgentMessage>) -> AgentSession {
    let provider = Arc::new(ProviderStream::new(None, None));
    provider.queue_faux(script);
    let config = SessionConfig {
        system_prompt: "sys".to_owned(),
        model: support::faux_model(),
        thinking_level: None,
        tool_execution: yi_loop::ExecutionMode::Sequential,
    };
    AgentSession::new(config, provider)
}

/// What a child does: speak its lines, ask its parent `ask_user` first, or hold a bash call.
#[derive(Clone, Copy)]
enum Child {
    Says(&'static [&'static str]),
    Asks,
    Holds(&'static str),
}

fn family(
    parent: &Arc<AgentSession>,
    kind: Child,
) -> std::io::Result<(Scratch, Arc<SubagentHost>)> {
    let root = Scratch::new("yi-requests")?;
    let store = support::memory_store("requests-parent");
    let host = Arc::new(SubagentHost::new(SubagentHostOptions {
        depth: 0,
        max_depth: 1,
        max_children: 8,
        parent_session_dir: root.to_path_buf(),
        cwd: root.to_path_buf(),
        home: root.join("home"),
        lane_slots: 1,
        defaults: Arc::new(|| (support::faux_model(), yi_types::model::Effort::Medium)),
        factory: Arc::new(move |build: yi_runtime::ChildBuild<'_>| {
            let mut child = match kind {
                Child::Says(lines) => session(lines.iter().map(|line| said(line)).collect()),
                Child::Asks => {
                    let question = json!({"question": "Which file name?", "options": ["notes.md", "hello.txt"]});
                    let question = question.as_object().cloned().unwrap_or_default();
                    let call = faux_tool_call("ask-1", "ask_user", question);
                    session(vec![
                        faux_assistant_message(vec![call], StopReason::ToolUse),
                        said("wrote the file the parent named"),
                    ])
                }
                Child::Holds(command) => {
                    let args = json!({"command": command})
                        .as_object()
                        .cloned()
                        .unwrap_or_default();
                    let call = faux_tool_call("hold-1", "bash", args);
                    let call = faux_assistant_message(vec![call], StopReason::ToolUse);
                    session(vec![
                        call,
                        said("held"),
                        said("read it"),
                        said("read it again"),
                    ])
                }
            };
            match kind {
                Child::Asks => {
                    let ask =
                        yi_runtime::auto_review::AskUserTool::new(None).asking(Some(build.link));
                    child.use_tools(vec![Arc::new(ask)], std::env::temp_dir(), None);
                }
                Child::Holds(_) => {
                    child.use_tools(yi_tools::builtin_tools(), std::env::temp_dir(), None)
                }
                Child::Says(_) => {}
            }
            Ok(child)
        }),
        notice: yi_runtime::wiring::lifecycle_notice(parent),
        events: parent.events_sender(),
        parent_messages: Arc::new(Vec::new),
        report: parent.deliver_hook(),
        attribute: Arc::new(|_usage| {}),
        store: Arc::new(move || Some(store.clone())),
        plans_dir: root.join(".yi/plans"),
        family_live: Arc::new(|| 0),
    }));
    Ok((root, host))
}

fn spawn(host: &Arc<SubagentHost>, name: &str) -> TestResult {
    let mut kwargs = Map::new();
    kwargs.insert("name".to_owned(), Value::String(name.to_owned()));
    host.spawn(format!("you are {name}"), kwargs)?;
    Ok(())
}

async fn reaches(host: &SubagentHost, state: &str) -> bool {
    for _ in 0..400 {
        if host.status()["members"][0]["state"] == state {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

/// Dies with a request answered in prose (`mbx-service`, 2 of 3 rounds): the service said "no
/// request has arrived yet", ended its turn, and the parent's `rlm.request` waited out 300 s.
#[tokio::test]
async fn a_request_left_open_is_chased_once_then_taken_from_the_final_text() -> TestResult {
    let parent = Arc::new(session((0..6).map(|_| said("noted")).collect()));
    let lines: &'static [&'static str] = &[
        "Running total is 0.",
        "No request has arrived yet.",
        "Still nothing to add.",
        "The total is 5.",
        "Nothing more.",
    ];
    let (_root, host) = family(&parent, Child::Says(lines))?;
    spawn(&host, "tally")?;
    assert!(reaches(&host, "finished").await, "the brief's turn ends");
    let started = Instant::now();
    let reply = host.request("parent", "tally", "5", 20_000).await?;
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(reply["envelope"]["answeredBy"], "final_text", "{reply:?}");
    let child = host.children_view().pop().ok_or("no child")?;
    let transcript = serde_json::to_string(&child.session.messages())?;
    assert!(
        transcript.contains("is still open and your turn ended without answering it"),
        "one steer came before the final text was taken: {transcript}"
    );
    Ok(())
}

/// Dies with an ask answered by plain sends (`mbx-ask` round 3): four `rlm.send('asker', ...)`
/// were queued as mail while the child waited out its question.
#[tokio::test]
async fn a_plain_send_to_a_child_asking_its_one_question_answers_it() -> TestResult {
    let parent = Arc::new(session((0..4).map(|_| said("noted")).collect()));
    let (_root, host) = family(&parent, Child::Asks)?;
    spawn(&host, "asker")?;
    assert!(reaches(&host, "needs_you").await, "the child asks");
    let send = json!({"target": "asker", "message": "Use the file name greeting.txt."});
    let sent = host.send("parent", send.as_object().ok_or("send")?)?;
    assert_eq!(sent["receipts"][0]["state"], "answered", "{sent:?}");
    assert!(
        reaches(&host, "finished").await,
        "the answered child finishes"
    );
    let child = host.children_view().pop().ok_or("no child")?;
    let transcript = serde_json::to_string(&child.session.messages())?;
    assert!(transcript.contains("greeting.txt"), "{transcript}");
    Ok(())
}

/// Dies with a wait blocked on a child asking its caller (`mbx-ask` round 3 waited 20, 60 and
/// 90 s each time), and with a wait on a family with nothing live (`mbx-fanout`, 540 s).
#[tokio::test]
async fn a_wait_returns_at_once_on_a_question_and_on_a_settled_family() -> TestResult {
    let parent = Arc::new(session((0..4).map(|_| said("noted")).collect()));
    let (_root, host) = family(&parent, Child::Asks)?;
    spawn(&host, "asker")?;
    assert!(reaches(&host, "needs_you").await, "the child asks");
    let seen = host.wait(1_000, None).await;
    let cursor = seen["cursor"].as_u64();
    let started = Instant::now();
    let asks = host.wait(60_000, cursor).await;
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        (&asks["state"], &asks["causes"]["asker"]),
        (&json!("asks"), &json!("asks"))
    );
    let answer = json!({"target": "asker", "message": "notes.md"});
    host.send("parent", answer.as_object().ok_or("answer")?)?;
    assert!(
        reaches(&host, "finished").await,
        "the answered child finishes"
    );
    let cursor = host.wait(1_000, None).await["cursor"].as_u64();
    let started = Instant::now();
    let settled = host.wait(60_000, cursor).await;
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(settled["state"], "settled", "{settled:?}");
    assert_eq!(settled["finished"], json!(["asker"]));
    Ok(())
}

/// Dies with a receipt that says `queued` for every send to a busy child (`mbx-steer`) and
/// nothing of when the message reaches the child's model.
#[tokio::test]
async fn a_receipt_says_when_its_message_is_presented() -> TestResult {
    let parent = Arc::new(session((0..4).map(|_| said("noted")).collect()));
    let (_root, host) = family(&parent, Child::Holds("sleep 2"))?;
    spawn(&host, "busy")?;
    assert!(reaches(&host, "running").await, "the child holds its turn");
    let busy = json!({"target": "busy", "message": "Include the word BANANA."});
    let busy = host.send("parent", busy.as_object().ok_or("busy")?)?;
    let row = &busy["receipts"][0];
    assert_eq!(row["state"], "queued", "{busy:?}");
    let presented = row["presented"].as_str().unwrap_or_default();
    assert!(presented.contains("next message boundary"), "{presented}");
    assert!(reaches(&host, "finished").await, "the child finishes");
    let idle = json!({"target": "busy", "message": "noted"});
    let idle = host.send("parent", idle.as_object().ok_or("idle")?)?;
    let presented = idle["receipts"][0]["presented"]
        .as_str()
        .unwrap_or_default();
    assert!(presented.contains("wakes nothing"), "{idle:?}");
    Ok(())
}
