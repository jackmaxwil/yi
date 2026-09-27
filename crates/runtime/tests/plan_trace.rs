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
/// record carries no flags; or with a revision repeating a flag that still stands.
#[tokio::test]
async fn a_plan_that_misses_a_message_and_invents_a_todo_is_flagged_both_ways() -> TestResult {
    let root = Scratch::new("yi-plan-trace")?;
    let provider = Arc::new(ProviderStream::new(None, None));
    let init = json!({"op": "init", "goal": "ship the parser", "todos": [
        {"label": "keep the guardrails green", "intent": ["user://1"]},
        {"label": "wire the parser"},
        {"label": "polish the docs", "intent": ["user://9"]}
    ]});
    let drop = json!({"op": "drop", "label": "keep the guardrails green"});
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
        faux_assistant_message(
            vec![faux_tool_call(
                "c2",
                "plan",
                drop.as_object().cloned().unwrap_or_default(),
            )],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("dropped")], StopReason::Stop),
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
    let revised = tool_text(&store, "c2")?;
    let rows: Vec<&str> = revised
        .lines()
        .filter(|row| row.starts_with("trace:"))
        .collect();
    assert_eq!(
        rows,
        [
            "trace: 1 user message(s) no todo cites or waives, possibly forgotten: \"user://1\"; fetch one to read it, then cite it in a todo's intent or waive it with waived: [{address, reason}]"
        ],
        "a revision raises what it uncovered and repeats nothing standing: {revised}"
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

/// Dies with a waiver not counted as cover, a dropped todo still counted, an address past the
/// session's messages read as resolving, or a message a rewind left behind called forgotten.
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
            todo("past the end", &["user://5"], &[])?,
        ],
    );
    let found = trace(&plan, &[true, true, true, false]);
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
        Some(
            "[… 8 of 9 shown (trace cap 8); the rest are the todos whose intent cites no user message in fetch plan://capped]"
        ),
        "{past:?}"
    );
    assert!(
        past.first()
            .is_some_and(|row| row.contains("\"todo 8\"") && !row.contains("\"todo 9\"")),
        "{past:?}"
    );
    Ok(())
}

fn call(id: &str, args: serde_json::Value) -> AgentMessage {
    let args = args.as_object().cloned().unwrap_or_default();
    faux_assistant_message(vec![faux_tool_call(id, "plan", args)], StopReason::ToolUse)
}

/// Dies with the pick unread: the user's "2" leaves no answer, the unblock names no exemplar and
/// the reply's address never joins the intent; or with an unblock before any reply let through.
#[tokio::test]
async fn a_todo_asking_three_options_takes_the_users_pick_by_number() -> TestResult {
    let root = Scratch::new("yi-plan-ask")?;
    let provider = Arc::new(ProviderStream::new(None, None));
    let option = |id: &str, label: &str| json!({"id": id, "label": label, "preview": format!("hero: {label}")});
    let asked = |options: Vec<serde_json::Value>| {
        json!({"op": "block", "label": "hero style", "on": {"user": null},
            "note": "which hero?", "options": options})
    };
    let three = vec![
        option("a", "Calm"),
        option("b", "Bold"),
        option("c", "Dense"),
    ];
    provider.queue_faux(vec![
        call(
            "c1",
            json!({"op": "init", "goal": "land the page", "todos": [{"label": "hero style"}]}),
        ),
        call("c2", asked(three[..2].to_vec())),
        call("c3", asked(three)),
        call("c4", json!({"op": "unblock", "label": "hero style"})),
        faux_assistant_message(vec![faux_text("which hero?")], StopReason::Stop),
        call("c5", json!({"op": "unblock", "label": "hero style"})),
        faux_assistant_message(vec![faux_text("bold it is")], StopReason::Stop),
    ]);
    let mut session = session(Arc::clone(&provider));
    let store = memory_store();
    session.attach_store(Arc::clone(&store))?;
    wired(&mut session, &root, provider);
    for prompt in ["draft the landing page hero", "2"] {
        session.prompt_message(yi_runtime::session::user_input(prompt))?;
        session.wait_idle().await;
    }

    let two = tool_text(&store, "c2")?;
    assert!(two.contains("offers 3 to 5 options, not 2"), "{two}");
    let block = tool_text(&store, "c3")?;
    assert!(
        block.contains("options 1. Calm · 2. Bold · 3. Dense;")
            && block.contains("unattended it stays blocked: nothing picks for the user"),
        "{block}"
    );
    let early = tool_text(&store, "c4")?;
    assert!(
        early.contains("no user message came after it asked"),
        "an unblock with no reply since the ask is refused: {early}"
    );
    let picked = tool_text(&store, "c5")?;
    assert!(
        picked.contains("picked b Bold by user://2, the exemplar; rejected a, c"),
        "{picked}"
    );
    let plan = PlanStore::open(root.join("plans"))?.read(&PlanId::new("land-the-page")?)?;
    let todo = plan.todo(&TodoLabel::new("hero style")?).ok_or("no todo")?;
    assert_eq!(todo.state, TodoState::Pending);
    let ask = todo
        .ask
        .as_ref()
        .ok_or("the ask is kept past the unblock")?;
    let answer = ask.answer.as_ref().ok_or("no answer recorded")?;
    assert_eq!(answer.address.to_string(), "user://2");
    assert_eq!(answer.option.as_ref().map(|id| id.as_str()), Some("b"));
    assert_eq!(
        ask.options.len(),
        3,
        "the rejected options stay on the record"
    );
    let intent: Vec<String> = todo.cites.intent.iter().map(ToString::to_string).collect();
    assert_eq!(intent, ["user://1", "user://2"]);
    Ok(())
}
