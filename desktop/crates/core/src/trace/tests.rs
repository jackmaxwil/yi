use std::error::Error;

use yi_types::event::AgentEvent;
use yi_types::subagent::ChildUpdate;

use super::*;
use crate::feed::{FeedEvent, Table};
use crate::ids::AgentId;
use crate::model::Model;
use crate::msg::{Msg, step};
use crate::reduce::threads::Thread;
use crate::row::rows_from_json;
use crate::wire::replay::play_line;

/// A real server's turn that runs `bash`, asks to, is allowed, and then answers.
const ALLOWED: &str = include_str!("../../../../fixtures/wire/approval-allow.jsonl");
const MOCKUP: &str = include_str!("../../../../fixtures/mockup/trace.rows.json");
/// Recordings carry no times; line `n` is given this many milliseconds after the first.
const T0: u64 = 1_794_679_000_000;

fn at(line: usize) -> Millis {
    Millis(T0 + u64::try_from(line).unwrap_or(0) * 100)
}

/// Plays the first `lines` lines of the recording, each at its own time.
fn play(lines: usize) -> Result<Model, Box<dyn Error>> {
    let mut model = Model::default();
    for (index, line) in ALLOWED.lines().take(lines).enumerate() {
        step(&mut model, Msg::Tick(at(index)));
        play_line(&mut model, line)?;
    }
    Ok(model)
}

fn thread(model: &Model) -> Result<&Thread, Box<dyn Error>> {
    model.wire.threads.values().next().ok_or("no thread".into())
}

fn table(rows: Vec<Row>) -> Table {
    let mut table = Table::default();
    table.apply(FeedEvent::Reset(rows));
    table
}

fn cells<'a>(rows: &'a [Row], field: &str) -> Vec<&'a str> {
    rows.iter().filter_map(|row| row.words(field)).collect()
}

#[test]
fn a_recorded_turn_is_a_model_call_a_tool_with_its_ask_and_a_second_model_call()
-> Result<(), Box<dyn Error>> {
    let model = play(51)?;
    let trace = Trace::of(thread(&model)?, None, at(60)).ok_or("no turn")?;
    let rows = trace.rows();
    assert_eq!(cells(&rows, "start"), ["0", "+0.5", "+1.3", "+2.9"]);
    // The server said the tool ran 73 ms; its two stamps were 800 ms apart.
    assert_eq!(cells(&rows, "took"), ["3.4 s", "500 ms", "73 ms", "500 ms"]);
    let names = cells(&rows, "step");
    assert_eq!(names.get(2), Some(&"bash echo one > one.txt"));
    assert_eq!(
        names.iter().filter(|n| n.starts_with("bash echo")).count(),
        1,
        "one command, one span: {names:?}"
    );
    let tool = rows.get(2).ok_or("no tool")?;
    assert_eq!(tool.words("asked"), Some("17:56:42"));
    assert!(
        tool.words("waited").is_some(),
        "the wait is a fact of the span"
    );
    Ok(())
}

#[test]
fn a_step_not_yet_answered_runs_to_now() -> Result<(), Box<dyn Error>> {
    let model = play(29)?;
    let trace = Trace::of(thread(&model)?, None, at(60)).ok_or("no turn")?;
    let rows = trace.rows();
    let wait = rows
        .iter()
        .find(|row| row.id.as_str().starts_with("wait-"))
        .ok_or("no wait")?;
    assert_eq!(wait.words("took"), Some("0:03…"));
    assert!(matches!(wait.get("actions"), Some(Cell::Tags(t)) if t.len() == 4));
    let header = Header::of(&table(rows)).ok_or("no header")?;
    assert!(header.summary.contains("so far"), "{}", header.summary);
    Ok(())
}

fn child(id: &str, cost: f64, first: usize) -> Result<Agent, Box<dyn Error>> {
    let update: ChildUpdate = serde_json::from_str(&format!(
        r#"{{"id":"{id}","name":"Tests","status":"completed","activity":"waiting","toolUseCount":1,"tokenCount":0,"cost":{cost},"elapsedMs":1000}}"#
    ))?;
    Ok(Agent {
        id: AgentId::new(id),
        parent: None,
        update,
        first_seen: at(first),
        last_seen: at(first.saturating_add(10)),
        flagged: None,
        transcript: Transcript::default(),
        spawned_by: None,
        gone: false,
    })
}

#[test]
fn a_child_agents_work_hangs_under_it_and_its_spend_is_counted_once() -> Result<(), Box<dyn Error>>
{
    let model = play(51)?;
    let mut thread = thread(&model)?.clone();
    let mut worked = child("a1", 0.5, 30)?;
    let events = ALLOWED.lines().enumerate().skip(16).take(17);
    for (index, line) in events {
        let row: Value = serde_json::from_str(line)?;
        let event = row.pointer("/frame/params/update/event").cloned();
        if let Some(Ok(event)) = event.map(serde_json::from_value::<AgentEvent>) {
            worked.transcript.event(event, at(index));
        }
    }
    thread.agents.push(worked);
    thread.agents.push(child("a2", 0.25, 35)?);
    let rows = Trace::of(&thread, None, at(60)).ok_or("no turn")?.rows();
    let under = |parent: &str| -> Vec<String> {
        let of = |row: &&Row| row.parent.as_ref().is_some_and(|id| id.as_str() == parent);
        let ids = rows.iter().filter(of).map(|row| row.id.as_str());
        ids.map(|id| id.split('-').next().unwrap_or_default().to_owned())
            .collect()
    };
    assert_eq!(under("child-a1"), ["model", "tool"]);
    assert!(under("child-a2").is_empty());
    let turn = rows.first().ok_or("no turn row")?;
    // Two real replies cost nothing here; the child that sent none is charged its figure.
    assert_eq!(turn.words("cost"), Some("$0.25"));
    let first = rows
        .iter()
        .find(|row| row.id.as_str() == "child-a1")
        .ok_or("no child")?;
    assert_eq!(
        (first.words("state"), first.words("took")),
        (Some("completed"), Some("1.0 s"))
    );
    Ok(())
}

#[test]
fn the_mockups_rows_read_as_its_header_says() -> Result<(), Box<dyn Error>> {
    let header = Header::of(&table(rows_from_json(MOCKUP)?)).ok_or("no header")?;
    assert_eq!(header.title, "Turn 2");
    assert_eq!(header.summary, "14.8 s so far · $0.31 · 2 in parallel");
    assert_eq!(header.meta, "11 spans · 2 parallel");
    assert_eq!(header.ticks, ["0 s", "5", "10", "15"]);
    Ok(())
}

#[test]
fn the_lane_is_three_equal_whole_steps_that_hold_the_turn() {
    let lane = |ms: u64| span(Millis(ms)).0;
    assert_eq!(lane(14_800), 15_000);
    assert_eq!(lane(15_000), 15_000);
    assert_eq!(lane(15_001), 30_000);
    assert_eq!(lane(0), 3_000);
    assert_eq!(lane(400), 3_000);
}
