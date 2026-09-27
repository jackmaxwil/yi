//! With a plan open the session todo list is the plan's view (D226): every committed plan op
//! re-projects the plan into the list, and the todo tool refuses to step it.

use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_loop::{ExecutionMode, TurnSnapshot};
use yi_runtime::plan::ops::{Actor, Delegate, Op, OpRequest, PlanEngine, TodoSpec};
use yi_runtime::plan::store::PlanStore;
use yi_runtime::todo::coupling::{Eager, Options, coupling};
use yi_runtime::todo::mirror::{ENGINE_ACTOR, Mirror, plan_of};
use yi_runtime::todo::{Op as TodoOp, TodoError, TodoStore, latest_record, text};
use yi_runtime::{AgentSession, ProviderStream, SessionConfig};
use yi_types::message::{AgentMessage, Attribution, StopReason, UserContent};
use yi_types::model::{Model, ModelCost};
use yi_types::plan::doc::{
    AgentId, Check, Delegation, GoalText, SpawnSpec, TodoAddr, TodoLabel, TodoStateName,
};
use yi_types::todo::{PhaseName, TodoItem};
use yi_types::url::Url;

type TestResult = Result<(), Box<dyn Error>>;

struct Child;

impl Delegate for Child {
    fn spawn(&self, _at: &TodoAddr, _delegation: &Delegation) -> Result<AgentId, String> {
        AgentId::new("child-0").map_err(|error| error.to_string())
    }

    fn reap(&self, _agent: &AgentId, _supplied: &[Url]) -> Result<Option<Url>, String> {
        Ok(None)
    }
}

struct Nothing;

impl yi_runtime::plan::ops::OpSink for Nothing {
    fn record(&self, _record: yi_types::plan::ledger::PlanOpRecord) -> Result<(), String> {
        Ok(())
    }
}

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

fn session(provider: Arc<ProviderStream>) -> AgentSession {
    AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    )
}

fn memory_store() -> yi_session::SharedSession {
    Arc::new(Mutex::new(yi_session::SessionStore::in_memory(
        yi_session::SessionMetadata {
            id: "mirror".to_owned(),
            created_at: 0,
            parent_session_id: None,
            name: None,
        },
    )))
}

fn spec(text: &str, delegated: bool) -> Result<TodoSpec, Box<dyn Error>> {
    let delegation = delegated.then(|| Delegation {
        spec: SpawnSpec {
            role: None,
            model: None,
            effort: None,
            tools: Vec::new(),
            isolation: None,
            budget: None,
            wall: None,
            parent_close: None,
            extra: Map::new(),
        },
        accept: Check::Command("true".to_owned()),
        output: None,
        context: Vec::new(),
        note: None,
        extra: Map::new(),
    });
    Ok(TodoSpec {
        label: TodoLabel::new(text)?,
        after: Vec::new(),
        delegation,
        contract: None,
        children: Vec::new(),
    })
}

fn owner(op: Op) -> OpRequest {
    OpRequest {
        plan: None,
        actor: Actor::Owner,
        op,
        request_id: None,
        expected_revision: None,
    }
}

/// A session with its own list `first`, `gate`, and an engine whose sink is the mirror.
fn mirrored(dir: &Scratch) -> Result<(AgentSession, Arc<TodoStore>, PlanEngine), Box<dyn Error>> {
    let session = session(Arc::new(ProviderStream::new(None, None)));
    session.attach_store(memory_store())?;
    let todos = TodoStore::new(session.store_handle(), "main");
    todos.apply(
        TodoOp::Init {
            phases: vec![(
                PhaseName::new("Tasks")?,
                vec![TodoItem::from_text("first")?, TodoItem::from_text("gate")?],
            )],
        },
        None,
    )?;
    let store = PlanStore::open(dir.to_path_buf())?;
    todos.set_resync(Mirror::resync(store.clone()));
    let mirror = Mirror {
        inner: Arc::new(Nothing),
        todos: Arc::clone(&todos),
        store: store.clone(),
    };
    let engine = PlanEngine::new(store, Arc::new(Child)).with_op_sink(Arc::new(mirror));
    engine.apply(owner(Op::Init {
        goal: GoalText::new("ship the widget")?,
        todos: vec![spec("gate", false)?, spec("delegated job", true)?],
    }))?;
    Ok((session, todos, engine))
}

fn ids(todos: &TodoStore) -> Vec<(String, String, TodoStateName)> {
    todos
        .list()
        .items()
        .map(|item| {
            (
                item.id
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                item.label.to_string(),
                item.state.clone(),
            )
        })
        .collect()
}

#[test]
fn a_plan_op_replaces_the_list_and_keeps_ids() -> TestResult {
    let dir = Scratch::new("yi-todo-mirror-ids")?;
    let (session, todos, engine) = mirrored(&dir)?;
    let list = todos.list();
    let plan = plan_of(&list).ok_or("the list names no plan")?.to_owned();
    assert_eq!(
        list.phases.len(),
        2,
        "the owner's own phase, then the plan's"
    );
    assert_eq!(list.phases[1].name.as_str(), plan);
    assert_eq!(
        ids(&todos),
        [
            ("t1".to_owned(), "first".to_owned(), TodoStateName::Running),
            ("t2".to_owned(), "gate".to_owned(), TodoStateName::Pending),
            (
                "t3".to_owned(),
                "delegated job".to_owned(),
                TodoStateName::Running
            ),
        ],
        "gate keeps its id, the new label takes the next one"
    );
    let running = list.items().nth(2).ok_or("no third item")?;
    assert_eq!(running.extra.get("by"), Some(&json!("child-0")));
    assert_eq!(running.extra.get("plan"), Some(&json!(plan)));
    let record =
        latest_record(&session.store_handle()().ok_or("no store")?).ok_or("no todo record")?;
    assert_eq!(
        (record.op.as_str(), record.actor.as_str()),
        ("plan", ENGINE_ACTOR)
    );
    engine.apply(owner(Op::Start {
        label: TodoLabel::new("gate")?,
    }))?;
    assert_eq!(
        ids(&todos)[1],
        ("t2".to_owned(), "gate".to_owned(), TodoStateName::Running)
    );
    assert_eq!(
        text::header(&todos.list()),
        "Todos 0/3 · running: gate",
        "the environment's todo line carries the plan's counts and its running row"
    );
    Ok(())
}

/// Dies with the projection built from a list read outside the store's lock: an owner-row
/// write landing between that read and the replace is undone by the next plan op.
#[test]
fn a_plan_op_racing_an_owner_row_write_never_undoes_it() -> TestResult {
    let dir = Scratch::new("yi-todo-mirror-race")?;
    let (_session, todos, engine) = mirrored(&dir)?;
    let (first, gate) = (TodoLabel::new("first")?, TodoLabel::new("gate")?);
    let wide: Result<Vec<_>, _> = (0..250).map(|at| spec(&format!("s{at}"), false)).collect();
    engine.apply(owner(Op::Append { todos: wide? }))?;
    let stop = std::sync::atomic::AtomicBool::new(false);
    let state_of = |todos: &TodoStore| {
        let list = todos.list();
        let item = list.items().find(|item| item.label == first).cloned();
        item.map(|item| item.state)
    };
    let lost = std::thread::scope(|scope| {
        scope.spawn(|| {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let on = yi_types::plan::doc::BlockedOn::User;
                let (label, note) = (gate.clone(), "wait".to_owned());
                let _ = engine.apply(owner(Op::Block { label, on, note }));
                let label = gate.clone();
                let _ = engine.apply(owner(Op::Unblock { label }));
            }
        });
        let mut lost = 0;
        for _ in 0..100 {
            let on = yi_types::todo::BlockedOn::User;
            let (label, note) = (first.clone(), "wait".to_owned());
            let blocked = todos.apply(TodoOp::Block { label, on, note }, None).is_ok();
            lost += usize::from(!blocked || state_of(&todos) != Some(TodoStateName::Blocked));
            let label = first.clone();
            lost += usize::from(todos.apply(TodoOp::Unblock { label }, None).is_err());
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        lost
    });
    assert_eq!(lost, 0, "an owner-row block was undone by a plan op");
    Ok(())
}

#[test]
fn a_mirrored_item_refuses_start_with_the_plan_road() -> TestResult {
    let dir = Scratch::new("yi-todo-mirror-refuse")?;
    let (_session, todos, _engine) = mirrored(&dir)?;
    let before = todos.list();
    let refused = todos.apply(
        TodoOp::Start {
            label: TodoLabel::new("gate")?,
        },
        None,
    );
    let Err(error) = refused else {
        return Err("start on a mirrored item must be refused".into());
    };
    assert!(matches!(error, TodoError::Mirrored { .. }), "{error}");
    let text = error.to_string();
    assert!(
        text.contains("the plan tool changes it") && text.contains("the engine steps delegated"),
        "{text}"
    );
    assert_eq!(todos.list(), before, "the refusal moves nothing");
    assert!(todos.apply(TodoOp::View, None).is_ok(), "view passes");
    assert!(
        text::next_lines(&before).is_empty(),
        "no todo-tool move is offered for a list the todo tool cannot step"
    );
    Ok(())
}

#[test]
fn a_mirrored_list_with_a_running_child_does_not_nag() -> TestResult {
    let dir = Scratch::new("yi-todo-mirror-nag")?;
    let (session, todos, _engine) = mirrored(&dir)?;
    let hooks = coupling(
        &session,
        Arc::clone(&todos),
        Options {
            eager: Eager::Prelude,
            children_running: Arc::new(|| false),
            inner: None,
        },
    );
    let prompt = AgentMessage::User {
        content: UserContent::Text("Fix the parser, then add the test, and land it.".to_owned()),
        attribution: Attribution::User,
        timestamp: 0,
    };
    (hooks.on_prompt)(&prompt);
    let stop = faux_assistant_message(vec![faux_text("waiting on the child")], StopReason::Stop);
    let snapshot = TurnSnapshot {
        message: &stop,
        tool_results: &[],
    };
    assert!(
        (hooks.intercept_stop)(&snapshot).is_none(),
        "a stop over the plan's list is the plan coupling's to judge"
    );
    let edit = faux_assistant_message(
        vec![faux_tool_call("c1", "edit", Map::new())],
        StopReason::ToolUse,
    );
    let landed = [AgentMessage::ToolResult {
        tool_call_id: "c1".to_owned(),
        tool_name: "edit".to_owned(),
        content: vec![faux_text("ok")],
        details: None,
        usage: None,
        added_tool_names: None,
        is_error: false,
        timestamp: 0,
    }];
    for _ in 0..30 {
        (hooks.on_turn)(&TurnSnapshot {
            message: &edit,
            tool_results: &landed,
        });
    }
    assert_eq!(
        session.pending_count(),
        0,
        "no prelude and no nudge names a todo op the list refuses"
    );
    Ok(())
}

/// Dies with the mirror or its carry unwired (wiring.rs): the session's list stays the owner's
/// own, or the todo tool's done on the plan's item is refused.
#[tokio::test]
async fn a_plan_opened_through_the_tool_is_the_sessions_todo_list() -> TestResult {
    let root = Scratch::new("yi-todo-mirror-wired")?;
    let provider = Arc::new(ProviderStream::new(None, None));
    let mut args: Map<String, Value> = Map::new();
    args.insert("op".to_owned(), json!("init"));
    args.insert("goal".to_owned(), json!("ship the widget"));
    args.insert(
        "todos".to_owned(),
        json!([{"label": "cut the seam"}, {"label": "wire it"}]),
    );
    let mut done: Map<String, Value> = Map::new();
    done.insert("op".to_owned(), json!("done"));
    done.insert("label".to_owned(), json!("cut the seam"));
    done.insert("evidence".to_owned(), json!("`true` exit 0"));
    provider.queue_faux(vec![
        faux_assistant_message(
            vec![faux_tool_call("c1", "plan", args)],
            StopReason::ToolUse,
        ),
        faux_assistant_message(
            vec![faux_tool_call("c2", "todo", done)],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("opened")], StopReason::Stop),
    ]);
    let mut session = session(Arc::clone(&provider));
    session.attach_store(memory_store())?;
    yi_runtime::attach_runtime(
        &mut session,
        yi_runtime::RuntimeWiring {
            provider,
            system_prompt: "sys".to_owned(),
            tool_execution: ExecutionMode::Sequential,
            cwd: root.to_path_buf(),
            home: root.join("home"),
            lane_slots: 1,
            broker: None,
            tools: Arc::new(yi_tools::builtin_tools),
            depth: 0,
            max_depth: 1,
            rlm_dir: root.join("rlm"),
            family_dir: None,
            summarizer: None,
            advisor: None,
            auto_review: None,
            plan_stale_turns: None,
            plans_dir: Some(root.join("plans")),
            parent_link: None,
            wall: yi_runtime::Wall::default(),
            auto_background: None,
            deadline: None,
            kernel_prewarm: false,
            mcp_read: None,
            sessions_dir: None,
            kernels: yi_runtime::fetch::KernelServiceMap::new(),
        },
    );
    session.prompt("open a plan")?;
    session.wait_idle().await;
    let list = session.todos().ok_or("no todo store")?.list();
    assert!(plan_of(&list).is_some(), "{list:?}");
    assert_eq!(
        text::header(&list),
        "Todos 1/2",
        "the todo tool's done reached the plan"
    );
    Ok(())
}

// Dies with the resync in `apply_as`: the list keeps the finished plan's lock and every todo op
// is refused `Mirrored` naming a plan that is done.
#[test]
fn a_list_whose_plan_another_engine_finished_is_released() -> TestResult {
    let dir = Scratch::new("yi-todo-mirror-released")?;
    let (_session, todos, _engine) = mirrored(&dir)?;
    let cli = PlanEngine::new(PlanStore::open(dir.to_path_buf())?, Arc::new(Child));
    cli.apply(owner(Op::Drop {
        label: TodoLabel::new("gate")?,
        disposition: None,
    }))?;
    cli.apply(owner(Op::Fail {
        label: TodoLabel::new("delegated job")?,
        cause: "closed from the command line".to_owned(),
        disposition: None,
    }))?;
    assert!(plan_of(&todos.list()).is_some(), "no op reached the mirror");
    todos.apply(
        TodoOp::Init {
            phases: vec![(PhaseName::new("Next")?, vec![TodoItem::from_text("after")?])],
        },
        None,
    )?;
    let list = todos.list();
    assert_eq!(plan_of(&list), None);
    assert_eq!(
        list.items()
            .map(|item| item.label.to_string())
            .collect::<Vec<_>>(),
        ["after"]
    );
    Ok(())
}

/// Dies with a released plan's rows dropped in silence: a probe session's `todo set` answered
/// `Todos 1/5` after replacing nineteen rows of a finished plan.
#[test]
fn a_set_over_a_released_plan_names_the_rows_it_replaced() -> TestResult {
    use yi_tools::{Tool, ToolContext};
    let dir = Scratch::new("yi-todo-mirror-replaced")?;
    let (_session, todos, _engine) = mirrored(&dir)?;
    let plan = plan_of(&todos.list()).ok_or("no plan")?.to_owned();
    let cli = PlanEngine::new(PlanStore::open(dir.to_path_buf())?, Arc::new(Child));
    let label = TodoLabel::new("gate")?;
    cli.apply(owner(Op::Drop {
        label,
        disposition: None,
    }))?;
    let label = TodoLabel::new("delegated job")?;
    let cause = "closed from the command line".to_owned();
    cli.apply(owner(Op::Fail {
        label,
        cause,
        disposition: None,
    }))?;
    let tool = yi_runtime::todo::tool::TodoTool::new(Arc::clone(&todos));
    let args = json!({"op": "set", "list": "- [ ] after"});
    let output = tool.execute(
        args.as_object().cloned().unwrap_or_default(),
        &ToolContext::new(dir.to_path_buf()),
    );
    let text: String = output
        .result
        .content
        .iter()
        .map(|content| match content {
            yi_types::message::Content::Text { text, .. } => text.clone(),
            _ => String::new(),
        })
        .collect();
    assert!(!output.is_error, "{text}");
    assert!(
        text.contains(&format!("replaced plan {plan}'s 2 rows")),
        "{text}"
    );
    Ok(())
}

/// Dies with `Mirrored` for any label off the owner's rows: `todo start t999` was told the
/// plan tool changes an item that exists nowhere.
#[test]
fn an_unknown_label_on_a_mirrored_list_is_not_found() -> TestResult {
    let dir = Scratch::new("yi-todo-mirror-unknown")?;
    let (_session, todos, _engine) = mirrored(&dir)?;
    let refused = todos.apply(
        TodoOp::Start {
            label: TodoLabel::new("t999")?,
        },
        None,
    );
    let Err(error) = refused else {
        return Err("start on a label that exists nowhere must be refused".into());
    };
    assert!(matches!(error, TodoError::NoSuchLabel { .. }), "{error}");
    Ok(())
}

// Dies with an id looked up by label across the whole list: `cli > tests` takes the id of
// `api > tests` on the next projection, and two items answer to one id.
#[test]
fn a_child_label_under_two_parents_keeps_two_ids() -> TestResult {
    let dir = Scratch::new("yi-todo-mirror-children")?;
    let (_session, todos, engine) = mirrored(&dir)?;
    let parent = |label: &str| -> Result<TodoSpec, Box<dyn Error>> {
        let mut parent = spec(label, false)?;
        parent.children = vec![serde_json::from_value(
            json!({"label": "tests", "state": "pending"}),
        )?];
        Ok(parent)
    };
    engine.apply(owner(Op::Append {
        todos: vec![parent("api")?, parent("cli")?],
    }))?;
    let first = ids(&todos);
    engine.apply(owner(Op::Start {
        label: TodoLabel::new("gate")?,
    }))?;
    let again = ids(&todos);
    let tests: Vec<&String> = again
        .iter()
        .filter(|(_, label, _)| label == "tests")
        .map(|(id, _, _)| id)
        .collect();
    assert_eq!(tests.len(), 2);
    assert_ne!(tests[0], tests[1], "{again:?}");
    let ids_of = |rows: &[(String, String, TodoStateName)]| -> Vec<String> {
        rows.iter().map(|(id, _, _)| id.clone()).collect()
    };
    assert_eq!(
        ids_of(&again),
        ids_of(&first),
        "a projection keeps every id"
    );
    Ok(())
}

/// Twenty-one final-confirmation `todo done t1` calls named the owner's own list, which the
/// plan's init had replaced: the owner's open items stay beside the plan with their ids.
#[test]
fn an_owners_open_items_survive_the_plan_and_the_todo_tool_steps_them() -> TestResult {
    let dir = Scratch::new("yi-todo-mirror-own")?;
    let (_session, todos, engine) = mirrored(&dir)?;
    let first = ids(&todos);
    assert_eq!(
        first.first(),
        Some(&("t1".to_owned(), "first".to_owned(), TodoStateName::Running)),
        "{first:?}"
    );
    assert_eq!(
        first.iter().filter(|(_, label, _)| label == "gate").count(),
        1,
        "an own item the plan declared is the plan's: {first:?}"
    );
    todos.apply(
        TodoOp::Done {
            target: yi_runtime::todo::Target::Label(TodoLabel::new("t1")?),
            evidence: Some("`true` exit 0".to_owned()),
        },
        None,
    )?;
    engine.apply(owner(Op::Start {
        label: TodoLabel::new("gate")?,
    }))?;
    let after = ids(&todos);
    assert_eq!(
        after.first(),
        Some(&("t1".to_owned(), "first".to_owned(), TodoStateName::Done)),
        "{after:?}"
    );
    assert!(plan_of(&todos.list()).is_some());
    Ok(())
}

/// Dies with `normalize` promoting an owner row while the plan owns the list: the unblocked
/// `first` reads running again and the header names it over the plan's running child.
#[test]
fn an_owner_row_is_not_promoted_to_running_while_the_plan_is_open() -> TestResult {
    let dir = Scratch::new("yi-todo-mirror-promote")?;
    let (_session, todos, _engine) = mirrored(&dir)?;
    let (label, note) = (TodoLabel::new("first")?, "wait".to_owned());
    let on = yi_types::todo::BlockedOn::User;
    todos.apply(TodoOp::Block { label, on, note }, None)?;
    let label = TodoLabel::new("first")?;
    todos.apply(TodoOp::Unblock { label }, None)?;
    let rows = ids(&todos);
    assert_eq!(rows[0].2, TodoStateName::Pending, "{rows:?}");
    assert_eq!(
        text::header(&todos.list()),
        "Todos 0/3 · running: delegated job"
    );
    Ok(())
}

/// Dies with a whole-list op refused `Mirrored`: three confirmation `todo set`/`init` calls,
/// made on the harness's own prelude after the plan opened, bounced off the plan's list.
#[test]
fn a_todo_set_or_init_under_a_plan_writes_the_owners_rows_beside_it() -> TestResult {
    let dir = Scratch::new("yi-todo-mirror-set")?;
    let (_session, todos, _engine) = mirrored(&dir)?;
    let plan_rows = |todos: &TodoStore| {
        let list = todos.list();
        let rows: Vec<TodoItem> = list
            .items()
            .filter(|item| item.extra.contains_key("plan"))
            .cloned()
            .collect();
        rows
    };
    let before = plan_rows(&todos);
    todos.apply(
        TodoOp::Set {
            list: "- [ ] t2 wait for the plan\n- [ ] report".to_owned(),
        },
        None,
    )?;
    let phases = vec![(
        PhaseName::new("Mine")?,
        vec![TodoItem::from_text("verify it")?],
    )];
    todos.apply(TodoOp::Init { phases }, None)?;
    assert_eq!(plan_rows(&todos), before, "the plan's rows never move");
    let list = todos.list();
    let ids: Vec<String> = list
        .items()
        .filter_map(|item| item.id.as_ref().map(ToString::to_string))
        .collect();
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), ids.len(), "{ids:?}");
    assert!(list.items().any(|item| item.label.as_str() == "verify it"));
    assert!(plan_of(&list).is_some());
    Ok(())
}

/// A `todo done` on the plan's own inline item is the plan op the owner meant, not a bounce.
#[test]
fn a_todo_done_on_an_inline_plan_item_is_carried_to_the_plan_tool() -> TestResult {
    use yi_tools::{Tool, ToolContext};
    let dir = Scratch::new("yi-todo-mirror-carry")?;
    let (_session, todos, engine) = mirrored(&dir)?;
    let engine = Arc::new(engine);
    todos.set_carry(yi_runtime::todo::mirror::carry(Arc::downgrade(&engine)));
    let tool = yi_runtime::todo::tool::TodoTool::new(Arc::clone(&todos));
    let call = |args: Value| {
        let output = tool.execute(
            args.as_object().cloned().unwrap_or_default(),
            &ToolContext::new(dir.to_path_buf()),
        );
        let text: String = output
            .result
            .content
            .iter()
            .map(|content| match content {
                yi_types::message::Content::Text { text, .. } => text.clone(),
                _ => String::new(),
            })
            .collect();
        (output.is_error, text)
    };
    let (refused, text) = call(json!({"op": "done", "id": "t2", "evidence": "`true` exit 0"}));
    assert!(!refused, "{text}");
    let plan = engine.store().read(&engine.store().roots()?[0])?;
    let gate = plan.todo(&TodoLabel::new("gate")?).ok_or("gate")?;
    assert_eq!(
        TodoStateName::of(&gate.state),
        TodoStateName::Done,
        "{text}"
    );
    Ok(())
}
