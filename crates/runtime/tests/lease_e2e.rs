//! A child's lease is what its parent drew for it: the count it is billed is the fresh fold of
//! its replies, and the run ends at the reply that crosses the cap. Both are wire-visible on the
//! terminal `ChildUpdate`, so these tests read the update a parent's bus carries.

use crate::scratch;
use crate::support::faux_model;
use scratch::Scratch;

use std::error::Error;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_loop::ExecutionMode;
use yi_runtime::{AgentSession, ProviderStream, SessionConfig, SubagentHost, SubagentHostOptions};
use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, StopReason};
use yi_types::subagent::{ChildExit, ChildStatus, ChildUpdate, FailClass};

type TestResult = Result<(), Box<dyn Error>>;

/// One reply whose price is `fresh` new tokens on top of a `cached` prefix: the prefix is what a
/// context gauge reads, the fresh part is what a lease is drawn in.
fn reply(text: &str, stop: StopReason, fresh: u64, cached: u64) -> AgentMessage {
    let content = match stop {
        StopReason::ToolUse => {
            let mut args = serde_json::Map::new();
            args.insert("command".to_owned(), Value::String(format!("echo {text}")));
            vec![faux_tool_call("call-lease", "bash", args)]
        }
        _ => vec![faux_text(text)],
    };
    let mut message = faux_assistant_message(content, stop);
    if let AgentMessage::Assistant { usage, .. } = &mut message {
        usage.input = i64::try_from(fresh).unwrap_or(0);
        usage.cache_read = i64::try_from(cached).unwrap_or(0);
        usage.total_tokens = i64::try_from(fresh.saturating_add(cached)).unwrap_or(0);
    }
    message
}

/// `turns` tool-use replies at `fresh` new tokens each over a `cached` prefix, then one answer.
fn turns(turns: usize, fresh: u64, cached: u64) -> Vec<AgentMessage> {
    let mut script = Vec::new();
    for turn in 0..turns {
        // A distinct command each turn: three identical calls in a row break the loop.
        script.push(reply(
            &format!("lease-{turn}"),
            StopReason::ToolUse,
            fresh,
            cached,
        ));
    }
    script.push(reply("swept the logs", StopReason::Stop, fresh, cached));
    script
}

struct Rig {
    host: Arc<SubagentHost>,
    events: tokio::sync::broadcast::Sender<AgentEvent>,
    notices: Arc<Mutex<Vec<String>>>,
    _root: Scratch,
}

fn rig(depth: u8, max_depth: u8, script: Vec<AgentMessage>) -> Result<Rig, Box<dyn Error>> {
    let root = Scratch::new("yi-lease")?;
    let notices = Arc::new(Mutex::new(Vec::new()));
    let notice_sink = Arc::clone(&notices);
    let (events, _keep) = tokio::sync::broadcast::channel(256);
    let provider: Arc<ProviderStream> = Arc::new(ProviderStream::new(None));
    let queued = Arc::new(Mutex::new(Some(script)));
    let store: yi_session::SharedSession = Arc::new(Mutex::new(
        yi_session::SessionStore::in_memory(yi_session::SessionMetadata {
            id: "lease-test".to_owned(),
            created_at: 0,
            parent_session_id: None,
            name: None,
        }),
    ));
    let home = root.to_path_buf();
    let host = Arc::new(SubagentHost::new(SubagentHostOptions {
        depth,
        max_depth,
        max_children: 4,
        parent_session_dir: root.to_path_buf(),
        defaults: Arc::new(|| (faux_model(), yi_types::model::Effort::Medium)),
        factory: Arc::new(move |build: yi_runtime::ChildBuild<'_>| {
            let provider = Arc::new(ProviderStream::new(None));
            let script = queued
                .lock()
                .ok()
                .and_then(|mut script| script.take())
                .unwrap_or_default();
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
            child.set_wall(build.wall.clone());
            child.use_tools(yi_tools::builtin_tools(), std::env::temp_dir(), None);
            Ok(child)
        }),
        provider,
        notice: Arc::new(move |text: &str, _| {
            if let Ok(mut sink) = notice_sink.lock() {
                sink.push(text.to_owned());
            }
        }),
        events: events.clone(),
        parent_messages: Arc::new(Vec::new),
        cwd: std::env::temp_dir(),
        home,
        lane_slots: 1,
        report: Arc::new(|_, _| {}),
        attribute: Arc::new(|_| {}),
        store: Arc::new(move || Some(store.clone())),
        plans_dir: root.join("plans"),
        family_live: yi_runtime::fetch::KernelServiceMap::new(),
    }));
    Ok(Rig {
        host,
        events,
        notices,
        _root: root,
    })
}

fn kwargs(tokens: Option<u64>) -> Map<String, Value> {
    let mut args: Map<String, Value> = [("role".to_owned(), Value::String("root".to_owned()))]
        .into_iter()
        .collect();
    args.insert("name".to_owned(), Value::String("sweeper".to_owned()));
    if let Some(tokens) = tokens {
        args.insert("tokens".to_owned(), Value::from(tokens));
    }
    args
}

const POLL_ATTEMPTS: usize = 400;
const POLL_INTERVAL_MS: u64 = 30;

/// The parent's bus, drained to the child's terminal update: the last thing a parent can see.
async fn run(rig: &Rig, tokens: Option<u64>) -> Result<(ChildUpdate, Vec<String>), String> {
    let mut receiver = rig.events.subscribe();
    rig.host
        .spawn("sweep the logs".to_owned(), kwargs(tokens))
        .map_err(|error| error.to_string())?;
    let mut last = None;
    for _ in 0..POLL_ATTEMPTS {
        let Ok(Ok(event)) = tokio::time::timeout(
            std::time::Duration::from_millis(POLL_INTERVAL_MS),
            receiver.recv(),
        )
        .await
        else {
            continue;
        };
        if let AgentEvent::ChildUpdate { update } = event {
            let terminal = update.status != ChildStatus::Running;
            last = Some(update);
            if terminal {
                break;
            }
        }
    }
    let update = last.ok_or("no child update rode the bus")?;
    let notices = rig
        .notices
        .lock()
        .map(|notices| notices.clone())
        .map_err(|_| "poisoned")?;
    Ok((update, notices))
}

fn lease_spent(update: &ChildUpdate) -> bool {
    matches!(
        update.exit,
        Some(ChildExit::Failed {
            class: FailClass::LeaseSpent
        })
    )
}

#[tokio::test]
async fn a_child_is_charged_for_its_fresh_tokens_not_its_cached_prefix() -> TestResult {
    let rig = rig(0, 1, turns(12, 100, 14_900))?;
    let (update, _) = run(&rig, None).await?;
    assert_eq!(
        update.token_count, 1_300,
        "thirteen replies of 100 fresh tokens on a 14.9k cached prefix bill 1300, not 195000"
    );
    assert_eq!(
        update.context_tokens,
        Some(15_000),
        "the gauge reads the last request's whole context"
    );
    assert_eq!(update.context_window, Some(128_000));
    Ok(())
}

#[tokio::test]
async fn a_child_is_stopped_after_the_reply_that_crosses_its_lease() -> TestResult {
    let rig = rig(0, 1, turns(5, 600, 0))?;
    let (update, notices) = run(&rig, Some(1_000)).await?;
    assert!(
        lease_spent(&update),
        "a lease of 1000 stops the run: {:?}",
        update.exit
    );
    assert_eq!(
        update.token_count, 1_200,
        "it stops at the second reply, not after five"
    );
    let notices = notices.join("\n");
    assert!(
        notices.contains("1000 token lease"),
        "the parent is told the cap: {notices}"
    );
    Ok(())
}

#[tokio::test]
async fn a_child_at_exactly_its_lease_runs_one_more_reply() -> TestResult {
    let at = rig(0, 1, turns(5, 600, 0))?;
    let (update, _) = run(&at, Some(1_200)).await?;
    assert!(
        lease_spent(&update),
        "the third reply crosses the cap: {:?}",
        update.exit
    );
    assert_eq!(
        update.token_count, 1_800,
        "a count equal to the cap is not over it, so the reply after it runs"
    );
    let under = rig(0, 1, turns(5, 600, 0))?;
    let (update, _) = run(&under, Some(1_199)).await?;
    assert!(lease_spent(&update), "{:?}", update.exit);
    assert_eq!(
        update.token_count, 1_200,
        "one token under the cap stops at the crossing reply"
    );
    Ok(())
}

#[tokio::test]
async fn a_grandchild_is_held_to_the_same_lease() -> TestResult {
    let rig = rig(1, 2, turns(5, 600, 0))?;
    let (update, _) = run(&rig, Some(1_000)).await?;
    assert!(
        lease_spent(&update),
        "every depth is capped: {:?}",
        update.exit
    );
    assert_eq!(update.token_count, 1_200);
    Ok(())
}
