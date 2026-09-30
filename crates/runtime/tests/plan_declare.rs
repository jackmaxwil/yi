//! G2b: the shapes the paid G2 confirmation's parents wrote on the plan tool, and an owner's
//! step on a todo the engine runs. Each row was red on 059ba86b.

use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::{Value, json};
use yi_runtime::plan::acceptance::Phase;
use yi_runtime::plan::ops::{Actor, Delegate, Op, OpRequest, PlanEngine, PlanOpError};
use yi_runtime::plan::store::PlanStore;
use yi_runtime::plan::tool::PlanTool;
use yi_tools::{Tool, ToolContext};
use yi_types::plan::canonical::Digest;
use yi_types::plan::contract::{CheckerManifest, Decider};
use yi_types::plan::doc::{AgentId, Delegation, Isolation, Plan, TodoAddr, TodoLabel, TodoState};
use yi_types::url::Url;

type TestResult = Result<(), Box<dyn Error>>;

#[derive(Default)]
struct Stub(AtomicU32);

impl Delegate for Stub {
    fn spawn(&self, _at: &TodoAddr, _delegation: &Delegation) -> Result<AgentId, String> {
        let serial = self.0.fetch_add(1, Ordering::SeqCst);
        AgentId::new(format!("child-{serial}")).map_err(|error| error.to_string())
    }

    fn reap(&self, _agent: &AgentId, _supplied: &[Url]) -> Result<Option<Url>, String> {
        Ok(None)
    }
    fn finishes(&self) -> bool {
        true
    }
}

struct Rig {
    temp: Scratch,
    store: PlanStore,
    engine: Arc<PlanEngine>,
    tool: PlanTool,
}

fn rig(name: &str) -> Result<Rig, Box<dyn Error>> {
    let temp = Scratch::new(&format!("yi-plan-declare-{name}"))?;
    let store = PlanStore::open(temp.to_path_buf())?;
    let engine = Arc::new(PlanEngine::new(store.clone(), Arc::new(Stub::default())));
    let tool = PlanTool::new(Arc::clone(&engine), Actor::Owner);
    Ok(Rig {
        temp,
        store,
        engine,
        tool,
    })
}

/// The tool's answer and whether it was an error.
fn call(rig: &Rig, args: Value) -> (bool, String) {
    let input = args.as_object().cloned().unwrap_or_default();
    let output = rig
        .tool
        .execute(input, &ToolContext::new(std::env::temp_dir()));
    let text = output
        .result
        .content
        .iter()
        .map(|content| match content {
            yi_types::message::Content::Text { text, .. } => text.clone(),
            _ => String::new(),
        })
        .collect();
    (output.is_error, text)
}

fn opened(rig: &Rig, todos: Value) -> Result<Plan, Box<dyn Error>> {
    let (refused, text) = call(
        rig,
        json!({"op": "init", "goal": "ship it", "todos": todos}),
    );
    assert!(!refused, "{text}");
    let id = rig.store.roots()?.into_iter().next().ok_or("no plan")?;
    Ok(rig.store.read(&id)?)
}

fn todo_of<'a>(
    plan: &'a Plan,
    label: &str,
) -> Result<&'a yi_types::plan::doc::Todo, Box<dyn Error>> {
    Ok(plan.todo(&TodoLabel::new(label)?).ok_or("todo missing")?)
}

/// 248 engine starts were refused `contract` and 58 declarations `expected struct ArtifactRef`:
/// the parent wrote the checker as the command it is.
#[test]
fn a_plain_checker_command_is_frozen_and_the_engine_starts_the_todo() -> TestResult {
    let rig = rig("cmd")?;
    let plan = opened(
        &rig,
        json!([{
            "label": "alpha",
            "delegation": {"spec": {"isolation": "worktree"}, "accept": {"stated": "alpha.txt holds alpha"}},
            "contract": {"class": "writer", "items": [
                {"id": "alpha", "critical": true, "weight": 1, "decider": {"cmd": "grep -qx alpha alpha.txt"}}
            ]},
        }]),
    )?;
    let todo = todo_of(&plan, "alpha")?;
    assert!(
        matches!(todo.state, TodoState::Running { .. }),
        "{:?}",
        todo.state
    );
    let item = todo
        .contract
        .as_ref()
        .and_then(|c| c.items.first())
        .ok_or("no item")?;
    let Decider::Cmd {
        checker,
        timeout_ms,
    } = &item.decider
    else {
        return Err("not a cmd item".into());
    };
    assert_eq!(*timeout_ms, 60_000);
    let manifest = CheckerManifest::parse(&rig.store.artifacts(&plan.id).get(&checker.digest)?)?;
    assert_eq!(manifest.command, "grep -qx alpha alpha.txt");
    Ok(())
}

/// A worktree delegation's `accept` command is the contract when none is declared (D223 holds:
/// the todo is still contracted), with `isolation` written beside the spec and `delegate`,
/// the library's spelling, for `delegation`.
#[test]
fn a_worktree_accept_command_is_its_contract_whatever_the_spelling() -> TestResult {
    let rig = rig("accept")?;
    let plan = opened(
        &rig,
        json!([{
            "label": "beta",
            "delegate": {"isolation": "worktree", "spec": {}},
            "accept": {"command": "grep -qx beta beta.txt"},
        }]),
    )?;
    let todo = todo_of(&plan, "beta")?;
    let delegation = todo.delegation.as_ref().ok_or("no delegation")?;
    assert_eq!(delegation.spec.isolation, Some(Isolation::Worktree));
    assert!(delegation.extra.is_empty(), "{:?}", delegation.extra);
    let contract = todo.contract.as_ref().ok_or("no contract")?;
    assert!(contract.items.iter().all(|item| item.critical));
    assert!(
        matches!(todo.state, TodoState::Running { .. }),
        "{:?}",
        todo.state
    );
    let (refused, text) = call(
        &rig,
        json!({"op": "append", "todos": [{
            "label": "gamma",
            "delegation": {"isolation": "worktree", "spec": {"isolation": "none"}, "accept": {"command": "true"}},
        }]}),
    );
    assert!(
        refused && text.contains("isolation is named twice"),
        "{text}"
    );
    Ok(())
}

/// Children read `family://brief_quota` and failed: the 1024-byte cap pushed parents off the
/// note. An over-cap note is stored whole and the brief names its `plan://` address.
#[test]
fn an_over_cap_note_is_stored_whole_and_linked() -> TestResult {
    let rig = rig("note")?;
    let note = "Parse the ledger. ".repeat(200);
    let plan = opened(
        &rig,
        json!([{"label": "quota", "delegation": {"spec": {}, "accept": {"command": "true"}, "note": note}}]),
    )?;
    let delegation = todo_of(&plan, "quota")?
        .delegation
        .as_ref()
        .ok_or("no delegation")?;
    assert!(
        delegation
            .note
            .as_ref()
            .is_some_and(|head| head.as_str().len() <= 1024)
    );
    let digest = delegation.extra["note_ref"]["digest"]
        .as_str()
        .ok_or("no note_ref")?;
    let bytes = rig.store.artifacts(&plan.id).get(&Digest::parse(digest)?)?;
    assert_eq!(String::from_utf8(bytes)?, note);
    let family = rig.temp.join("family");
    std::fs::create_dir_all(&family)?;
    std::fs::write(family.join("brief_quota.json"), "\"the quota brief\"")?;
    let resolver = Arc::new(
        yi_runtime::fetch::Resolver::new(rig.temp.to_path_buf(), yi_runtime::Wall::default())
            .with_plans_dir(rig.store.dir().to_path_buf())
            .with_family_dir(family),
    );
    let mut tools = yi_tools::builtin_tools();
    yi_runtime::fetch::route_urls(&mut tools, &resolver);
    let read = tools
        .iter()
        .find(|tool| tool.name() == "read")
        .ok_or("no read tool")?;
    let hex = digest.trim_start_matches("sha256:");
    for (path, want) in [
        (format!("plan://{}/artifacts/{hex}", plan.id), note.as_str()),
        ("family://brief_quota".to_owned(), "the quota brief"),
    ] {
        let input = json!({"path": path})
            .as_object()
            .cloned()
            .unwrap_or_default();
        let output = read.execute(input, &ToolContext::new(rig.temp.to_path_buf()));
        let text = format!("{:?}", output.result.content);
        assert!(
            !output.is_error && text.contains(want.trim()),
            "{path}: {text}"
        );
    }
    Ok(())
}

/// 93 owner start and done calls on engine-owned todos were refused; each is the engine's
/// step, so the owner reads where the todo stands and nothing is journaled.
#[test]
fn an_owner_step_on_an_engine_todo_answers_with_its_standing() -> TestResult {
    let rig = rig("owner")?;
    let plan = opened(
        &rig,
        json!([{"label": "alpha", "delegation": {"spec": {}, "accept": {"command": "true"}}}]),
    )?;
    let records = rig.store.journal(&plan.id).read()?.records.len();
    for (op, verb) in [
        ("start", "starts"),
        ("done", "accepts"),
        ("submit", "submits"),
    ] {
        let args =
            json!({"op": op, "label": "alpha", "attempt": 1, "output": "plan://x/artifacts/00"});
        let args = if op == "submit" {
            args
        } else {
            json!({"op": op, "label": "alpha"})
        };
        let (refused, text) = call(&rig, args);
        assert!(!refused, "{op}: {text}");
        assert!(
            text.contains(&format!("the engine {verb} delegated todos")),
            "{text}"
        );
        assert!(text.contains("running by child-0"), "{text}");
    }
    assert_eq!(rig.store.journal(&plan.id).read()?.records.len(), records);
    Ok(())
}

#[test]
fn view_shows_a_plan_that_has_finished() -> TestResult {
    let rig = rig("view")?;
    let plan = opened(&rig, json!([{"label": "only"}]))?;
    for op in ["start", "done"] {
        let (refused, text) = call(&rig, json!({"op": op, "label": "only"}));
        assert!(!refused, "{text}");
    }
    let (refused, text) = call(&rig, json!({"op": "view"}));
    assert!(!refused, "{text}");
    assert!(text.contains(plan.id.as_str()), "{text}");
    Ok(())
}

#[test]
fn a_phase_refusal_reads_as_a_sentence() -> TestResult {
    let error = PlanOpError::PhaseMissing {
        label: TodoLabel::new("alpha")?,
        phase: "disposition",
        missing: Phase::Disposed.missing().unwrap_or_default(),
    };
    let text = error.to_string();
    assert!(
        text.ends_with("it needs a new attempt: this one was disposed"),
        "{text}"
    );
    assert!(!text.contains("no a "), "{text}");
    Ok(())
}

/// The library's op met `lease ... is held by pid 183`, its own process mid-spawn: an op waits
/// out a short hold instead of refusing.
#[test]
fn an_op_waits_out_a_short_lease_hold() -> TestResult {
    let rig = rig("lease")?;
    let plan = opened(&rig, json!([{"label": "first"}]))?;
    let held = rig.store.lease()?;
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        drop(held);
    });
    rig.engine.apply(OpRequest {
        plan: Some(plan.id.clone()),
        actor: Actor::Owner,
        op: Op::Append {
            todos: vec![yi_runtime::plan::ops::TodoSpec {
                label: TodoLabel::new("second")?,
                after: Vec::new(),
                delegation: None,
                contract: None,
                children: Vec::new(),
                cites: Default::default(),
            }],
        },
        request_id: None,
        expected_revision: None,
    })?;
    release.join().map_err(|_| "release panicked")?;
    Ok(())
}

/// Dies with the accept item left at the manifest default: `cargo test` taking ninety seconds
/// abstains on every finish while the brief promises its exit 0.
#[test]
fn an_accept_command_keeps_the_check_timeout() -> TestResult {
    let rig = rig("accept-timeout")?;
    let plan = opened(
        &rig,
        json!([{"label": "beta", "delegate": {"isolation": "worktree", "spec": {}},
                "accept": {"command": "cargo test"}}]),
    )?;
    let contract = todo_of(&plan, "beta")?
        .contract
        .clone()
        .ok_or("no contract")?;
    let Some(Decider::Cmd {
        checker,
        timeout_ms,
    }) = contract.items.first().map(|item| &item.decider)
    else {
        return Err("not a cmd item".into());
    };
    assert_eq!(*timeout_ms, 600_000);
    let manifest = CheckerManifest::parse(&rig.store.artifacts(&plan.id).get(&checker.digest)?)?;
    assert_eq!(manifest.timeout_ms, 600_000);
    Ok(())
}

struct Gone;

impl yi_runtime::plan::recovery::Liveness for Gone {
    fn alive(&self, _agent: &AgentId) -> Option<bool> {
        Some(false)
    }
}

/// Dies with the standing answered for a child the host no longer holds: its finish never
/// comes, and the owner's own step is swallowed every time.
#[test]
fn an_owner_step_on_a_todo_whose_child_is_gone_takes_the_normal_road() -> TestResult {
    let rig = rig("gone")?;
    let engine = Arc::new(
        PlanEngine::new(rig.store.clone(), Arc::new(Stub::default())).with_liveness(Arc::new(Gone)),
    );
    let gone = Rig {
        tool: PlanTool::new(Arc::clone(&engine), Actor::Owner),
        engine,
        ..rig
    };
    opened(
        &gone,
        json!([{"label": "alpha", "delegation": {"spec": {}, "accept": {"command": "true"}}}]),
    )?;
    let (_refused, text) = call(&gone, json!({"op": "done", "label": "alpha"}));
    assert!(
        !text.contains("the engine accepts delegated todos"),
        "{text}"
    );
    Ok(())
}

/// Dies with the blob store's error returned after the commit: the op is journaled, the
/// scheduling pass is skipped, and a retried init is refused as a plan that exists.
#[test]
fn a_blob_that_cannot_be_stored_leaves_the_op_recorded_and_scheduled() -> TestResult {
    let rig = rig("blob")?;
    let plan = opened(&rig, json!([{"label": "first"}]))?;
    std::fs::write(
        rig.store.plan_dir(&plan.id).join("artifacts"),
        "not a directory",
    )?;
    let (refused, text) = call(
        &rig,
        json!({"op": "append", "todos": [{
            "label": "second",
            "delegation": {"spec": {}, "accept": {"command": "true"}},
            "contract": {"class": "inline", "items": [
                {"id": "check", "critical": true, "weight": 1, "decider": {"cmd": "true"}}
            ]},
        }]}),
    );
    assert!(!refused, "{text}");
    assert!(text.contains("was not stored"), "{text}");
    assert!(
        text.contains("could not start"),
        "the scheduling pass ran: {text}"
    );
    Ok(())
}

/// Seven final-confirmation declarations wrote a stated accept on a worktree delegation and were
/// told only that it needs a contract: the refusal names the command form that declares one.
#[test]
fn a_stated_worktree_accept_is_refused_with_the_command_road() -> TestResult {
    let rig = rig("stated")?;
    let (refused, text) = call(
        &rig,
        json!({"op": "init", "goal": "ship it", "todos": [{
            "label": "alpha",
            "delegation": {"spec": {"isolation": "worktree"}, "accept": {"stated": "alpha.txt holds alpha"}},
        }]}),
    );
    assert!(refused, "{text}");
    assert!(
        text.contains(r#"{"command": "#) && text.contains("alpha"),
        "{text}"
    );
    Ok(())
}

/// The orchestrate protocol as the model reads it: attached by a prompt that asks for a plan.
fn orchestrate_protocol(dir: &std::path::Path) -> Result<String, Box<dyn Error>> {
    let mut host = yi_runtime::ext::install(yi_runtime::ext::ExtOptions {
        cwd: dir.to_path_buf(),
        home: dir.to_path_buf(),
        mode: yi_runtime::PermissionMode::Auto,
        user_system: String::new(),
        schema_instruction: None,
        context_window: 128_000,
        global_skills: Vec::new(),
    });
    host.start(None, false);
    host.dispatch(&host.prompt_event("plan this: split the crate"), None);
    let prompt = host.system_prompt();
    let at = prompt
        .find("# Orchestrate")
        .ok_or("the protocol did not attach")?;
    Ok(prompt.get(at..).ok_or("no protocol text")?.to_owned())
}

/// Dies on `check`: the protocol taught title, acceptance, check and deps, the tool refused
/// each, and a model spent five init calls learning the tool's own names from its errors.
#[test]
fn every_todo_field_the_protocol_teaches_is_one_init_accepts() -> TestResult {
    let rig = rig("protocol-keys")?;
    let protocol = orchestrate_protocol(&rig.temp)?;
    let section = protocol
        .split("## Write the plan")
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .ok_or("no Write the plan section")?;
    let keys: Vec<&str> = section
        .lines()
        .filter_map(|line| line.strip_prefix("- "))
        .filter_map(|line| line.split(':').next())
        .map(|key| key.trim_matches('`'))
        .collect();
    assert!(keys.len() >= 3, "the section names its fields: {section}");
    for key in keys {
        let mut todo = serde_json::Map::new();
        todo.insert("label".to_owned(), json!("probe"));
        todo.insert(key.to_owned(), Value::Null);
        let input = json!({"op": "init", "goal": "g", "todos": [todo]});
        let verdict = rig.tool.validate(input.as_object().ok_or("args")?);
        let refused = verdict.err().unwrap_or_default();
        assert!(
            !refused.contains("does not take"),
            "the protocol teaches `{key}`, which init refuses: {refused}"
        );
    }
    Ok(())
}

/// Dies with no pointer to `decider`: the refusal listed the legal keys and left the model to
/// guess where a check goes.
#[test]
fn a_todo_level_check_is_refused_with_where_it_belongs() -> TestResult {
    let rig = rig("check-hint")?;
    let (refused, text) = call(
        &rig,
        json!({"op": "init", "goal": "g", "todos": [{"label": "t", "check": "cargo test"}]}),
    );
    assert!(refused, "{text}");
    assert!(text.contains("decider: {cmd"), "{text}");
    Ok(())
}

/// Dies on a label past the cap reaching the engine: the schema now carries the cap the
/// parser enforces, so a provider that honours it never sends the 103-character label.
#[test]
fn the_todo_schema_carries_the_parsers_label_cap_and_list_shapes() -> TestResult {
    let rig = rig("schema")?;
    let schema = rig.tool.schema();
    let todo = &schema["properties"]["todos"]["items"]["properties"];
    assert_eq!(
        todo["label"]["maxLength"],
        json!(yi_types::plan::doc::TODO_LABEL_MAX)
    );
    assert_eq!(todo["after"]["type"], json!("array"));
    assert_eq!(todo["intent"]["type"], json!("array"));
    let item = &todo["contract"]["properties"]["items"]["items"];
    assert_eq!(
        item["required"],
        json!(["id", "critical", "weight", "decider"])
    );
    Ok(())
}

/// Dies on the schema's own example: `{cmd: {checker, timeout_ms}}` is how the decider text says
/// to set a check's deadline, and the deadline it names must be the one frozen.
#[test]
fn a_checker_given_as_an_object_keeps_its_deadline() -> TestResult {
    let rig = rig("cmd-object")?;
    let plan = opened(
        &rig,
        json!([{
            "label": "slow",
            "contract": {"class": "inline", "items": [
                {"id": "gate", "critical": true, "weight": 1,
                 "decider": {"cmd": {"checker": "true", "timeout_ms": 900_000}}}
            ]},
        }]),
    )?;
    let contract = todo_of(&plan, "slow")?
        .contract
        .as_ref()
        .ok_or("no contract")?;
    let [item] = contract.items.as_slice() else {
        return Err("one item".into());
    };
    let Decider::Cmd {
        checker,
        timeout_ms,
    } = &item.decider
    else {
        return Err("a cmd decider".into());
    };
    let manifest = CheckerManifest::parse(&rig.store.artifacts(&plan.id).get(&checker.digest)?)?;
    assert_eq!((*timeout_ms, manifest.timeout_ms), (900_000, 900_000));
    Ok(())
}
