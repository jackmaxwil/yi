//! Letters between root sessions: two real roots on one sessions directory, in two repositories,
//! each wired as the binary wires it, so a letter crosses the mail channel the way it would.
use crate::scratch;
use crate::support;
use scratch::Scratch;

use std::error::Error;
use std::path::Path;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use yi_ai::faux::{faux_assistant_message, faux_text};
use yi_loop::ExecutionMode;
use yi_runtime::fetch::KernelServiceMap;
use yi_runtime::{
    AgentSession, ProviderStream, RuntimeWiring, SessionConfig, SubagentHost, Wall, attach_runtime,
};
use yi_session::{SessionRepo, SharedSession};
use yi_types::message::{AgentMessage, StopReason, UserContent};

type TestResult = Result<(), Box<dyn Error>>;

struct Root {
    session: AgentSession,
    host: Arc<SubagentHost>,
    id: String,
}

/// A root as `build_session` leaves it: the runtime attached first, then its file store.
fn root(scratch: &Path, repo: &str, name: &str) -> Result<Root, Box<dyn Error>> {
    bind(scratch, repo, name, ledger(scratch, repo, name)?)
}

/// A session's file on disk, as one no process holds.
fn ledger(scratch: &Path, repo: &str, name: &str) -> Result<SharedSession, Box<dyn Error>> {
    let cwd = scratch.join(repo);
    std::fs::create_dir_all(&cwd)?;
    let mut repo = yi_session::JsonlRepo::new(scratch.join("sessions"), cwd.to_string_lossy());
    let store = repo.create(Default::default())?;
    yi_session::lock_session(&store).set_name(Some(name.to_owned()))?;
    Ok(store)
}

fn bind(
    scratch: &Path,
    repo: &str,
    name: &str,
    store: SharedSession,
) -> Result<Root, Box<dyn Error>> {
    let (sessions, cwd) = (scratch.join("sessions"), scratch.join(repo));
    let provider = Arc::new(ProviderStream::new(None));
    let noted = || faux_assistant_message(vec![faux_text("noted")], StopReason::Stop);
    provider.queue_faux((0..64).map(|_| noted()).collect());
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: String::new(),
            model: support::faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        Arc::clone(&provider),
    );
    let host = attach_runtime(
        &mut session,
        RuntimeWiring {
            provider,
            system_prompt: String::new(),
            tool_execution: ExecutionMode::Sequential,
            cwd: cwd.clone(),
            lane_slots: 1,
            deadline: None,
            home: scratch.to_path_buf(),
            broker: None,
            tools: Arc::new(yi_tools::builtin_tools),
            depth: 0,
            max_depth: 1,
            rlm_dir: scratch.join(format!("rlm-{name}")),
            family_dir: None,
            summarizer: None,
            advisor: None,
            auto_review: None,
            plan_stale_turns: None,
            plans_dir: Some(scratch.join(format!("plans-{name}"))),
            parent_link: None,
            wall: Wall::default(),
            auto_background: None,
            kernel_prewarm: false,
            mcp_read: None,
            sessions_dir: Some(sessions.clone()),
            kernels: KernelServiceMap::new(),
        },
    );
    let id = yi_session::lock_session(&store).metadata().id.clone();
    session.attach_store(store)?;
    Ok(Root { session, host, id })
}

fn letter(target: &str, body: &str, extra: &[(&str, &str)]) -> Map<String, Value> {
    let mut payload = json!({"target": target, "message": body, "kind": "request"});
    for (key, value) in extra {
        payload[*key] = json!(value);
    }
    payload.as_object().cloned().unwrap_or_default()
}

/// The letters a root's mail wakes carried, oldest first, as the envelope each line holds.
fn delivered(root: &Root) -> Vec<Value> {
    let lines = |text: &str| -> Vec<Value> {
        text.lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter_map(|line| line.get("data").cloned())
            .collect()
    };
    root.session
        .messages()
        .iter()
        .filter_map(|message| match message {
            AgentMessage::Custom {
                custom_type,
                content: UserContent::Text(text),
                ..
            } if custom_type == "heartbeat_prompt" => Some(lines(text)),
            _ => None,
        })
        .flatten()
        .collect()
}

async fn until(what: &str, mut ready: impl FnMut() -> bool) -> Result<(), Box<dyn Error>> {
    for _ in 0..600 {
        if ready() {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    Err(format!("timed out waiting for {what}").into())
}

/// Waits for `root`'s next letter and for the turn it woke to end, so the next post lands idle.
async fn next_letter(root: &Root, seen: usize) -> Result<Value, Box<dyn Error>> {
    until("a letter", || delivered(root).len() > seen).await?;
    until("the woken turn to end", || {
        root.session.status() != yi_runtime::Status::Running
    })
    .await?;
    delivered(root).get(seen).cloned().ok_or("no letter".into())
}

/// Dies with mail ending at the family, a letter arriving without its sender's return address
/// or the user's words it serves, or its reply not finding its way back.
#[tokio::test]
async fn two_roots_in_two_repositories_exchange_a_request_and_its_reply() -> TestResult {
    let scratch = Scratch::new("yi-session-mail")?;
    let (ant, bee) = (
        root(&scratch, "here", "ant")?,
        root(&scratch, "there", "bee")?,
    );
    let sent = ant.host.send(
        "parent",
        &letter("history://bee", "which runner image?", &[]),
    )?;
    let receipt = &sent["receipts"][0];
    assert_eq!(receipt["state"], "stored", "{sent:?}");
    assert_eq!(receipt["target"], format!("history://{}", bee.id));
    let got = next_letter(&bee, 0).await?;
    assert_eq!(got["from"], format!("history://{}", ant.id));
    assert_eq!(got["body"], "which runner image?");
    assert_eq!(got["hops"], 1);
    let todos = bee.session.todos().ok_or("no todo list")?.list();
    assert!(
        todos
            .items()
            .any(|item| item.label.to_string().starts_with("mail")),
        "the request made no todo"
    );
    let id = got["id"].as_str().ok_or("no id")?;
    let from = got["from"].as_str().ok_or("no from")?;
    let answer = letter(
        from,
        "v3, pinned by the gate",
        &[("kind", "reply"), ("reply_to", id)],
    );
    bee.host.send("parent", &answer)?;
    let back = next_letter(&ant, 0).await?;
    assert_eq!(back["body"], "v3, pinned by the gate");
    assert_eq!(
        (back["inReplyTo"].as_str(), back["hops"].as_u64()),
        (Some(id), Some(2))
    );
    Ok(())
}

/// Dies with two sessions that answer every letter with a new one never stopping: each letter
/// opens a fresh conversation, so only the count the woken turns carry can end it.
#[tokio::test]
async fn a_chain_of_letters_stops_at_the_hop_limit_and_says_why() -> TestResult {
    let scratch = Scratch::new("yi-session-hops")?;
    let pair = [
        root(&scratch, "here", "ping")?,
        root(&scratch, "there", "pong")?,
    ];
    let address = |root: &Root| format!("history://{}", root.id);
    let mut seen = [0_usize, 0];
    for hop in 1..=yi_runtime::post::HOP_LIMIT {
        let (from, to) = (usize::from(hop % 2 == 0), usize::from(hop % 2 == 1));
        let body = format!("letter {hop}");
        pair[from]
            .host
            .send("parent", &letter(&address(&pair[to]), &body, &[]))?;
        let got = next_letter(&pair[to], seen[to]).await?;
        seen[to] = seen[to].saturating_add(1);
        assert_eq!(
            (got["body"].as_str(), got["hops"].as_u64()),
            (Some(body.as_str()), Some(u64::from(hop)))
        );
    }
    let last = usize::from(yi_runtime::post::HOP_LIMIT % 2 == 1);
    let refused = pair[last].host.send(
        "parent",
        &letter(&address(&pair[1 - last]), "one more", &[]),
    );
    let error = refused.err().ok_or("the letter past the limit was sent")?;
    assert!(
        error.starts_with("hop limit:") && error.contains("mail.hopLimit = 20"),
        "{error}"
    );
    Ok(())
}

/// Dies with a child mailing another session around its own wall, a request call that would
/// wait forever on a reply that comes back as mail, and a letter past the channel cap kept.
#[tokio::test]
async fn a_child_a_request_call_and_an_oversized_letter_are_refused_by_name() -> TestResult {
    let scratch = Scratch::new("yi-session-refusals")?;
    let (ant, bee) = (
        root(&scratch, "here", "ant")?,
        root(&scratch, "there", "bee")?,
    );
    let target = format!("history://{}", bee.id);
    let child = ant
        .host
        .send("worker", &letter(&target, "please deploy", &[]));
    let error = child.err().ok_or("a child's letter left its session")?;
    assert!(
        error.contains("leaves its session only from the root"),
        "{error}"
    );
    let call = ant
        .host
        .request("parent", &target, "which image?", 1_000)
        .await;
    let error = call
        .err()
        .ok_or("a request call waited on another session")?;
    assert!(error.contains("answered by mail"), "{error}");
    let big = "x".repeat(16_300);
    let error = (ant.host.send("parent", &letter(&target, &big, &[])))
        .err()
        .ok_or("an oversized letter was sent")?;
    assert!(
        error.contains("as sent") && error.contains("16384"),
        "{error}"
    );
    let local = letter(&target, "see the report", &[("ref", "family://report")]);
    let error = ant
        .host
        .send("parent", &local)
        .err()
        .ok_or("a family ref was sent")?;
    assert!(error.contains("reader's own store"), "{error}");
    assert!(delivered(&bee).is_empty());
    Ok(())
}

/// Dies with an open todo holding the next letter back: two requests make two todos, each
/// stamped with its own letter.
#[tokio::test]
async fn an_open_todo_does_not_hold_the_next_request() -> TestResult {
    let scratch = Scratch::new("yi-session-overlap")?;
    let (ant, bee) = (
        root(&scratch, "here", "ant")?,
        root(&scratch, "there", "bee")?,
    );
    let target = format!("history://{}", bee.id);
    ant.host.send("parent", &letter(&target, "first", &[]))?;
    ant.host.send("parent", &letter(&target, "second", &[]))?;
    next_letter(&bee, 0).await?;
    next_letter(&bee, 1).await?;
    let list = bee.session.todos().ok_or("no todo list")?.list();
    let stamps: Vec<Value> = list
        .items()
        .filter(|item| !item.state.is_terminal())
        .filter_map(|item| item.extra.get("channel").cloned())
        .collect();
    assert_eq!(stamps.len(), 2, "{stamps:?}");
    assert_ne!(stamps[0]["ids"], stamps[1]["ids"]);
    Ok(())
}

/// Dies with an `inform` ringing the doorbell: it makes its todo and waits, and the request
/// that follows wakes the session, whose turn then reads both.
#[tokio::test]
async fn an_inform_makes_a_todo_and_waits_for_the_next_turn() -> TestResult {
    let scratch = Scratch::new("yi-session-inform")?;
    let (ant, bee) = (
        root(&scratch, "here", "ant")?,
        root(&scratch, "there", "bee")?,
    );
    let target = format!("history://{}", bee.id);
    let note = letter(&target, "the gate is green", &[("kind", "inform")]);
    ant.host.send("parent", &note)?;
    let mail = |root: &Root| {
        (root.session.todos()).map_or(0, |todos| {
            let list = todos.list();
            let count = list
                .items()
                .filter(|item| item.label.to_string().starts_with("mail"));
            count.count()
        })
    };
    until("the inform's todo", || mail(&bee) == 1).await?;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(delivered(&bee).is_empty(), "the inform woke a turn");
    ant.host
        .send("parent", &letter(&target, "please deploy", &[]))?;
    until("both letters", || delivered(&bee).len() == 2).await?;
    let mut bodies: Vec<Value> = delivered(&bee)
        .iter()
        .map(|got| got["body"].clone())
        .collect();
    bodies.sort_by_key(ToString::to_string);
    assert_eq!(bodies, [json!("please deploy"), json!("the gate is green")]);
    Ok(())
}

/// Dies with a letter lost because it came first: a session no process held when it was posted
/// reads it at its next start, and a `/heartbeat clear` there leaves its mailbox reading.
#[tokio::test]
async fn a_letter_waits_for_a_session_no_process_holds_and_outlives_a_heartbeat_clear() -> TestResult
{
    let scratch = Scratch::new("yi-session-dark")?;
    let ant = root(&scratch, "here", "ant")?;
    let dark = ledger(&scratch, "there", "bee")?;
    ant.host.send(
        "parent",
        &letter("history://bee", "while you were out", &[]),
    )?;
    let bee = bind(&scratch, "there", "bee", dark)?;
    let got = next_letter(&bee, 0).await?;
    assert_eq!(got["body"], "while you were out");
    let schedule = bee.session.heartbeat_service().ok_or("no schedule")?;
    schedule.run("clear")?;
    ant.host
        .send("parent", &letter("history://bee", "after the clear", &[]))?;
    assert_eq!(next_letter(&bee, 1).await?["body"], "after the clear");
    Ok(())
}

/// Dies with a session reading another's letters through a subscription to its mailbox, in
/// either case, since a case-insensitive disk opens both spellings as one file.
#[tokio::test]
async fn a_session_cannot_subscribe_to_another_sessions_mailbox() -> TestResult {
    let scratch = Scratch::new("yi-session-spy")?;
    let (ant, bee) = (
        root(&scratch, "here", "ant")?,
        root(&scratch, "there", "bee")?,
    );
    let mut registry = yi_runtime::HostRegistry::default();
    (bee.session.heartbeat_service().ok_or("no schedule")?).register(&mut registry);
    let upper = format!("channel://MAIL-{}", ant.id.to_ascii_uppercase());
    for address in [format!("channel://mail-{}", ant.id), upper] {
        let payload = json!({"address": address, "prompt": "watch"});
        let payload = payload.as_object().cloned().unwrap_or_default();
        let call =
            yi_kernel::client::HostHandlers::dispatch(&registry, "rlm_heartbeat.create", payload);
        let made = call.ok_or("rlm_heartbeat.create is not registered")?.await;
        let error = made
            .err()
            .ok_or("a session subscribed to another's mailbox")?;
        assert!(error.contains("another session's mailbox"), "{error}");
    }
    Ok(())
}

/// Dies with a mailbox no process reads growing without bound, or dropping a letter it held
/// without the sender hearing: the 256th letter is kept and the 257th is refused by name.
#[tokio::test]
async fn a_full_mailbox_refuses_the_next_letter_and_names_its_cap() -> TestResult {
    let scratch = Scratch::new("yi-session-full")?;
    let ant = root(&scratch, "here", "ant")?;
    let dark = ledger(&scratch, "there", "bee")?;
    let target = format!(
        "history://{}",
        yi_session::lock_session(&dark).metadata().id
    );
    for n in 1..=256 {
        let sent = ant
            .host
            .send("parent", &letter(&target, &format!("letter {n}"), &[]))?;
        assert_eq!(
            sent["receipts"][0]["state"], "stored",
            "letter {n}: {sent:?}"
        );
    }
    let error = (ant.host.send("parent", &letter(&target, "one more", &[])))
        .err()
        .ok_or("a letter past the mailbox cap was sent")?;
    assert!(
        error.contains("256 unread letters") && error.contains("KEPT = 256"),
        "{error}"
    );
    let bee = bind(&scratch, "there", "bee", dark)?;
    assert_eq!(next_letter(&bee, 0).await?["body"], "letter 1");
    Ok(())
}
