//! A child's lease is what its parent drew for it: the count it is billed is the fresh fold of
//! its replies, and the run ends at the reply that crosses the cap. Both are wire-visible on the
//! terminal `ChildUpdate`, so these tests read the update a parent's bus carries.

use crate::scratch;
use crate::support::{self, Family};
use scratch::Scratch;

use std::error::Error;

use serde_json::{Map, Value};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, StopReason};
use yi_types::subagent::{ChildActivity, ChildExit, ChildStatus, ChildUpdate, FailClass};

type TestResult = Result<(), Box<dyn Error>>;

/// One reply whose price is `fresh` new tokens on top of a `cached` prefix.
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
    family: Family,
    _root: Scratch,
}

fn rig(depth: u8, max_depth: u8, script: Vec<AgentMessage>) -> Result<Rig, Box<dyn Error>> {
    let root = Scratch::new("yi-lease")?;
    let store = support::memory_store("lease-test");
    let family = support::family_with_slots(
        root.to_path_buf(),
        std::env::temp_dir(),
        store,
        None,
        2,
        depth,
        max_depth,
    );
    family.queue(script)?;
    Ok(Rig {
        family,
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

async fn run_updates(
    rig: &Rig,
    tokens: Option<u64>,
) -> Result<(Vec<ChildUpdate>, Vec<String>), String> {
    let mut receiver = rig.family.events.subscribe();
    rig.family
        .host
        .spawn("sweep the logs".to_owned(), kwargs(tokens))
        .map_err(|error| error.to_string())?;
    let mut updates = Vec::new();
    let terminal = support::wait_until(12_000, || {
        while let Ok(event) = receiver.try_recv() {
            if let AgentEvent::ChildUpdate { update } = event {
                updates.push(update);
            }
        }
        updates
            .iter()
            .any(|update| update.status != ChildStatus::Running)
    })
    .await;
    if !terminal {
        return Err("no terminal child update rode the bus".to_owned());
    }
    let notices = rig
        .family
        .notices
        .lock()
        .map(|notices| notices.clone())
        .map_err(|_| "poisoned")?;
    Ok((updates, notices))
}

/// The parent's bus, drained to the child's terminal update: the last thing a parent can see.
async fn run(rig: &Rig, tokens: Option<u64>) -> Result<(ChildUpdate, Vec<String>), String> {
    let (updates, notices) = run_updates(rig, tokens).await?;
    let update = updates
        .last()
        .cloned()
        .ok_or("no child update rode the bus")?;
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

#[tokio::test]
async fn a_spent_lease_refuses_a_follow_up_instead_of_running_unpriced() -> TestResult {
    let rig = rig(
        0,
        1,
        vec![
            reply("first answer", StopReason::Stop, 1_200, 0),
            reply("unpriced answer", StopReason::Stop, 600, 0),
        ],
    )?;
    let (update, _) = run(&rig, Some(1_000)).await?;
    assert!(lease_spent(&update), "{:?}", update.exit);
    let sent = rig
        .family
        .host
        .route("parent", "sweeper", "what did you find?", true)
        .map_err(|error| error.to_string())?;
    assert_eq!(sent["receipts"][0]["state"], "inboxed", "{sent:?}");
    Ok(())
}

#[tokio::test]
async fn a_reply_moves_the_child_back_to_waiting_before_its_next_event() -> TestResult {
    let rig = rig(0, 1, turns(2, 100, 0))?;
    let (updates, _) = run_updates(&rig, None).await?;
    let billed = updates
        .iter()
        .find(|update| update.status == ChildStatus::Running && update.token_count > 0)
        .ok_or("no running update carried a billed reply")?;
    assert_eq!(billed.activity, ChildActivity::Waiting, "{updates:?}");
    Ok(())
}
