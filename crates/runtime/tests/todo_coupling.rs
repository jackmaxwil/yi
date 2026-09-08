use std::error::Error;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_loop::{ExecutionMode, TurnSnapshot};
use yi_runtime::todo::coupling::{
    Cycle, EMPTY_STOP_TEXT, Eager, INTERCEPT_CUSTOM_TYPE, Options, SEED_ACTOR, StopPosture,
    coupling, gate, landed, numbers_of, stop_posture,
};
use yi_runtime::todo::{Op, Target, TodoStore, latest_record};
use yi_runtime::{AgentSession, ProviderStream, SessionConfig};
use yi_types::message::{AgentMessage, Attribution, StopReason, UserContent};
use yi_types::model::{Model, ModelCost, ToolChoice};
use yi_types::plan::doc::TodoLabel;
use yi_types::todo::{BlockedOn, PhaseName, TODO_INTERCEPT_ENTRY_TYPE, TodoInterceptRecord};

type TestResult = Result<(), Box<dyn Error>>;

fn faux_model() -> Model {
    let zero = || serde_json::Number::from(0u64);
    Model {
        id: "faux-1".to_owned(),
        name: "Faux".to_owned(),
        api: "faux".to_owned(),
        provider: "faux".to_owned(),
        base_url: "http://localhost:0".to_owned(),
        reasoning: false,
        input: vec!["text".to_owned()],
        cost: ModelCost {
            input: zero(),
            output: zero(),
            cache_read: zero(),
            cache_write: zero(),
            tiers: None,
        },
        context_window: 128_000,
        max_tokens: 16_384,
        compat: None,
        thinking_level_map: None,
        headers: None,
    }
}

fn memory_store(name: &str) -> yi_session::SharedSession {
    Arc::new(Mutex::new(yi_session::SessionStore::in_memory(
        yi_session::SessionMetadata {
            id: name.to_owned(),
            created_at: 0,
            parent_session_id: None,
            name: None,
        },
    )))
}

struct Rig {
    session: AgentSession,
    store: yi_session::SharedSession,
    todos: Arc<TodoStore>,
}

fn rig(name: &str) -> Result<Rig, Box<dyn Error>> {
    let session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        Arc::new(ProviderStream::new(None, None)),
    );
    let store = memory_store(name);
    session.attach_store(store.clone())?;
    let todos = TodoStore::new(session.store_handle(), "main");
    Ok(Rig {
        session,
        store,
        todos,
    })
}

fn open_list(todos: &TodoStore) -> Result<(), Box<dyn Error>> {
    todos.apply(
        Op::Init {
            phases: vec![(
                PhaseName::new("Tasks")?,
                vec![TodoLabel::new("first")?, TodoLabel::new("second")?],
            )],
        },
        None,
    )?;
    Ok(())
}

fn stop(text: &str) -> AgentMessage {
    faux_assistant_message(vec![faux_text(text)], StopReason::Stop)
}

fn user(text: &str) -> AgentMessage {
    AgentMessage::User {
        content: UserContent::Text(text.to_owned()),
        attribution: Attribution::User,
        timestamp: 0,
    }
}

fn result(id: &str, name: &str, is_error: bool) -> AgentMessage {
    AgentMessage::ToolResult {
        tool_call_id: id.to_owned(),
        tool_name: name.to_owned(),
        content: vec![faux_text("ok")],
        details: None,
        usage: None,
        added_tool_names: None,
        is_error,
        timestamp: 0,
    }
}

fn intercept_records(store: &yi_session::SharedSession) -> Vec<TodoInterceptRecord> {
    yi_session::lock_session(store)
        .find_entries(&yi_session::EntryQuery {
            custom_type: Some(TODO_INTERCEPT_ENTRY_TYPE.to_owned()),
            order: yi_session::EntryOrder::OldestFirst,
            ..yi_session::EntryQuery::default()
        })
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| match entry {
            yi_types::entry::Entry::Custom {
                data: Some(data), ..
            } => serde_json::from_value(data).ok(),
            _ => None,
        })
        .collect()
}

fn custom_type(message: &AgentMessage) -> (String, bool, String) {
    match message {
        AgentMessage::Custom {
            custom_type,
            display,
            content: UserContent::Text(text),
            ..
        } => (custom_type.clone(), *display, text.clone()),
        _ => (String::new(), false, String::new()),
    }
}

#[test]
fn an_unchanged_open_set_climbs_the_ladder_and_progress_restarts_it() -> TestResult {
    let mut cycle = Cycle::default();
    assert_eq!(cycle.intercept("a=running"), Some(1));
    assert_eq!(cycle.intercept("a=running"), Some(2));
    assert_eq!(cycle.intercept("a=running"), Some(3));
    assert_eq!(
        cycle.intercept("a=running"),
        None,
        "past the top rung the turn ends"
    );
    assert_eq!(
        cycle.intercept("b=running"),
        Some(1),
        "progress restarts the ladder"
    );
    assert_eq!(cycle.intercept("c=running"), Some(1));
    assert_eq!(cycle.intercept("d=running"), Some(1));
    assert_eq!(
        cycle.intercept("e=running"),
        None,
        "{} re-drives is the cycle's whole budget",
        gate::INTERCEPT_CAP_PER_CYCLE
    );
    cycle.reset();
    assert_eq!(cycle.intercept("e=running"), Some(1));
    Ok(())
}

#[test]
fn the_nudge_fires_once_per_crossing_and_twice_per_cycle() -> TestResult {
    let mut cycle = Cycle::default();
    assert!(!cycle.work(gate::NUDGE_WORK - 1));
    assert!(cycle.work(1));
    assert!(!cycle.work(gate::NUDGE_WORK - 1));
    cycle.touched();
    assert!(
        !cycle.work(gate::NUDGE_WORK - 1),
        "a todo op re-arms the counter from zero"
    );
    assert!(cycle.work(1));
    assert!(
        !cycle.work(gate::NUDGE_WORK * 3),
        "the second crossing was the last"
    );
    Ok(())
}

#[test]
fn posture_reads_states_never_sentences() -> TestResult {
    let r = rig("posture")?;
    open_list(&r.todos)?;
    assert_eq!(stop_posture(&r.todos.list(), false), StopPosture::Continue);
    assert_eq!(stop_posture(&r.todos.list(), true), StopPosture::Quiet);
    r.todos.apply(
        Op::Block {
            label: TodoLabel::new("first")?,
            on: BlockedOn::External,
            note: "CI".to_owned(),
        },
        None,
    )?;
    assert_eq!(stop_posture(&r.todos.list(), false), StopPosture::Continue);
    r.todos.apply(
        Op::Block {
            label: TodoLabel::new("second")?,
            on: BlockedOn::External,
            note: "CI".to_owned(),
        },
        None,
    )?;
    assert_eq!(stop_posture(&r.todos.list(), false), StopPosture::Cadence);
    r.todos.apply(
        Op::Block {
            label: TodoLabel::new("second")?,
            on: BlockedOn::User,
            note: "which branch".to_owned(),
        },
        None,
    )?;
    assert_eq!(stop_posture(&r.todos.list(), false), StopPosture::Ask);
    r.todos.apply(
        Op::Done {
            target: Target::All,
            evidence: Some("both blocked; nothing left to check".to_owned()),
        },
        None,
    )?;
    assert_eq!(stop_posture(&r.todos.list(), false), StopPosture::Quiet);
    Ok(())
}

#[test]
fn work_is_changes_landed_not_calls_made() -> TestResult {
    let mut status: Map<String, Value> = Map::new();
    status.insert("command".to_owned(), json!("git status"));
    let mut build: Map<String, Value> = Map::new();
    build.insert("command".to_owned(), json!("mv old.rs new.rs"));
    let message = faux_assistant_message(
        vec![
            faux_tool_call("c1", "bash", status),
            faux_tool_call("c2", "bash", build),
            faux_tool_call("c3", "edit", Map::new()),
            faux_tool_call("c4", "edit", Map::new()),
            faux_tool_call("c5", "ipython", Map::new()),
            faux_tool_call("c6", "read", Map::new()),
        ],
        StopReason::ToolUse,
    );
    let results = vec![
        result("c1", "bash", false),
        result("c2", "bash", false),
        result("c3", "edit", false),
        result("c4", "edit", true),
        result("c5", "ipython", false),
        result("c6", "read", false),
    ];
    assert_eq!(landed(&message, &results), (2, false));
    let results = vec![result("c7", "todo", false)];
    assert_eq!(landed(&message, &results), (0, true));
    Ok(())
}

#[test]
fn a_stop_with_open_todos_is_re_driven_up_the_ladder_then_let_go() -> TestResult {
    let r = rig("ladder")?;
    open_list(&r.todos)?;
    let hooks = coupling(
        &r.session,
        Arc::clone(&r.todos),
        Options {
            eager: Eager::Prelude,
            children_running: Arc::new(|| false),
            inner: None,
        },
    );
    let message = stop("All done, anything else?");
    fn snapshot(message: &AgentMessage) -> TurnSnapshot<'_> {
        TurnSnapshot {
            message,
            tool_results: &[],
        }
    }
    let first = (hooks.intercept_stop)(&snapshot(&message)).ok_or("rung 1 must re-drive")?;
    let (kind, display, text) = custom_type(&first);
    assert_eq!(kind, INTERCEPT_CUSTOM_TYPE);
    assert!(display, "rung 1 is the one the user sees");
    assert!(text.contains("done \"first\" evidence="), "{text}");
    assert!(text.contains("block \"first\" on user note="), "{text}");
    assert!(text.contains("start \"second\""), "{text}");
    let second = (hooks.intercept_stop)(&snapshot(&message)).ok_or("rung 2")?;
    let (_, display, text) = custom_type(&second);
    assert!(!display);
    assert!(
        text.contains("`block` the item on user and ask in the same message"),
        "{text}"
    );
    let third = (hooks.intercept_stop)(&snapshot(&message)).ok_or("rung 3")?;
    let (_, _, text) = custom_type(&third);
    assert!(text.contains("call no tools"), "{text}");
    assert!(
        (hooks.intercept_stop)(&snapshot(&message)).is_none(),
        "past the top rung the turn ends"
    );
    let rungs: Vec<u8> = intercept_records(&r.store)
        .iter()
        .map(|record| record.rung)
        .collect();
    assert_eq!(rungs, vec![1, 2, 3, 3]);
    let reasons: Vec<String> = intercept_records(&r.store)
        .iter()
        .map(|record| record.reason.clone())
        .collect();
    assert_eq!(reasons.last().map(String::as_str), Some("let go"));

    r.todos.apply(
        Op::Done {
            target: Target::Label(TodoLabel::new("first")?),
            evidence: Some("test green".to_owned()),
        },
        None,
    )?;
    let again = (hooks.intercept_stop)(&snapshot(&message)).ok_or("progress restarts")?;
    let (_, display, _) = custom_type(&again);
    assert!(display, "a fresh rung 1 after progress");
    Ok(())
}

#[test]
fn only_a_state_suppresses_the_interception() -> TestResult {
    let r = rig("suppress")?;
    open_list(&r.todos)?;
    let hooks = coupling(
        &r.session,
        Arc::clone(&r.todos),
        Options {
            eager: Eager::Prelude,
            children_running: Arc::new(|| false),
            inner: None,
        },
    );
    let question = stop("Which branch should I land on?");
    assert!(
        (hooks.intercept_stop)(&TurnSnapshot {
            message: &question,
            tool_results: &[],
        })
        .is_some(),
        "a question mark alone is not a clean stop"
    );
    let asked = vec![result("q1", "ask_user", false)];
    assert!(
        (hooks.intercept_stop)(&TurnSnapshot {
            message: &question,
            tool_results: &asked,
        })
        .is_none(),
        "an ask_user call is"
    );
    r.todos.apply(
        Op::Block {
            label: TodoLabel::new("first")?,
            on: BlockedOn::User,
            note: "which branch".to_owned(),
        },
        None,
    )?;
    assert!(
        (hooks.intercept_stop)(&TurnSnapshot {
            message: &question,
            tool_results: &[],
        })
        .is_none(),
        "so is an item blocked on the user"
    );
    Ok(())
}

#[test]
fn terminal_stops_are_never_re_driven_and_empty_stops_are_capped() -> TestResult {
    let r = rig("terminal")?;
    open_list(&r.todos)?;
    let hooks = coupling(
        &r.session,
        Arc::clone(&r.todos),
        Options {
            eager: Eager::Prelude,
            children_running: Arc::new(|| false),
            inner: None,
        },
    );
    for reason in [StopReason::Aborted, StopReason::Error, StopReason::Length] {
        let message = faux_assistant_message(vec![faux_text("half")], reason);
        assert!(
            (hooks.intercept_stop)(&TurnSnapshot {
                message: &message,
                tool_results: &[],
            })
            .is_none(),
            "{reason:?} ends the turn"
        );
    }
    let empty = faux_assistant_message(Vec::new(), StopReason::Stop);
    for _ in 0..gate::EMPTY_STOP_CAP {
        let message = (hooks.intercept_stop)(&TurnSnapshot {
            message: &empty,
            tool_results: &[],
        })
        .ok_or("an empty stop is re-driven")?;
        assert_eq!(custom_type(&message).2, EMPTY_STOP_TEXT);
    }
    assert!(
        (hooks.intercept_stop)(&TurnSnapshot {
            message: &empty,
            tool_results: &[],
        })
        .is_none()
    );
    Ok(())
}

#[test]
fn a_multi_step_prompt_gets_the_prelude_and_force_is_opt_in() -> TestResult {
    let r = rig("prelude")?;
    let prompt = user("Fix the parser, then add the test, and land it on the branch.");
    let question = user("what does the parser do?");
    let hooks = coupling(
        &r.session,
        Arc::clone(&r.todos),
        Options {
            eager: Eager::Prelude,
            children_running: Arc::new(|| false),
            inner: None,
        },
    );
    assert_eq!(r.session.pending_count(), 0);
    assert!((hooks.on_prompt)(&prompt).is_none(), "prelude never forces");
    assert_eq!(
        r.session.pending_count(),
        1,
        "the prelude is queued for the turn"
    );
    assert!((hooks.on_prompt)(&question).is_none());
    assert_eq!(r.session.pending_count(), 1, "a question gets no prelude");

    let forced = coupling(
        &r.session,
        Arc::clone(&r.todos),
        Options {
            eager: Eager::Force,
            children_running: Arc::new(|| false),
            inner: None,
        },
    );
    match (forced.on_prompt)(&prompt) {
        Some(ToolChoice::Tool(tool)) => assert_eq!(String::from(tool), "todo"),
        other => return Err(format!("force must name the todo tool, got {other:?}").into()),
    }
    Ok(())
}

fn prelude_hooks(r: &Rig) -> yi_runtime::session::TurnCoupling {
    coupling(
        &r.session,
        Arc::clone(&r.todos),
        Options {
            eager: Eager::Prelude,
            children_running: Arc::new(|| false),
            inner: None,
        },
    )
}

fn labels(todos: &TodoStore) -> Vec<String> {
    todos
        .list()
        .items()
        .map(|item| item.label.to_string())
        .collect()
}

#[test]
fn a_numbered_prompt_seeds_one_pending_item_per_line() -> TestResult {
    let r = rig("seed")?;
    let hooks = prelude_hooks(&r);
    let prompt =
        user("Do these:\n1. add the parser\n2) wire the CLI\n- write the test\n1. add the parser");
    assert!((hooks.on_prompt)(&prompt).is_none(), "seeding never forces");
    assert_eq!(
        labels(&r.todos),
        vec!["add the parser", "wire the CLI", "write the test"],
        "one item per line, duplicates once"
    );
    assert_eq!(r.todos.progress().open, 3);
    assert_eq!(
        latest_record(&r.store).map(|record| record.actor),
        Some(SEED_ACTOR.to_owned()),
        "the record names the prompt as the actor"
    );
    assert_eq!(r.session.pending_count(), 1, "the seeded prelude is queued");

    let prose = user("Fix the parser, then add the test, and land it on the branch.");
    let fresh = rig("seed-prose")?;
    let hooks = prelude_hooks(&fresh);
    assert!((hooks.on_prompt)(&prose).is_none());
    assert_eq!(fresh.todos.progress().total, 0, "prose seeds nothing");
    assert_eq!(
        fresh.session.pending_count(),
        1,
        "the plain prelude still rides"
    );
    Ok(())
}

#[test]
fn an_open_list_is_never_reseeded() -> TestResult {
    let r = rig("reseed")?;
    open_list(&r.todos)?;
    let hooks = prelude_hooks(&r);
    let prompt = user("Now:\n1. something else\n2. and another");
    assert!((hooks.on_prompt)(&prompt).is_none());
    assert_eq!(labels(&r.todos), vec!["first", "second"]);
    Ok(())
}

#[test]
fn a_line_over_the_label_max_is_cut_not_dropped() -> TestResult {
    let r = rig("seed-long")?;
    let hooks = prelude_hooks(&r);
    let long = "word ".repeat(40);
    let prompt = user(&format!("1. {long}\n2. short"));
    assert!((hooks.on_prompt)(&prompt).is_none());
    let labels = labels(&r.todos);
    assert_eq!(labels.len(), 2);
    let first = labels.first().ok_or("no first label")?;
    assert!(first.chars().count() <= yi_types::plan::doc::TODO_LABEL_MAX);
    assert!(first.starts_with("word word"));
    Ok(())
}

#[test]
fn the_first_list_nudge_fires_once_after_three_changes_on_an_empty_list() -> TestResult {
    let mut cycle = Cycle::default();
    assert!(!cycle.first_list(gate::FIRST_LIST_WORK - 1));
    assert!(cycle.first_list(1));
    assert!(
        !cycle.first_list(gate::FIRST_LIST_WORK * 3),
        "once per cycle"
    );
    cycle.reset();
    assert!(
        cycle.first_list(gate::FIRST_LIST_WORK),
        "a new prompt re-arms it"
    );

    let r = rig("first-list")?;
    let hooks = prelude_hooks(&r);
    let message = faux_assistant_message(
        vec![
            faux_tool_call("c1", "edit", Map::new()),
            faux_tool_call("c2", "edit", Map::new()),
            faux_tool_call("c3", "write", Map::new()),
        ],
        StopReason::ToolUse,
    );
    let results = vec![
        result("c1", "edit", false),
        result("c2", "edit", false),
        result("c3", "write", false),
    ];
    let snapshot = TurnSnapshot {
        message: &message,
        tool_results: &results,
    };
    assert_eq!(r.session.pending_count(), 0);
    (hooks.on_turn)(&snapshot);
    assert_eq!(
        r.session.pending_count(),
        1,
        "three changes with no list earn one nudge"
    );
    (hooks.on_turn)(&snapshot);
    assert_eq!(r.session.pending_count(), 1, "and only one");
    Ok(())
}

#[test]
fn numbers_match_the_extractor_regex() {
    assert_eq!(numbers_of("1,234 files"), vec!["234"]);
    assert!(numbers_of("v1.2.345").is_empty());
    assert!(numbers_of("abc123 and 123abc").is_empty());
    assert!(numbers_of("12 of 99").is_empty());
    assert_eq!(numbers_of("ran 4567 tests, 4567 again"), vec!["4567"]);
    assert_eq!(numbers_of("(1088) and 2500. then 777"), vec!["1088", "777"]);
}

#[test]
fn an_unsourced_number_is_re_driven_once_per_cycle() -> TestResult {
    let r = rig("unsourced")?;
    let mut seen = result("c1", "bash", false);
    if let AgentMessage::ToolResult { content, .. } = &mut seen {
        *content = vec![faux_text("42 passed, 1088 lines")];
    }
    yi_session::lock_session(&r.store).append_message("main", seen)?;
    let hooks = prelude_hooks(&r);
    fn snapshot(message: &AgentMessage) -> TurnSnapshot<'_> {
        TurnSnapshot {
            message,
            tool_results: &[],
        }
    }
    let claim = stop("There are 1088 lines and 2500 tests.");
    let first = (hooks.intercept_stop)(&snapshot(&claim)).ok_or("an unsourced number re-drives")?;
    let (kind, display, text) = custom_type(&first);
    assert_eq!(kind, INTERCEPT_CUSTOM_TYPE);
    assert!(!display);
    assert!(text.contains("2500") && !text.contains("1088"), "{text}");
    assert_eq!(
        intercept_records(&r.store)
            .last()
            .map(|record| (record.reason.clone(), record.rung)),
        Some(("unsourced".to_owned(), 0))
    );
    assert!(
        (hooks.intercept_stop)(&snapshot(&claim)).is_none(),
        "once per cycle, and an empty list has nothing else to say"
    );

    let fresh = rig("sourced")?;
    yi_session::lock_session(&fresh.store).append_message("main", user("how many of the 4567?"))?;
    let hooks = prelude_hooks(&fresh);
    let sourced = stop("4567, as you said.");
    assert!(
        (hooks.intercept_stop)(&snapshot(&sourced)).is_none(),
        "a number the user wrote is sourced"
    );
    Ok(())
}

#[test]
fn the_cycle_counter_survives_a_resume() -> TestResult {
    let r = rig("resume")?;
    open_list(&r.todos)?;
    let hooks = coupling(
        &r.session,
        Arc::clone(&r.todos),
        Options {
            eager: Eager::Prelude,
            children_running: Arc::new(|| false),
            inner: None,
        },
    );
    let message = stop("done");
    for _ in 0..3 {
        (hooks.intercept_stop)(&TurnSnapshot {
            message: &message,
            tool_results: &[],
        });
    }
    let resumed = coupling(
        &r.session,
        Arc::clone(&r.todos),
        Options {
            eager: Eager::Prelude,
            children_running: Arc::new(|| false),
            inner: None,
        },
    );
    assert!(
        (resumed.intercept_stop)(&TurnSnapshot {
            message: &message,
            tool_results: &[],
        })
        .is_none(),
        "a resumed session does not mint a fresh ladder for the same open set"
    );
    Ok(())
}
