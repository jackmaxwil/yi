use std::error::Error;
use std::path::Path;

use serde_json::{Value, json};
use yi_types::goal::{Goal, GoalStatus};
use yi_types::plan::contract::Resolution;
use yi_types::plan::doc::{AgentId as YiAgent, Todo, TodoState};
use yi_types::subagent::Discovery;
use yi_types::todo::{CheckLast, Claim, PhaseName, RowCheck, TodoList, TodoPhase};

use super::*;
use crate::findings::triage::{Act, TriageMsg};
use crate::ids::Millis;
use crate::msg::{Msg, step};
use crate::wire::event::WireEvent;
use crate::wire::replay::play;

type Check = Result<(), Box<dyn Error>>;

/// The thread `turn.jsonl` made, recorded from a real `yi serve`.
pub(crate) const THREAD: &str = "01a1de97-bf6c-7514-9fae-b57db1eaa7e1";

fn update(update: Value) -> Msg {
    let frame = json!({"jsonrpc": "2.0", "method": "session/update",
        "params": {"sessionId": THREAD, "update": update}});
    Msg::Wire(WireEvent::Line(frame.to_string()))
}

fn todo(label: &str, state: TodoState, check: Option<CheckLast>) -> Result<Todo, Box<dyn Error>> {
    let mut todo = Todo::from_text(label)?;
    todo.state = state;
    if let Some(last) = check {
        let command = format!("cargo test {}", label.len());
        RowCheck { command, last }.put(&mut todo);
    }
    Ok(todo)
}

/// The recorded turn, then the todos, claims and goal a working agent would have sent,
/// serialised from Yi's own types since no recording holds them yet.
pub(crate) fn worked() -> Result<Model, Box<dyn Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/wire/turn.jsonl");
    let mut model = Model::default();
    play(&mut model, &std::fs::read_to_string(path)?)?;
    let tests = YiAgent::new("tests")?;
    let items = vec![
        todo("Write the limiter", TodoState::Pending, None)?,
        todo(
            "Cover the refill boundary",
            TodoState::Running { by: tests },
            None,
        )?,
        todo(
            "Stop the underflow",
            TodoState::Done {
                output: None,
                resolution: None,
            },
            Some(CheckLast::red("9f2c")),
        )?,
        todo(
            "Document the limits",
            TodoState::Done {
                output: None,
                resolution: Some(Resolution::VerifiedDone),
            },
            Some(CheckLast::green()),
        )?,
        todo("Retry with jitter", TodoState::Abandoned, None)?,
    ];
    let list = TodoList {
        phases: vec![TodoPhase {
            name: PhaseName::new("Limit uploads")?,
            items,
            extra: serde_json::Map::new(),
        }],
        next_id: 6,
        extra: serde_json::Map::new(),
    };
    step(
        &mut model,
        update(json!({"sessionUpdate": "_yi/todo", "list": serde_json::to_value(&list)?})),
    );
    let claims = [
        Claim {
            label: "Document the limits".to_owned(),
            observed: Some("call_7".to_owned()),
        },
        Claim {
            label: "Rotate the keys".to_owned(),
            observed: None,
        },
    ];
    step(
        &mut model,
        update(json!({"sessionUpdate": "_yi/claims", "claims": serde_json::to_value(claims)?})),
    );
    let goal = Goal {
        objective: "Uploads cannot be hammered".to_owned(),
        status: GoalStatus::Active,
        token_budget: None,
        tokens_used: 0,
        time_used_seconds: 0,
        created: 1,
        updated: 2,
        check: Some("cargo test -p api upload::".to_owned()),
        check_timeout_ms: None,
        check_failure: None,
        discoveries: vec![Discovery {
            text: "Anonymous uploads are unlimited".to_owned(),
            violates_check_of: None,
            fingerprint: "anon-unlimited".to_owned(),
            extra: serde_json::Map::new(),
        }],
        extra: serde_json::Map::new(),
    };
    step(
        &mut model,
        update(json!({"sessionUpdate": "_yi/goal", "goal": serde_json::to_value(&goal)?})),
    );
    Ok(model)
}

fn stages(model: &Model) -> Vec<(String, Stage)> {
    findings(model, &ThreadId::new(THREAD), Scope::Thread)
        .into_iter()
        .map(|finding| (finding.title, finding.stage))
        .collect()
}

fn accept_all(model: &mut Model) {
    let thread = ThreadId::new(THREAD);
    for finding in findings(model, &thread, Scope::Thread) {
        let act = TriageMsg::Act {
            thread: thread.clone(),
            finding: finding.id,
            act: Act::Accept,
        };
        step(model, Msg::Triage(act));
    }
}

#[test]
fn an_agents_todos_claims_and_discoveries_are_suggestions_until_accepted() -> Check {
    let model = worked()?;
    let shown = stages(&model);
    let titles: Vec<&str> = shown.iter().map(|(title, _)| title.as_str()).collect();
    assert_eq!(
        titles,
        [
            "Write the limiter",
            "Cover the refill boundary",
            "Stop the underflow",
            "Document the limits",
            "Rotate the keys",
            "Anonymous uploads are unlimited",
        ],
        "an abandoned todo is no finding"
    );
    assert!(shown.iter().all(|(_, stage)| *stage == Stage::New));
    let counts = counts(&findings(&model, &ThreadId::new(THREAD), Scope::Thread));
    assert_eq!((counts.open, counts.suggestions), (0, 6));
    Ok(())
}

#[test]
fn an_accepted_finding_stands_where_its_record_and_checks_put_it() -> Check {
    let mut model = worked()?;
    accept_all(&mut model);
    assert_eq!(
        stages(&model),
        [
            ("Write the limiter".to_owned(), Stage::Open),
            ("Cover the refill boundary".to_owned(), Stage::Fixing),
            // Claimed done, but its check is red: back to fixing.
            ("Stop the underflow".to_owned(), Stage::Fixing),
            ("Document the limits".to_owned(), Stage::Done),
            // Claimed done with nothing in the session to check it against.
            ("Rotate the keys".to_owned(), Stage::Proving),
            ("Anonymous uploads are unlimited".to_owned(), Stage::Open),
        ]
    );
    let all = findings(&model, &ThreadId::new(THREAD), Scope::Thread);
    let owner = all.get(1).and_then(|finding| finding.owner.clone());
    assert_eq!(owner, Some(AgentId::new("tests")));
    let documented = all.get(3).ok_or("the verified todo is shown")?;
    let evidence: Vec<String> = documented
        .evidence
        .iter()
        .map(|thing| thing.address.to_string())
        .collect();
    assert_eq!(evidence, [format!("yi://step/{THREAD}/call_7")]);
    assert_eq!(
        all.get(2).map(|finding| finding.severity),
        Some(Severity::High)
    );
    Ok(())
}

#[test]
fn counts_add_up_and_the_gate_names_what_blocks() -> Check {
    let mut model = worked()?;
    accept_all(&mut model);
    let all = findings(&model, &ThreadId::new(THREAD), Scope::Project);
    let counts = counts(&all);
    let total: u32 = counts.stages.values().sum();
    assert_eq!(usize::try_from(total)?, all.len());
    assert_eq!(counts.open, 5, "everything filed but the done one");
    assert_eq!(counts.severities.values().sum::<u32>(), counts.open);
    assert_eq!(
        counts.places.iter().map(|place| place.count).sum::<u32>(),
        counts.open
    );
    assert_eq!(counts.suggestions, 0);
    let gate = ready(&all);
    let high: Vec<&str> = gate.high.iter().map(FindingId::as_str).collect();
    assert_eq!(high, [format!("{THREAD}/todo/Stop the underflow").as_str()]);
    let unproven: Vec<&str> = gate.unproven.iter().map(FindingId::as_str).collect();
    assert_eq!(
        unproven,
        [format!("{THREAD}/claim/Rotate the keys").as_str()]
    );
    assert!(!gate.ready());
    Ok(())
}

#[test]
fn a_dismissed_finding_leaves_and_a_merged_one_lends_its_evidence() -> Check {
    let mut model = worked()?;
    let thread = ThreadId::new(THREAD);
    let all = findings(&model, &thread, Scope::Thread);
    let first = all.first().ok_or("a finding")?.id.clone();
    let last = all.last().ok_or("a finding")?.id.clone();
    let documented = all.get(3).ok_or("a finding")?.id.clone();
    let act = Act::Dismiss {
        reason: "The limiter exists.".to_owned(),
    };
    step(
        &mut model,
        Msg::Triage(TriageMsg::Act {
            thread: thread.clone(),
            finding: first,
            act,
        }),
    );
    step(
        &mut model,
        Msg::Triage(TriageMsg::Merge {
            finding: documented,
            into: last.clone(),
        }),
    );
    let after = findings(&model, &thread, Scope::Thread);
    assert_eq!(after.len(), 4);
    let joined = after
        .iter()
        .find(|finding| finding.id == last)
        .ok_or("the merge target shows")?;
    assert_eq!(joined.evidence.len(), 1);
    Ok(())
}

#[test]
fn a_made_finding_is_a_suggestion_with_its_thing_as_evidence() -> Check {
    let mut model = worked()?;
    step(&mut model, Msg::Tick(Millis(5_000)));
    let line = Address::new(Kind::Lines, "src/rate_limit.rs")?.part("L20");
    let thread = ThreadId::new(THREAD);
    let make = TriageMsg::Make {
        thread: Some(thread.clone()),
        evidence: line.clone(),
    };
    step(&mut model, Msg::Triage(make));
    step(&mut model, Msg::Tick(Millis(8_000)));
    let all = findings(&model, &thread, Scope::Thread);
    let made = all.last().ok_or("the made finding shows")?;
    assert_eq!(made.origin, Origin::You);
    assert_eq!(made.stage, Stage::New);
    assert_eq!(
        made.evidence.first().map(|thing| &thing.address),
        Some(&line)
    );
    assert_eq!(made.place, Address::new(Kind::File, "src/rate_limit.rs")?);
    assert_eq!(made.age, Some(Millis(3_000)));
    Ok(())
}

#[test]
fn a_finding_made_from_a_pane_is_named_by_the_pane_and_placed_in_the_threads_folder() -> Check {
    use crate::grid::step::GridMsg;
    let mut model = worked()?;
    let show = GridMsg::Show {
        window: crate::ids::WindowId(0),
        saved: "everything",
    };
    step(&mut model, Msg::Grid(show));
    let terminal = model
        .grid
        .panes
        .iter()
        .find(|p| p.kind == crate::pane::PaneKind::Terminal);
    let pane = terminal.ok_or("no terminal")?.id;
    let evidence = Address::new(Kind::Pane, pane.0.to_string())?;
    let thread = ThreadId::new(THREAD);
    let make = TriageMsg::Make {
        thread: Some(thread.clone()),
        evidence,
    };
    step(&mut model, Msg::Triage(make));
    let all = findings(&model, &thread, Scope::Thread);
    let made = all.last().ok_or("the made finding shows")?;
    assert_eq!(made.title, "pane · Terminal", "named, not numbered");
    let cwd = &model.wire.threads.get(&thread).ok_or("no thread")?.cwd;
    assert_eq!(
        made.place,
        Address::new(Kind::Place, cwd)?,
        "where its thread works"
    );
    Ok(())
}

#[test]
fn an_attached_finding_reaches_the_model_as_its_words_not_an_app_address() -> Check {
    let model = worked()?;
    let all = findings(&model, &ThreadId::new(THREAD), Scope::Thread);
    let underflow = all.iter().find(|f| f.title == "Stop the underflow");
    let address = underflow.ok_or("no underflow finding")?.address()?;
    let sent = crate::address::attach::blocks(&[Thing::at(&model, address)]);
    let text = sent
        .first()
        .and_then(|b| b["text"].as_str())
        .ok_or("no text")?;
    assert!(text.contains("Stop the underflow"), "{text}");
    assert!(text.contains("Check: cargo test 18 (Failing)"), "{text}");
    assert!(!text.contains("yi://"), "{text}");
    Ok(())
}
