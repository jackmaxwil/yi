//! Agents beyond the recordings, from Yi's own types: a child's trail, a grandchild, a wait.

use serde_json::{Value, json};
use yi_types::entry::Entry;
use yi_types::event::{AgentEvent, Wait};
use yi_types::subagent::{
    CHILD_ENTRY, ChildActivity, ChildEnded, ChildExit, ChildId, ChildSpawned, ChildStatus,
    ChildTrail, ChildUpdate,
};

use super::{Check, feed, played, recording, thread, update};
use crate::ids::{AgentId, Millis};

const THREAD: &str = "01a1de97-bf6c-7514-9fae-b57db1eaa7e1";

/// A line of the trail a thread keeps of its children, as the session entry that stores it.
fn trail_entry(id: &str, at: u64, trail: &ChildTrail) -> Result<Value, serde_json::Error> {
    serde_json::to_value(Entry::Custom {
        id: id.to_owned(),
        custom_type: CHILD_ENTRY.to_owned(),
        data: Some(serde_json::to_value(trail)?),
        parent_id: None,
        seq: 7,
        timestamp: at,
    })
}

#[test]
fn a_child_that_ended_before_this_app_looked_is_listed_from_the_threads_trail() -> Check {
    let (mut model, _) = played(&recording("turn.jsonl")?)?;
    let child = ChildId("a7".to_owned());
    let spawned = ChildTrail::Spawned(ChildSpawned {
        name: "Tests · upload".to_owned(),
        id: child.clone(),
        session: "01a1df00-0000-7000-8000-000000000007".to_owned(),
        path: "children/a7.jsonl".to_owned(),
        brief: "9f86d081".to_owned(),
        spawned_by: None,
        role: Some("worker".to_owned()),
        task: Some("Write tests for the new limiter.".to_owned()),
    });
    let ended = ChildTrail::Ended(ChildEnded {
        name: "Tests · upload".to_owned(),
        id: child,
        exit: ChildExit::Completed,
        error: None,
        tokens: 8400,
        elapsed_ms: Some(360_000),
        cost: serde_json::Number::from_f64(0.03),
        answer_preview: Some("Three tests written.".to_owned()),
    });
    let entries = [
        trail_entry("e-spawn", 100, &spawned)?,
        trail_entry("e-end", 460_100, &ended)?,
    ];
    // The entries past the six this thread already holds, as a resume would replay them.
    let replay = json!({"sessionUpdate": "_yi/replay", "entries": entries, "from": 6});
    feed(&mut model, 1, &update(THREAD, replay));

    let agents = &thread(&model, THREAD)?.agents;
    let [agent] = agents.as_slice() else {
        return Err(format!("{} agents from one child's trail", agents.len()).into());
    };
    assert_eq!(agent.id, AgentId::new("a7"));
    let said = &agent.update;
    assert_eq!(said.name, "Tests · upload");
    assert_eq!(
        said.task.as_deref(),
        Some("Write tests for the new limiter.")
    );
    assert_eq!(said.status, ChildStatus::Completed);
    assert_eq!((said.token_count, said.elapsed_ms), (8400, Some(360_000)));
    assert_eq!(said.answer_preview.as_deref(), Some("Three tests written."));
    assert_eq!(
        (agent.first_seen, agent.last_seen),
        (Millis(100), Millis(460_100)),
        "the trail's own times say when it ran"
    );
    Ok(())
}

/// One agent event in the envelope the recordings show; `child` is the agent it came from.
fn event(seq: u64, child: Option<&str>, event: &AgentEvent) -> Result<Value, serde_json::Error> {
    let mut fields = json!({"sessionUpdate": "_yi/event",
        "event": serde_json::to_value(event)?, "seq": seq});
    if let Some(child) = child {
        fields["childId"] = Value::from(child);
    }
    Ok(update(THREAD, fields))
}

fn reported(id: &str) -> ChildUpdate {
    ChildUpdate {
        id: ChildId(id.to_owned()),
        name: id.to_owned(),
        status: ChildStatus::Running,
        activity: ChildActivity::Writing,
        tool_use_count: 0,
        token_count: 0,
        cost: None,
        answer_preview: None,
        error: None,
        exit: None,
        flag: None,
        session: None,
        role: None,
        task: None,
        elapsed_ms: None,
    }
}

#[test]
fn a_child_reported_in_another_childs_stream_knows_its_parent() -> Check {
    let (mut model, _) = played(&recording("turn.jsonl")?)?;
    let own = AgentEvent::ChildUpdate {
        update: reported("a1"),
    };
    feed(&mut model, 1, &event(11, None, &own)?);
    // The server gives no parent id: a grandchild is known only by whose stream names it.
    let nested = AgentEvent::ChildUpdate {
        update: reported("a2"),
    };
    feed(&mut model, 2, &event(12, Some("a1"), &nested)?);
    let parents: Vec<(&str, Option<&str>)> = thread(&model, THREAD)?
        .agents
        .iter()
        .map(|agent| {
            (
                agent.id.as_str(),
                agent.parent.as_ref().map(AgentId::as_str),
            )
        })
        .collect();
    assert_eq!(parents, [("a1", None), ("a2", Some("a1"))]);
    Ok(())
}

#[test]
fn what_holds_the_agent_up_shows_until_it_is_over() -> Check {
    let (mut model, _) = played(&recording("turn.jsonl")?)?;
    let retry = Wait::Retry {
        attempt: 1,
        of: 3,
        delay_ms: 2_000,
        cause: "rate limited".to_owned(),
    };
    let held = AgentEvent::Wait {
        wait: Some(retry.clone()),
    };
    feed(&mut model, 1, &event(11, None, &held)?);
    assert_eq!(thread(&model, THREAD)?.transcript.wait, Some(retry));
    feed(
        &mut model,
        2,
        &event(12, None, &AgentEvent::Wait { wait: None })?,
    );
    assert_eq!(thread(&model, THREAD)?.transcript.wait, None);
    Ok(())
}
