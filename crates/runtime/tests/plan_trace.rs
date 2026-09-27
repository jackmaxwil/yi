//! Two-way traceability through the wired plan tool: a plan drafted over three user messages that
//! serves two of them names the third as possibly forgotten, and a todo citing no message that
//! resolves as one nobody asked for; both land in the plan journal as well as the tool result.

use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::sync::Arc;

use serde_json::{Map, json};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_runtime::ProviderStream;
use yi_runtime::plan::store::PlanStore;
use yi_runtime::plan::trace::{TRACE_KEY, TRACE_SHOWN, notices, trace};
use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, Content, StopReason};
use yi_types::plan::doc::{GoalText, Plan, PlanId, PlanTier, Todo, TodoLabel, TodoState, Waiver};
use yi_types::plan::ledger::PlanOpRecord;
use yi_types::url::Url;

use crate::todo_mirror::{memory_store, session, wired};

type TestResult = Result<(), Box<dyn Error>>;

fn tool_text(store: &yi_session::SharedSession, call: &str) -> Result<String, Box<dyn Error>> {
    let entries = yi_session::lock_session(store).find_entries(&yi_session::EntryQuery {
        order: yi_session::EntryOrder::OldestFirst,
        ..yi_session::EntryQuery::default()
    })?;
    entries
        .into_iter()
        .find_map(|entry| match entry {
            Entry::Message {
                message:
                    AgentMessage::ToolResult {
                        tool_call_id,
                        content,
                        ..
                    },
                ..
            } if tool_call_id == call => Some(
                content
                    .iter()
                    .filter_map(|block| match block {
                        Content::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect(),
            ),
            _ => None,
        })
        .ok_or_else(|| format!("no tool result for {call}").into())
}

/// Dies with the check unwired or disabled: no `trace:` row reaches the model, and the journal
/// record carries no flags.
#[tokio::test]
async fn a_plan_that_misses_a_message_and_invents_a_todo_is_flagged_both_ways() -> TestResult {
    let root = Scratch::new("yi-plan-trace")?;
    let provider = Arc::new(ProviderStream::new(None, None));
    let init = json!({"op": "init", "goal": "ship the parser", "todos": [
        {"label": "keep the guardrails green", "intent": ["user://1"]},
        {"label": "wire the parser"},
        {"label": "polish the docs", "intent": ["user://9"]}
    ]});
    let noted = || faux_assistant_message(vec![faux_text("noted")], StopReason::Stop);
    provider.queue_faux(vec![
        noted(),
        noted(),
        faux_assistant_message(
            vec![faux_tool_call(
                "c1",
                "plan",
                init.as_object().cloned().unwrap_or_default(),
            )],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("planned")], StopReason::Stop),
    ]);
    let mut session = session(Arc::clone(&provider));
    let store = memory_store();
    session.attach_store(Arc::clone(&store))?;
    wired(&mut session, &root, provider);
    for prompt in [
        "keep the guardrails green while you work",
        "also add a changelog entry for the parser",
        "now wire the parser",
    ] {
        session.prompt_message(yi_runtime::session::user_input(prompt))?;
        session.wait_idle().await;
    }

    let text = tool_text(&store, "c1")?;
    let rows: Vec<&str> = text
        .lines()
        .filter(|row| row.starts_with("trace:"))
        .collect();
    assert_eq!(
        rows,
        [
            "trace: 1 todo(s) cite no user message that resolves, so nobody asked for them: \"polish the docs\"; cite one with intent: [\"user://<n>\"]",
            "trace: 1 user message(s) no todo cites or waives, possibly forgotten: \"user://2\"; fetch one to read it, then cite it in a todo's intent or waive it with waived: [{address, reason}]",
        ],
        "{text}"
    );
    let id = PlanId::new("ship-the-parser")?;
    let plans = PlanStore::open(root.join("plans"))?;
    let reading = plans.journal(&id).read()?;
    let opened = reading.records.first().ok_or("no journal record")?;
    assert_eq!(
        opened.record.extra.get("trace"),
        Some(&json!({"unasked": ["polish the docs"], "forgotten": ["user://2"]}))
    );
    let plan = plans.read(&id)?;
    let defaulted = plan
        .todo(&TodoLabel::new("wire the parser")?)
        .ok_or("no todo")?;
    assert_eq!(
        defaulted
            .cites
            .intent
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["user://3"],
        "a todo declared without intent cites the prompt that drafted it"
    );
    Ok(())
}

fn todo(label: &str, intent: &[&str], waived: &[&str]) -> Result<Todo, Box<dyn Error>> {
    let mut todo = Todo::pending(TodoLabel::new(label)?);
    for url in intent {
        todo.cites.intent.push(url.parse()?);
    }
    for url in waived {
        todo.cites.waived.push(Waiver {
            address: url.parse()?,
            reason: "set aside".to_owned(),
            extra: Map::new(),
        });
    }
    Ok(todo)
}

/// Dies with a waiver not counted as cover, a dropped todo still counted, or an address past the
/// session's messages read as resolving.
#[test]
fn a_waiver_covers_a_message_and_a_dropped_todo_does_not() -> TestResult {
    let mut dropped = todo("old path", &["user://2"], &[])?;
    dropped.state = TodoState::Abandoned;
    let plan = Plan::opening(
        PlanId::new("covers")?,
        GoalText::new("covers")?,
        PlanTier::Root,
        vec![
            todo("serves one", &["user://1"], &["user://3"])?,
            dropped,
            todo("past the end", &["user://4"], &[])?,
        ],
    );
    let found = trace(&plan, 3);
    assert_eq!(found.unasked, [TodoLabel::new("past the end")?]);
    assert_eq!(found.forgotten, ["user://2".parse::<Url>()?]);
    Ok(())
}

fn record_with(unasked: usize) -> Result<PlanOpRecord, Box<dyn Error>> {
    let labels: Vec<String> = (1..=unasked).map(|n| format!("todo {n}")).collect();
    let mut extra = Map::new();
    extra.insert(
        TRACE_KEY.to_owned(),
        json!({"unasked": labels, "forgotten": []}),
    );
    Ok(PlanOpRecord {
        plan: PlanId::new("capped")?,
        op: "init".to_owned(),
        actor: "main".to_owned(),
        at: 0,
        todo: None,
        from: None,
        to: None,
        todos: 0,
        extra,
    })
}

/// Dies with the cap cut silently, or with the cut row written when nothing was cut.
#[test]
fn the_trace_cap_names_its_cut_only_past_the_cap() -> TestResult {
    let at = notices(&record_with(TRACE_SHOWN)?);
    assert_eq!(at.len(), 1, "{at:?}");
    let past = notices(&record_with(TRACE_SHOWN + 1)?);
    assert_eq!(
        past.last().map(String::as_str),
        Some("[… 8 of 9 shown (trace cap 8); fetch plan://capped shows every todo's intent]"),
        "{past:?}"
    );
    assert!(
        past.first()
            .is_some_and(|row| row.contains("\"todo 8\"") && !row.contains("\"todo 9\"")),
        "{past:?}"
    );
    Ok(())
}
