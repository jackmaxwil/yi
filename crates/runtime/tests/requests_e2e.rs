//! Stage M4 of the mailbox: every open request is answered or chased, a wait never blocks on
//! a family that cannot move without its caller, and a receipt says when it is presented.
use crate::scratch;
use crate::support;
use scratch::Scratch;

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
    let provider = Arc::new(ProviderStream::new(None));
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

type Family = (Scratch, Arc<SubagentHost>, yi_session::SharedSession);

fn family(parent: &Arc<AgentSession>, kind: Child) -> std::io::Result<Family> {
    let root = Scratch::new("yi-requests")?;
    let store = support::memory_store("requests-parent");
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
    Ok((root, host, kept))
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
    let (_root, host, _store) = family(&parent, Child::Says(lines))?;
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
    assert_eq!(
        reply["envelope"]["body"], "Still nothing to add.",
        "{reply:?}"
    );
    let child = host.children_view().pop().ok_or("no child")?;
    let steers = child
        .session
        .messages()
        .iter()
        .filter(|message| {
            message
                .plain_text()
                .contains("is still open and your turn ended")
        })
        .count();
    assert_eq!(steers, 1, "one steer came before the final text was taken");
    Ok(())
}

/// Dies with a final text over the body cap refused and dropped: the requester waited out its
/// timeout. The whole text is kept on the blackboard and its head travels with the ref.
#[tokio::test]
async fn a_final_text_over_the_body_cap_is_kept_whole_and_sent_by_ref() -> TestResult {
    let parent = Arc::new(session((0..6).map(|_| said("noted")).collect()));
    let long: &'static str = Box::leak("tally ".repeat(4_000).into_boxed_str());
    let lines: &'static [&'static str] =
        Box::leak(Box::new(["Ready.", "Nothing yet.", long, "Done."]));
    let (root, host, _store) = family(&parent, Child::Says(lines))?;
    spawn(&host, "tally")?;
    assert!(reaches(&host, "finished").await, "the brief's turn ends");
    let started = Instant::now();
    let reply = host
        .request("parent", "tally", "the total?", 20_000)
        .await?;
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    let envelope = &reply["envelope"];
    let id = envelope["inReplyTo"].as_str().ok_or("no id")?;
    assert_eq!(
        envelope["ref"],
        format!("family://reply-{id}"),
        "{envelope:?}"
    );
    let body = envelope["body"].as_str().unwrap_or_default();
    assert!(
        body.starts_with("tally tally") && body.contains("24000 bytes in all"),
        "{body}"
    );
    let kept = std::fs::read_to_string(root.join(format!("family/reply-{id}.json")))?;
    let kept: Value = serde_json::from_str(&kept)?;
    assert_eq!(kept["text"], long);
    Ok(())
}

/// Dies with an ask answered by plain sends (`mbx-ask` round 3): four `rlm.send('asker', ...)`
/// were queued as mail while the child waited out its question. Dies too with a send meant
/// as information taken as the answer to a question its sender was never shown.
#[tokio::test]
async fn a_plain_send_answers_a_question_only_once_its_sender_was_shown_it() -> TestResult {
    let parent = Arc::new(session((0..4).map(|_| said("noted")).collect()));
    let (_root, host, store) = family(&parent, Child::Asks)?;
    spawn(&host, "asker")?;
    assert!(reaches(&host, "needs_you").await, "the child asks");
    let (id, ..) = host.open_requests().pop().ok_or("no question")?;
    let inform = json!({"target": "asker", "message": "Also write tests."});
    let sent = host.send("parent", inform.as_object().ok_or("inform")?)?;
    let row = &sent["receipts"][0];
    assert_ne!(row["state"], "answered", "{sent:?}");
    let hint = row["hint"].as_str().unwrap_or_default();
    assert!(hint.contains(&format!("reply_to=\"{id}\"")), "{sent:?}");
    let shown = AgentMessage::Custom {
        custom_type: "agent_message".to_owned(),
        content: yi_types::message::UserContent::Text("Which file name?".to_owned()),
        display: true,
        details: Some(json!({"id": id})),
        timestamp: 0,
    };
    yi_session::lock_session(&store).append_message("main", shown)?;
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

/// Dies with only a presented message counting as shown: a cell that read the question off
/// `rlm.wait` and answered it plainly got a hint, and the child waited out its question.
#[tokio::test]
async fn a_plain_send_answers_a_question_its_sender_read_off_a_wait() -> TestResult {
    let parent = Arc::new(session((0..4).map(|_| said("noted")).collect()));
    let (_root, host, _store) = family(&parent, Child::Asks)?;
    spawn(&host, "asker")?;
    assert!(reaches(&host, "needs_you").await, "the child asks");
    let asks = host.wait(60_000, None).await?;
    assert_eq!(asks["state"], "asks", "{asks:?}");
    let send = json!({"target": "asker", "message": "Use the file name greeting.txt."});
    let sent = host.send("parent", send.as_object().ok_or("send")?)?;
    assert_eq!(sent["receipts"][0]["state"], "answered", "{sent:?}");
    Ok(())
}

/// Dies with a reply the model never read counting as shown: `h.result`'s own wait, or a
/// `status` naming one child, let a plain send answer another child's question.
#[tokio::test]
async fn only_a_question_a_read_reply_quoted_counts_as_shown() -> TestResult {
    use yi_kernel::client::HostHandlers;
    let parent = Arc::new(session((0..4).map(|_| said("noted")).collect()));
    let (_root, host, _store) = family(&parent, Child::Asks)?;
    let mut registry = yi_runtime::HostRegistry::default();
    host.register(&mut registry);
    for name in ["a", "b"] {
        spawn(&host, name)?;
    }
    for _ in 0..400 {
        if host.open_requests().len() < 2 {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
    let call = |kind: &str, payload: Value| {
        let payload = payload.as_object().cloned().unwrap_or_default();
        registry.dispatch(kind, payload).ok_or("no handler")
    };
    let plain = |to: &str| -> Result<Value, Box<dyn Error>> {
        let mail = json!({"target": to, "message": "Also write tests."});
        Ok(host.send("parent", mail.as_object().ok_or("mail")?)?["receipts"][0].clone())
    };
    let helper = json!({"timeout_ms": 1_000, "cursor": 0, "quiet": true});
    call("rlm.wait", helper)?.await?;
    assert_ne!(
        plain("b")?["state"],
        "answered",
        "a helper's wait showed it"
    );
    call("rlm.status", json!({"name": "a"}))?.await?;
    assert_ne!(
        plain("b")?["state"],
        "answered",
        "a's entry showed b's question"
    );
    call("rlm.status", json!({"name": "b"}))?.await?;
    assert_eq!(plain("b")?["state"], "answered");
    Ok(())
}

/// Dies with progress write-only: a child's `progress` was inboxed and never read. Dies too
/// with it waking an idle parent, or with each step presented instead of the latest.
#[tokio::test]
async fn progress_reaches_an_idle_parent_at_its_next_turn_coalesced() -> TestResult {
    let parent = Arc::new(session(vec![said("noted")]));
    let (_root, host, _store) = family(&parent, Child::Says(&["done"]))?;
    for step in ["25% of the rows", "75% of the rows"] {
        let progress = json!({"target": "parent", "message": step, "kind": "progress"});
        host.send("scout", progress.as_object().ok_or("progress")?)?;
    }
    assert_eq!(
        parent.status(),
        yi_runtime::Status::Idle,
        "progress woke it"
    );
    parent.prompt("go")?;
    parent.wait_idle().await;
    let seen = serde_json::to_string(&parent.messages())?;
    assert!(
        seen.contains("75% of") && !seen.contains("25% of"),
        "{seen}"
    );
    Ok(())
}

/// Dies with plan children answering only to `<plan>/<todo>` (four corpus sends to "billing"
/// failed). An ambiguous label is refused naming every child it could mean.
#[tokio::test]
async fn a_todo_label_alone_names_a_plan_child_unless_it_is_ambiguous() -> TestResult {
    let parent = Arc::new(session(Vec::new()));
    let (_root, host, _store) = family(&parent, Child::Says(&["done"]))?;
    for name in ["audit/ledger", "audit/billing", "fix/billing"] {
        spawn(&host, name)?;
    }
    let send = |target: &str| {
        let mail = json!({"target": target, "message": "the totals are in"});
        host.send("parent", &mail.as_object().cloned().unwrap_or_default())
    };
    let sent = send("ledger")?;
    assert_eq!(sent["receipts"][0]["target"], "audit/ledger", "{sent:?}");
    let refused = send("billing").err().unwrap_or_default();
    assert!(refused.contains("audit/billing, fix/billing"), "{refused}");
    Ok(())
}

/// Dies with a derived `stuck` moving no epoch: a cell blocked in `rlm.wait` slept through a
/// child the repeat breaker had stopped.
#[tokio::test]
async fn a_child_going_stuck_wakes_a_wait() -> TestResult {
    let parent = Arc::new(session(Vec::new()));
    let (_root, host, _store) = family(&parent, Child::Holds("sleep 5"))?;
    spawn(&host, "looper")?;
    let mut cursor = host.wait(1_000, None).await?["cursor"].as_u64();
    let store = host.transcript("looper").ok_or("no transcript")?;
    let signal = json!({"signal": "repeat_break"});
    yi_session::lock_session(&store).append_custom("main", "loop_signal", Some(signal))?;
    let mut causes = Vec::new();
    for _ in 0..3 {
        let reply = host.wait(1_000, cursor).await?;
        cursor = reply["cursor"].as_u64();
        causes.push(reply["causes"]["looper"].clone());
    }
    assert!(causes.contains(&json!("stuck")), "{causes:?}");
    Ok(())
}

/// Dies with a wait blocked on a child asking its caller (`mbx-ask` round 3 waited 20, 60 and
/// 90 s each time), and with a wait on a family with nothing live (`mbx-fanout`, 540 s). Dies
/// too with a repeat of either spinning: the second wait at the same state refuses, naming it.
#[tokio::test]
async fn a_wait_returns_at_once_on_a_question_and_on_a_settled_family() -> TestResult {
    let parent = Arc::new(session((0..4).map(|_| said("noted")).collect()));
    let (_root, host, _store) = family(&parent, Child::Asks)?;
    spawn(&host, "asker")?;
    assert!(reaches(&host, "needs_you").await, "the child asks");
    let started = Instant::now();
    let asks = host.wait(60_000, None).await?;
    assert_eq!(
        (&asks["state"], &asks["causes"]["asker"]),
        (&json!("asks"), &json!("asks"))
    );
    let again = host.wait(60_000, asks["cursor"].as_u64()).await;
    let refused = again.err().unwrap_or_default();
    assert!(
        refused.starts_with("asker is asking you") && refused.contains("reply_to="),
        "{refused}"
    );
    let (id, ..) = host.open_requests().pop().ok_or("no question")?;
    let answer = json!({"target": "asker", "message": "notes.md", "reply_to": id});
    host.send("parent", answer.as_object().ok_or("answer")?)?;
    assert!(
        reaches(&host, "finished").await,
        "the answered child finishes"
    );
    let settled = host.wait(60_000, None).await?;
    assert_eq!(settled["state"], "settled", "{settled:?}");
    assert_eq!(settled["finished"], json!(["asker"]));
    let again = host.wait(60_000, settled["cursor"].as_u64()).await;
    assert_eq!(
        again.err().as_deref(),
        Some("the family is settled: nothing is running; stop waiting")
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    Ok(())
}

/// Dies with a receipt that says `queued` for every send to a busy child (`mbx-steer`) and
/// nothing of when the message reaches the child's model.
#[tokio::test]
async fn a_receipt_says_when_its_message_is_presented() -> TestResult {
    let parent = Arc::new(session((0..4).map(|_| said("noted")).collect()));
    let (_root, host, _store) = family(&parent, Child::Holds("sleep 2"))?;
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
