#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

use std::error::Error;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};
use yi_kernel::client::HostHandlers;
use yi_runtime::HostRegistry;
use yi_runtime::plan::loop_coupling::{StopPosture, gate, stop_posture};
use yi_runtime::plan::ops::{
    Actor, Delegate, Op, OpRequest, Outcome, PlanEngine, PlanOpError, TodoSpec, dispatch_width,
};
use yi_runtime::plan::store::PlanStore;
use yi_runtime::todo::coupling::Cycle;
use yi_types::plan::doc::{
    AgentId, BlockedOn, Delegation, GoalText, Plan, PlanId, PlanState, PlanTier, TodoAddr,
    TodoLabel, TodoState,
};
use yi_types::url::Url;

type Fallible<T> = Result<T, Box<dyn Error>>;

const FIXTURE_STEMS: [&str; 3] = [
    "arc-game-at-level-grain",
    "campaign-decompose-and-supersede",
    "typo-fix-never-opens-a-plan",
];

const FIXTURE_KEYS: [&str; 11] = [
    "id",
    "description",
    "decisions",
    "prompt",
    "eagerInit",
    "cores",
    "width",
    "goal",
    "plan",
    "steps",
    "final",
];

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plans")
}

#[derive(Default)]
struct Stub {
    serial: AtomicU32,
    fail_reap: AtomicBool,
    produced: Mutex<Option<Url>>,
}

impl Stub {
    fn set_produced(&self, url: Option<Url>) {
        if let Ok(mut slot) = self.produced.lock() {
            *slot = url;
        }
    }
}

impl Delegate for Stub {
    fn spawn(&self, _at: &TodoAddr, _delegation: &Delegation) -> Result<AgentId, String> {
        let serial = self.serial.fetch_add(1, Ordering::SeqCst);
        AgentId::new(format!("walk-child-{serial}")).map_err(|error| error.to_string())
    }

    fn reap(&self, _agent: &AgentId, _supplied: &[Url]) -> Result<Option<Url>, String> {
        if self.fail_reap.load(Ordering::SeqCst) {
            return Err("the fixture said this reap fails".to_owned());
        }
        match self.produced.lock() {
            Ok(produced) => Ok(produced.clone()),
            Err(_) => Err("poisoned".to_owned()),
        }
    }

    fn follow_up(&self, _dispatched: &[TodoLabel], _held: usize) {}
}

fn object<'a>(value: &'a Value, what: &str) -> Fallible<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| format!("{what} is not an object").into())
}

fn text<'a>(value: &'a Value, what: &str) -> Fallible<&'a str> {
    value
        .as_str()
        .ok_or_else(|| format!("{what} is not a string").into())
}

fn boolean(value: &Value, what: &str) -> Fallible<bool> {
    value
        .as_bool()
        .ok_or_else(|| format!("{what} is not a bool").into())
}

fn require<'a>(map: &'a Map<String, Value>, key: &str, what: &str) -> Fallible<&'a Value> {
    map.get(key)
        .ok_or_else(|| format!("{what} is missing key {key:?}").into())
}

fn reject_unknown(map: &Map<String, Value>, allowed: &[&str], what: &str) -> Fallible<()> {
    for key in map.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!("{what} has unknown key {key:?}").into());
        }
    }
    Ok(())
}

fn parse_label(value: &Value, what: &str) -> Fallible<TodoLabel> {
    Ok(TodoLabel::new(text(value, what)?)?)
}

fn parse_url(value: &Value, what: &str) -> Fallible<Url> {
    Ok(text(value, what)?.parse::<Url>()?)
}

fn parse_labels(value: &Value, what: &str) -> Fallible<Vec<TodoLabel>> {
    let list = value
        .as_array()
        .ok_or_else(|| format!("{what} is not an array"))?;
    list.iter().map(|entry| parse_label(entry, what)).collect()
}

fn parse_specs(value: &Value, what: &str) -> Fallible<Vec<TodoSpec>> {
    let list = value
        .as_array()
        .ok_or_else(|| format!("{what} is not an array"))?;
    let mut specs = Vec::new();
    for entry in list {
        let map = object(entry, what)?;
        reject_unknown(map, &["label", "after", "delegation", "repeat"], what)?;
        let label = text(require(map, "label", what)?, what)?;
        let after = match map.get("after") {
            Some(edges) => parse_labels(edges, what)?,
            None => Vec::new(),
        };
        let delegation = match map.get("delegation") {
            Some(raw) => Some(serde_json::from_value::<Delegation>(raw.clone())?),
            None => None,
        };
        match map.get("repeat") {
            None => specs.push(TodoSpec {
                label: TodoLabel::new(label)?,
                after,
                delegation,
                contract: None,
                children: Vec::new(),
            }),
            Some(raw) => {
                let count = raw
                    .as_u64()
                    .ok_or_else(|| format!("{what} repeat is not an integer"))?;
                for serial in 1..=count {
                    specs.push(TodoSpec {
                        label: TodoLabel::new(format!("{label} {serial}"))?,
                        after: after.clone(),
                        delegation: delegation.clone(),
                        contract: None,
                        children: Vec::new(),
                    });
                }
            }
        }
    }
    Ok(specs)
}

fn parse_actor(raw: &str) -> Fallible<Actor> {
    if raw == "host" {
        return Ok(Actor::Host);
    }
    if raw.starts_with("user://") {
        return Ok(Actor::User(raw.parse::<Url>()?));
    }
    if let Some(rest) = raw.strip_prefix("agent://") {
        if rest == "main" {
            return Ok(Actor::Owner);
        }
        return Ok(Actor::Child(AgentId::new(rest)?));
    }
    Err(format!("unknown actor {raw:?}").into())
}

fn parse_op(name: &str, args: &Map<String, Value>) -> Fallible<(Op, bool)> {
    let what = format!("args of {name}");
    let label =
        |key: &str| -> Fallible<TodoLabel> { parse_label(require(args, key, &what)?, &what) };
    let string =
        |key: &str| -> Fallible<String> { Ok(text(require(args, key, &what)?, &what)?.to_owned()) };
    let op = match name {
        "init" => {
            reject_unknown(args, &["goal", "todos"], &what)?;
            Op::Init {
                goal: GoalText::new(string("goal")?)?,
                todos: parse_specs(require(args, "todos", &what)?, &what)?,
            }
        }
        "append" => {
            reject_unknown(args, &["todos"], &what)?;
            Op::Append {
                todos: parse_specs(require(args, "todos", &what)?, &what)?,
            }
        }
        "drop" => {
            reject_unknown(args, &["label"], &what)?;
            Op::Drop {
                label: label("label")?,
                disposition: None,
            }
        }
        "block" => {
            reject_unknown(args, &["label", "on", "note"], &what)?;
            Op::Block {
                label: label("label")?,
                on: serde_json::from_value::<BlockedOn>(require(args, "on", &what)?.clone())?,
                note: string("note")?,
            }
        }
        "unblock" => {
            reject_unknown(args, &["label"], &what)?;
            Op::Unblock {
                label: label("label")?,
            }
        }
        "reorder" => {
            reject_unknown(args, &["labels"], &what)?;
            Op::Reorder {
                labels: parse_labels(require(args, "labels", &what)?, &what)?,
            }
        }
        "add_edge" => {
            reject_unknown(args, &["todo", "after"], &what)?;
            Op::AddEdge {
                todo: label("todo")?,
                after: label("after")?,
            }
        }
        "start" => {
            reject_unknown(args, &["label"], &what)?;
            Op::Start {
                label: label("label")?,
            }
        }
        "done" => {
            reject_unknown(args, &["label", "output"], &what)?;
            let output = match args.get("output") {
                None | Some(Value::Null) => None,
                Some(raw) => Some(parse_url(raw, &what)?),
            };
            Op::Done {
                label: label("label")?,
                output,
            }
        }
        "fail" => {
            reject_unknown(args, &["label", "cause"], &what)?;
            Op::Fail {
                label: label("label")?,
                cause: string("cause")?,
                disposition: None,
            }
        }
        "retry" => {
            reject_unknown(args, &["label", "delegation"], &what)?;
            let delegation = match args.get("delegation") {
                Some(raw) => Some(Box::new(serde_json::from_value::<Delegation>(raw.clone())?)),
                None => None,
            };
            Op::Retry {
                label: label("label")?,
                delegation,
            }
        }
        "decompose" => {
            reject_unknown(args, &["label", "todos"], &what)?;
            Op::Decompose {
                label: label("label")?,
                todos: parse_specs(require(args, "todos", &what)?, &what)?,
            }
        }
        "supersede" => {
            reject_unknown(args, &["reason", "todos", "reapFails"], &what)?;
            let reap_fails = match args.get("reapFails") {
                Some(raw) => boolean(raw, &what)?,
                None => false,
            };
            return Ok((
                Op::Supersede {
                    reason: string("reason")?,
                    todos: parse_specs(require(args, "todos", &what)?, &what)?,
                },
                reap_fails,
            ));
        }
        "view" => {
            reject_unknown(args, &["full"], &what)?;
            let full = match args.get("full") {
                Some(raw) => boolean(raw, &what)?,
                None => false,
            };
            Op::View { full }
        }
        other => return Err(format!("unknown op {other:?}").into()),
    };
    Ok((op, false))
}

fn todo_state_tag(state: &TodoState) -> String {
    match state {
        TodoState::Pending => "pending".to_owned(),
        TodoState::Running { .. } => "running".to_owned(),
        TodoState::Blocked { .. } => "blocked".to_owned(),
        TodoState::Done { .. } => "done".to_owned(),
        TodoState::Failed { .. } => "failed".to_owned(),
        TodoState::Abandoned => "abandoned".to_owned(),
        TodoState::Other(tag) => tag.clone(),
    }
}

fn plan_state_tag(state: &PlanState) -> String {
    match state {
        PlanState::Active => "active".to_owned(),
        PlanState::Done => "done".to_owned(),
        PlanState::Superseded { .. } => "superseded".to_owned(),
        PlanState::Abandoned => "abandoned".to_owned(),
        PlanState::Other(tag) => tag.clone(),
    }
}

fn blocked_tag(on: &BlockedOn) -> String {
    match on {
        BlockedOn::Child(_) => "child".to_owned(),
        BlockedOn::User => "user".to_owned(),
        BlockedOn::External { .. } => "external".to_owned(),
        BlockedOn::Other(tag) => tag.clone(),
    }
}

fn tier_tag(tier: &PlanTier) -> String {
    match tier {
        PlanTier::Root => "root".to_owned(),
        PlanTier::Sub { .. } => "sub".to_owned(),
        PlanTier::Other { tier, .. } => tier.clone(),
    }
}

fn root_id(id: &PlanId) -> Fallible<PlanId> {
    match id.as_str().split_once('.') {
        Some((root, _)) => Ok(PlanId::new(root)?),
        None => Ok(id.clone()),
    }
}

fn active_root(store: &PlanStore) -> Fallible<Option<PlanId>> {
    for id in store.roots()? {
        if store.read(&id)?.state == PlanState::Active {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

fn cmp_u64(
    ctx: &str,
    key: &str,
    value: &Value,
    got: u64,
    failures: &mut Vec<String>,
) -> Fallible<()> {
    let want = value
        .as_u64()
        .ok_or_else(|| format!("{ctx} {key} is not an integer"))?;
    if want != got {
        failures.push(format!("{ctx} {key}: expected {want}, got {got}"));
    }
    Ok(())
}

fn cmp_str(
    ctx: &str,
    key: &str,
    value: &Value,
    got: &str,
    failures: &mut Vec<String>,
) -> Fallible<()> {
    let want = text(value, key)?;
    if want != got {
        failures.push(format!("{ctx} {key}: expected {want:?}, got {got:?}"));
    }
    Ok(())
}

fn cmp_list(
    ctx: &str,
    key: &str,
    value: &Value,
    got: &[String],
    failures: &mut Vec<String>,
) -> Fallible<()> {
    let list = value
        .as_array()
        .ok_or_else(|| format!("{ctx} {key} is not an array"))?;
    let mut want = Vec::new();
    for entry in list {
        want.push(text(entry, key)?.to_owned());
    }
    if want != got {
        failures.push(format!("{ctx} {key}: expected {want:?}, got {got:?}"));
    }
    Ok(())
}

fn cmp_opt_url(
    ctx: &str,
    key: &str,
    value: &Value,
    got: Option<&Url>,
    failures: &mut Vec<String>,
) -> Fallible<()> {
    match (value, got) {
        (Value::Null, None) => {}
        (Value::Null, Some(url)) => {
            failures.push(format!("{ctx} {key}: expected null, got {url}"));
        }
        (raw, None) => {
            failures.push(format!(
                "{ctx} {key}: expected {}, got null",
                text(raw, key)?
            ));
        }
        (raw, Some(url)) => {
            let want = text(raw, key)?;
            let rendered = url.to_string();
            if want != rendered {
                failures.push(format!("{ctx} {key}: expected {want:?}, got {rendered:?}"));
            }
        }
    }
    Ok(())
}

const TODO_KEYS: [&str; 8] = [
    "label",
    "state",
    "output",
    "last",
    "retries",
    "subplan",
    "delegated",
    "blockedOn",
];

fn check_todos(ctx: &str, value: &Value, plan: &Plan, failures: &mut Vec<String>) -> Fallible<()> {
    let list = value
        .as_array()
        .ok_or_else(|| format!("{ctx} todos is not an array"))?;
    if list.len() != plan.todos.len() {
        failures.push(format!(
            "{ctx} todos: expected {} entries, plan has {}",
            list.len(),
            plan.todos.len()
        ));
    }
    for (position, entry) in list.iter().enumerate() {
        let tctx = format!("{ctx} todos[{position}]");
        let map = object(entry, &tctx)?;
        reject_unknown(map, &TODO_KEYS, &tctx)?;
        let Some(todo) = plan.todos.get(position) else {
            failures.push(format!("{tctx} expected, but the plan ends sooner"));
            continue;
        };
        if let Some(raw) = map.get("label") {
            cmp_str(&tctx, "label", raw, todo.label.as_str(), failures)?;
        }
        if let Some(raw) = map.get("state") {
            cmp_str(&tctx, "state", raw, &todo_state_tag(&todo.state), failures)?;
        }
        if let Some(raw) = map.get("output") {
            match &todo.state {
                TodoState::Done { output, .. } => {
                    cmp_opt_url(&tctx, "output", raw, output.as_ref(), failures)?;
                }
                other => failures.push(format!(
                    "{tctx} output asserted on state {:?}",
                    todo_state_tag(other)
                )),
            }
        }
        if let Some(raw) = map.get("last") {
            match &todo.state {
                TodoState::Failed { last, .. } => {
                    cmp_opt_url(&tctx, "last", raw, last.as_ref(), failures)?;
                }
                other => failures.push(format!(
                    "{tctx} last asserted on state {:?}",
                    todo_state_tag(other)
                )),
            }
        }
        if let Some(raw) = map.get("retries") {
            cmp_u64(&tctx, "retries", raw, u64::from(todo.retries.0), failures)?;
        }
        if let Some(raw) = map.get("subplan") {
            match &todo.subplan {
                Some(id) => cmp_str(&tctx, "subplan", raw, id.as_str(), failures)?,
                None => failures.push(format!("{tctx} subplan: expected {raw}, the todo has none")),
            }
        }
        if let Some(raw) = map.get("delegated") {
            let want = boolean(raw, &tctx)?;
            let got = todo.delegation.is_some();
            if want != got {
                failures.push(format!("{tctx} delegated: expected {want}, got {got}"));
            }
        }
        if let Some(raw) = map.get("blockedOn") {
            match &todo.state {
                TodoState::Blocked { on, .. } => {
                    cmp_str(&tctx, "blockedOn", raw, &blocked_tag(on), failures)?;
                }
                other => failures.push(format!(
                    "{tctx} blockedOn asserted on state {:?}",
                    todo_state_tag(other)
                )),
            }
        }
    }
    Ok(())
}

fn check_plan_key(
    ctx: &str,
    key: &str,
    value: &Value,
    file: Option<&Plan>,
    store: &PlanStore,
    failures: &mut Vec<String>,
) -> Fallible<()> {
    let Some(file) = file else {
        failures.push(format!("{ctx} {key} asserted but no plan exists on disk"));
        return Ok(());
    };
    let plan = &file;
    match key {
        "version" => cmp_u64(ctx, key, value, plan.version.0, failures),
        "touched" => cmp_u64(ctx, key, value, plan.touched.0, failures),
        "spawns" => {
            let root = root_id(&plan.id)?;
            let spawns = u64::from(store.read(&root)?.spawns().get());
            cmp_u64(ctx, key, value, spawns, failures)
        }
        "planState" => cmp_str(ctx, key, value, &plan_state_tag(&plan.state), failures),
        "tier" => cmp_str(ctx, key, value, &tier_tag(&plan.tier), failures),
        "parent" => match &plan.tier {
            PlanTier::Sub { parent }
            | PlanTier::Other {
                parent: Some(parent),
                ..
            } => cmp_str(ctx, key, value, &parent.to_string(), failures),
            PlanTier::Root | PlanTier::Other { parent: None, .. } => {
                failures.push(format!("{ctx} parent asserted on a plan with no parent"));
                Ok(())
            }
        },
        "todos" => check_todos(ctx, value, plan, failures),
        other => Err(format!("{ctx} {other:?} is not a plan-file assertion").into()),
    }
}

fn check_outcome_key(
    ctx: &str,
    key: &str,
    value: &Value,
    outcome: Option<&Outcome>,
    store: &PlanStore,
    failures: &mut Vec<String>,
) -> Fallible<()> {
    let Some(outcome) = outcome else {
        failures.push(format!("{ctx} {key} asserted on a refused op"));
        return Ok(());
    };
    let labels = |list: &[TodoLabel]| -> Vec<String> {
        list.iter().map(|label| label.as_str().to_owned()).collect()
    };
    let urls = |list: &[Url]| -> Vec<String> { list.iter().map(Url::to_string).collect() };
    match key {
        "ready" => cmp_list(ctx, key, value, &labels(&outcome.ready), failures),
        "dispatched" => cmp_list(ctx, key, value, &labels(&outcome.dispatched), failures),
        "held" => cmp_list(ctx, key, value, &labels(&outcome.held), failures),
        "spawned" => cmp_list(ctx, key, value, &urls(&outcome.spawned), failures),
        "reaped" => cmp_list(ctx, key, value, &urls(&outcome.reaped), failures),
        "subplan" => {
            let want = text(value, key)?;
            match &outcome.subplan {
                Some(id) if id.as_str() == want => {}
                other => failures.push(format!("{ctx} subplan: expected {want:?}, got {other:?}")),
            }
            Ok(())
        }
        "subplanStates" => {
            let map = object(value, key)?;
            for (id, raw) in map {
                let want = text(raw, key)?;
                let got = plan_state_tag(&store.read(&PlanId::new(id.as_str())?)?.state);
                if want != got {
                    failures.push(format!(
                        "{ctx} subplanStates[{id}]: expected {want:?}, got {got:?}"
                    ));
                }
            }
            Ok(())
        }
        other => Err(format!("{ctx} {other:?} is not an outcome assertion").into()),
    }
}

fn check_expect(
    ctx: &str,
    expect: &Map<String, Value>,
    result: &Result<Outcome, PlanOpError>,
    store: &PlanStore,
    step_plan: Option<&PlanId>,
    failures: &mut Vec<String>,
) -> Fallible<()> {
    let expect_ok = boolean(require(expect, "ok", ctx)?, ctx)?;
    match (expect_ok, result) {
        (true, Err(error)) => {
            failures.push(format!("{ctx} expected ok, got error: {error}"));
            return Ok(());
        }
        (false, Ok(_)) => {
            failures.push(format!("{ctx} expected a refusal, but the op succeeded"));
            return Ok(());
        }
        (true, Ok(_)) | (false, Err(_)) => {}
    }
    if let Some(raw) = expect.get("error") {
        let want = text(raw, "error")?;
        if let Err(error) = result {
            let got = error.to_string();
            if !got.contains(want) {
                failures.push(format!("{ctx} error {got:?} does not contain {want:?}"));
            }
        }
    }
    let target = match result {
        Ok(outcome) => Some(outcome.plan.id.clone()),
        Err(_) => match step_plan {
            Some(id) => Some(id.clone()),
            None => active_root(store)?,
        },
    };
    let file = match &target {
        Some(id) => Some(store.read(id)?),
        None => None,
    };
    let outcome = result.as_ref().ok();
    for (key, value) in expect {
        match key.as_str() {
            "ok" | "error" => {}
            "version" | "touched" | "spawns" | "planState" | "tier" | "parent" | "todos" => {
                check_plan_key(ctx, key, value, file.as_ref(), store, failures)?;
            }
            "ready" | "dispatched" | "held" | "spawned" | "reaped" | "subplan"
            | "subplanStates" => {
                check_outcome_key(ctx, key, value, outcome, store, failures)?;
            }
            "stopPosture" | "stopInterception" => {
                let posture = file.as_ref().map(stop_posture);
                match key.as_str() {
                    "stopPosture" => {
                        let got = posture.map_or("quiet", StopPosture::as_str);
                        cmp_str(ctx, key, value, got, failures)?;
                    }
                    _ => {
                        let want = boolean(value, key)?;
                        let got = posture == Some(StopPosture::Continue);
                        if want != got {
                            failures.push(format!("{ctx} {key}: expected {want}, got {got}"));
                        }
                    }
                }
            }
            other => return Err(format!("{ctx} unknown assertion key {other:?}").into()),
        }
    }
    Ok(())
}

fn check_work_step(
    ctx: &str,
    step: &Map<String, Value>,
    nudge: &mut Cycle,
    failures: &mut Vec<String>,
) -> Fallible<()> {
    reject_unknown(step, &["work", "expect"], ctx)?;
    let work = object(require(step, "work", ctx)?, ctx)?;
    reject_unknown(work, &["mutatingCalls", "kernelActions"], ctx)?;
    let calls = require(work, "mutatingCalls", ctx)?
        .as_u64()
        .ok_or_else(|| format!("{ctx} mutatingCalls is not an integer"))?;
    let fired = nudge.work(u32::try_from(calls)?);
    let expect = object(require(step, "expect", ctx)?, ctx)?;
    for (key, value) in expect {
        match key.as_str() {
            "nudge" => {
                let want = boolean(value, key)?;
                if want != fired {
                    failures.push(format!("{ctx} nudge: expected {want}, got {fired}"));
                }
            }
            "nudgesThisCycle" => cmp_u64(ctx, key, value, u64::from(nudge.nudges), failures)?,
            other => return Err(format!("{ctx} unknown work assertion {other:?}").into()),
        }
    }
    Ok(())
}

fn check_final(
    stem: &str,
    doc: &Map<String, Value>,
    store: &PlanStore,
    failures: &mut Vec<String>,
) -> Fallible<()> {
    let ctx = format!("[{stem} final]");
    let map = object(require(doc, "final", "fixture")?, &ctx)?;
    reject_unknown(
        map,
        &["planCount", "stopInterception", "superseded", "spawns"],
        &ctx,
    )?;
    let ids = store.list()?;
    if let Some(raw) = map.get("planCount") {
        cmp_u64(&ctx, "planCount", raw, u64::try_from(ids.len())?, failures)?;
    }
    if let Some(raw) = map.get("superseded") {
        let mut total = 0u64;
        for id in &ids {
            total = total.saturating_add(store.read(id)?.version.0.saturating_sub(1));
        }
        cmp_u64(&ctx, "superseded", raw, total, failures)?;
    }
    if let Some(raw) = map.get("spawns") {
        let mut total = 0u64;
        for id in &ids {
            if id.is_root() {
                total = total.saturating_add(u64::from(store.read(id)?.spawns().get()));
            }
        }
        cmp_u64(&ctx, "spawns", raw, total, failures)?;
    }
    if let Some(raw) = map.get("stopInterception") {
        let want = boolean(raw, "stopInterception")?;
        let got = match active_root(store)? {
            Some(id) => stop_posture(&store.read(&id)?) == StopPosture::Continue,
            None => false,
        };
        if want != got {
            failures.push(format!(
                "{ctx} stopInterception: expected {want}, got {got}"
            ));
        }
    }
    Ok(())
}

fn build_engine(
    doc: &Map<String, Value>,
    store: &PlanStore,
    stub: &Arc<Stub>,
    failures: &mut Vec<String>,
) -> Fallible<PlanEngine> {
    let engine = PlanEngine::new(store.clone(), stub.clone());
    let Some(raw) = doc.get("cores") else {
        if doc.get("width").is_some() {
            return Err("fixture pins width without cores".into());
        }
        return Ok(engine);
    };
    let cores = raw
        .as_u64()
        .ok_or_else(|| "cores is not an integer".to_owned())?;
    let cores =
        NonZeroUsize::new(usize::try_from(cores)?).ok_or_else(|| "cores is zero".to_owned())?;
    let width = dispatch_width(cores);
    if let Some(raw) = doc.get("width") {
        cmp_u64(
            "[fixture]",
            "width",
            raw,
            u64::try_from(width.get())?,
            failures,
        )?;
    }
    Ok(engine.with_width(width))
}

fn run_fixture(stem: &str) -> Fallible<()> {
    let path = fixtures_dir().join(format!("{stem}.json"));
    let raw = std::fs::read_to_string(&path)?;
    let doc: Value = serde_json::from_str(&raw)?;
    let doc = object(&doc, "fixture")?;
    reject_unknown(doc, &FIXTURE_KEYS, "fixture")?;
    if text(require(doc, "id", "fixture")?, "fixture id")? != stem {
        return Err(format!("fixture id does not match file stem {stem:?}").into());
    }
    let eager = boolean(require(doc, "eagerInit", "fixture")?, "eagerInit")?;
    let dir = Scratch::new(&format!("yi-plan-walkthrough-{stem}"))?;
    let store = PlanStore::open(dir.to_path_buf())?;
    let stub = Arc::new(Stub::default());
    let mut failures: Vec<String> = Vec::new();
    let prompt = text(require(doc, "prompt", "fixture")?, "prompt")?;
    if gate::eager_init(prompt) != eager {
        failures.push(format!(
            "[{stem}] the eager-init gate said {}, the fixture pins {eager}",
            !eager
        ));
    }
    let mut nudge = Cycle::default();
    let engine = build_engine(doc, &store, &stub, &mut failures)?;
    let steps = require(doc, "steps", "fixture")?
        .as_array()
        .ok_or_else(|| "steps is not an array".to_owned())?;
    let mut refused_non_init = false;
    let mut init_checked = false;
    for (index, raw_step) in steps.iter().enumerate() {
        let ctx = format!("[{stem} step {index}]");
        let step = object(raw_step, &ctx)?;
        if step.contains_key("work") {
            check_work_step(&ctx, step, &mut nudge, &mut failures)?;
            continue;
        }
        reject_unknown(
            step,
            &["op", "actor", "plan", "args", "expect", "childProduced"],
            &ctx,
        )?;
        let name = text(require(step, "op", &ctx)?, &ctx)?;
        let actor = parse_actor(text(require(step, "actor", &ctx)?, &ctx)?)?;
        let plan = match step.get("plan") {
            Some(raw) => Some(PlanId::new(text(raw, &ctx)?)?),
            None => None,
        };
        let args = match step.get("args") {
            Some(raw) => object(raw, &ctx)?.clone(),
            None => Map::new(),
        };
        let (op, reap_fails) = parse_op(name, &args)?;
        if let Some(raw) = step.get("childProduced") {
            stub.set_produced(Some(parse_url(raw, &ctx)?));
        }
        stub.fail_reap.store(reap_fails, Ordering::SeqCst);
        let is_init = matches!(op, Op::Init { .. });
        let result = engine.apply(OpRequest {
            plan: plan.clone(),
            actor,
            op,
            request_id: None,
            expected_revision: None,
        });
        stub.fail_reap.store(false, Ordering::SeqCst);
        stub.set_produced(None);
        if result.is_ok() {
            nudge.touched();
        }
        if !is_init && result.is_err() {
            refused_non_init = true;
        }
        if is_init
            && !init_checked
            && let Ok(outcome) = &result
        {
            init_checked = true;
            if let Some(raw) = doc.get("goal") {
                cmp_str(&ctx, "goal", raw, outcome.plan.goal.as_str(), &mut failures)?;
            }
            if let Some(raw) = doc.get("plan") {
                cmp_str(&ctx, "plan", raw, outcome.plan.id.as_str(), &mut failures)?;
            }
        }
        let expect = object(require(step, "expect", &ctx)?, &ctx)?;
        check_expect(&ctx, expect, &result, &store, plan.as_ref(), &mut failures)?;
        if !eager && !store.list()?.is_empty() {
            failures.push(format!(
                "{ctx} eagerInit is false but a plan file was created"
            ));
        }
    }
    if !eager && !refused_non_init {
        failures.push(format!(
            "[{stem}] eagerInit is false but no non-init op was refused"
        ));
    }
    if (doc.contains_key("goal") || doc.contains_key("plan")) && !init_checked {
        failures.push(format!(
            "[{stem}] fixture names a goal/plan identity but no init succeeded"
        ));
    }
    check_final(stem, doc, &store, &mut failures)?;
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} fixture assertion(s) failed:\n{}",
            failures.len(),
            failures.join("\n")
        )
        .into())
    }
}

#[test]
fn arc_game_at_level_grain() -> Fallible<()> {
    run_fixture("arc-game-at-level-grain")
}

#[test]
fn campaign_decompose_and_supersede() -> Fallible<()> {
    run_fixture("campaign-decompose-and-supersede")
}

#[test]
fn typo_fix_never_opens_a_plan() -> Fallible<()> {
    run_fixture("typo-fix-never-opens-a-plan")
}

#[test]
fn every_fixture_has_a_named_runner() -> Fallible<()> {
    let mut stems = Vec::new();
    for entry in std::fs::read_dir(fixtures_dir())? {
        let path = entry?.path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
            stems.push(
                path.file_stem()
                    .and_then(|stem| stem.to_str())
                    .ok_or_else(|| format!("unreadable fixture name at {}", path.display()))?
                    .to_owned(),
            );
        }
    }
    stems.sort();
    for stem in &stems {
        if !FIXTURE_STEMS.contains(&stem.as_str()) {
            return Err(format!(
                "fixture {stem}.json exists but no #[test] drives it; add a runner naming it"
            )
            .into());
        }
    }
    for stem in FIXTURE_STEMS {
        if !stems.iter().any(|found| found == stem) {
            return Err(format!("expected fixture {stem}.json is missing").into());
        }
    }
    Ok(())
}

/// Every `.json` under `fixtures/plans`, at any depth, is opened by a test: its file name or
/// its quoted stem sits on a non-comment line of some file in `tests/`. A `.example.json` is
/// documentation, named by `contracts.md`, and is exempt. A fixture nothing reads rots unseen.
#[test]
fn every_fixture_at_any_depth_is_named_by_a_test_source() -> Fallible<()> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Fallible<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                walk(&path, out)?;
            } else if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
                out.push(path);
            }
        }
        Ok(())
    }
    let mut fixtures = Vec::new();
    walk(&fixtures_dir(), &mut fixtures)?;
    let mut sources = String::new();
    for entry in std::fs::read_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests"))? {
        let path = entry?.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        for line in std::fs::read_to_string(&path)?.lines() {
            if !line.trim_start().starts_with("//") {
                sources.push_str(line);
                sources.push('\n');
            }
        }
    }
    for path in &fixtures {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("unreadable fixture name at {}", path.display()))?;
        if name.ends_with(".example.json") {
            continue;
        }
        let stem = name.trim_end_matches(".json");
        if !sources.contains(name) && !sources.contains(&format!("\"{stem}\"")) {
            return Err(format!(
                "fixture {} exists but no test source names it; add a test that reads it",
                path.display()
            )
            .into());
        }
    }
    Ok(())
}

/// The fixture's step args in the shape the plan tool takes: `reapFails` is the driver's,
/// and `repeat` is the fixture's shorthand the tool has no word for.
fn tool_args(args: &Map<String, Value>) -> Fallible<Map<String, Value>> {
    let mut out = args.clone();
    out.remove("reapFails");
    if let Some(Value::Array(todos)) = out.get("todos") {
        let mut expanded = Vec::new();
        for entry in todos {
            let spec = object(entry, "todo spec")?;
            let Some(raw) = spec.get("repeat") else {
                expanded.push(entry.clone());
                continue;
            };
            let count = raw
                .as_u64()
                .ok_or_else(|| "repeat is not an integer".to_owned())?;
            let label = text(require(spec, "label", "todo spec")?, "todo spec")?;
            for serial in 1..=count {
                let mut one = spec.clone();
                one.remove("repeat");
                one.insert(
                    "label".to_owned(),
                    Value::String(format!("{label} {serial}")),
                );
                expanded.push(Value::Object(one));
            }
        }
        out.insert("todos".to_owned(), Value::Array(expanded));
    }
    Ok(out)
}

/// The checkpoints, with the journal digest and the clock blanked: two runs of one fixture
/// differ in wall-clock `at` fields and request ids, which the digest chain covers.
fn plan_files(store: &PlanStore) -> Fallible<Vec<(String, String)>> {
    let mut files = Vec::new();
    for id in store.list()? {
        let mut value: Value = serde_json::from_str(&std::fs::read_to_string(store.path(&id))?)?;
        if let Some(map) = value.as_object_mut() {
            map.insert("journal_digest".to_owned(), Value::Null);
        }
        files.push((
            id.as_str().to_owned(),
            serde_json::to_string_pretty(&value)?,
        ));
    }
    files.sort();
    Ok(files)
}

/// One fixture down both surfaces, step for step: the engine as the tool drives it, and
/// `plan.op` on a registry per fixture actor. Same refusals, same bytes on disk.
async fn replay_through_plan_op(stem: &str) -> Fallible<()> {
    let path = fixtures_dir().join(format!("{stem}.json"));
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    let doc = object(&doc, "fixture")?;
    let tool_dir = Scratch::new(&format!("yi-plan-op-tool-{stem}"))?;
    let request_dir = Scratch::new(&format!("yi-plan-op-request-{stem}"))?;
    let tool_store = PlanStore::open(tool_dir.to_path_buf())?;
    let request_store = PlanStore::open(request_dir.to_path_buf())?;
    let tool_stub = Arc::new(Stub::default());
    let request_stub = Arc::new(Stub::default());
    let mut failures = Vec::new();
    let tool_engine = build_engine(doc, &tool_store, &tool_stub, &mut failures)?;
    let request_engine = Arc::new(build_engine(
        doc,
        &request_store,
        &request_stub,
        &mut failures,
    )?);
    let mut registries: std::collections::HashMap<String, HostRegistry> =
        std::collections::HashMap::new();
    let steps = require(doc, "steps", "fixture")?
        .as_array()
        .ok_or_else(|| "steps is not an array".to_owned())?;
    for (index, raw_step) in steps.iter().enumerate() {
        let ctx = format!("[{stem} step {index}]");
        let step = object(raw_step, &ctx)?;
        if step.contains_key("work") {
            continue;
        }
        let name = text(require(step, "op", &ctx)?, &ctx)?;
        let actor_word = text(require(step, "actor", &ctx)?, &ctx)?;
        let actor = parse_actor(actor_word)?;
        let plan = match step.get("plan") {
            Some(raw) => Some(PlanId::new(text(raw, &ctx)?)?),
            None => None,
        };
        let args = match step.get("args") {
            Some(raw) => object(raw, &ctx)?.clone(),
            None => Map::new(),
        };
        let (op, reap_fails) = parse_op(name, &args)?;
        let produced = match step.get("childProduced") {
            Some(raw) => Some(parse_url(raw, &ctx)?),
            None => None,
        };
        for stub in [&tool_stub, &request_stub] {
            stub.set_produced(produced.clone());
            stub.fail_reap.store(reap_fails, Ordering::SeqCst);
        }
        let through_tool = tool_engine.apply(OpRequest {
            plan: plan.clone(),
            actor: actor.clone(),
            op,
            request_id: None,
            expected_revision: None,
        });
        let registry = registries.entry(actor_word.to_owned()).or_insert_with(|| {
            let mut registry = HostRegistry::default();
            yi_runtime::plan::request::register(
                Arc::clone(&request_engine),
                actor.clone(),
                &mut registry,
            );
            registry
        });
        let mut payload = Map::new();
        payload.insert(
            "request_id".to_owned(),
            Value::String(format!("{stem}-{index}")),
        );
        payload.insert("op".to_owned(), Value::String(name.to_owned()));
        if let Some(id) = &plan {
            payload.insert("plan".to_owned(), Value::String(id.as_str().to_owned()));
        }
        payload.insert("args".to_owned(), Value::Object(tool_args(&args)?));
        let through_request = registry
            .dispatch("plan.op", payload)
            .ok_or("plan.op is not registered")?
            .await?;
        for stub in [&tool_stub, &request_stub] {
            stub.set_produced(None);
            stub.fail_reap.store(false, Ordering::SeqCst);
        }
        if through_tool.is_ok() != (through_request["ok"] == Value::Bool(true)) {
            failures.push(format!(
                "{ctx} the tool said {:?}, plan.op said {through_request:?}",
                through_tool.as_ref().err().map(ToString::to_string)
            ));
        }
        if let Ok(outcome) = &through_tool
            && through_request["revision"] != outcome.plan.touched.0
        {
            failures.push(format!(
                "{ctx} revision {} differs from touched {}",
                through_request["revision"], outcome.plan.touched.0
            ));
        }
    }
    let tool_files = plan_files(&tool_store)?;
    let request_files = plan_files(&request_store)?;
    if tool_files != request_files {
        failures.push(format!(
            "[{stem}] plan files differ:\n{tool_files:#?}\n{request_files:#?}"
        ));
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n").into())
    }
}

/// Guards the shared parser and engine: a `plan.op` that parsed or applied differently from
/// the tool leaves different bytes in `.yi/plans`.
#[tokio::test]
async fn every_fixture_replays_identically_through_plan_op() -> Fallible<()> {
    for stem in FIXTURE_STEMS {
        replay_through_plan_op(stem).await?;
    }
    Ok(())
}

/// What a fixture and its program are compared on: every plan's state and version, its todos in
/// order (labels by slug, since a program's key is the label in lower case) with their edges,
/// attempt, retries, whether they are delegated and what a done one output, and the committed
/// transitions in journal order.
///
/// What is not: `refusals` and `touched`, because the fixture's refused steps are its own; the
/// text of a cause or a block note; a delegation's or a contract's contents; and the campaign
/// fixture's `reorder` and `add_edge`, which the library has no surface for and whose generation
/// its `supersede` closes before this reads the store.
fn reached(store: &PlanStore) -> Fallible<Vec<String>> {
    const TRANSITIONS: [&str; 9] = [
        "start",
        "done",
        "fail",
        "retry",
        "block",
        "unblock",
        "drop",
        "decompose",
        "supersede",
    ];
    let slug = |label: &TodoLabel| PlanId::slug(label.as_str()).map(|id| id.as_str().to_owned());
    let mut lines = Vec::new();
    for id in store.list()? {
        let plan = store.read(&id)?;
        lines.push(format!(
            "{id} {} v{}",
            plan_state_tag(&plan.state),
            plan.version.0
        ));
        for todo in &plan.todos {
            let after: Vec<String> = todo.after.iter().map(slug).collect::<Result<_, _>>()?;
            let output = match &todo.state {
                TodoState::Done { output, .. } => output.as_ref().map(ToString::to_string),
                _ => None,
            };
            lines.push(format!(
                "  {} {} after {after:?} attempt {} retries {} delegated {} output {output:?}",
                slug(&todo.label)?,
                todo_state_tag(&todo.state),
                todo.attempt.get(),
                todo.retries.0,
                todo.delegation.is_some()
            ));
        }
    }
    for root in store.roots()? {
        for record in store.journal(&root).read()?.records {
            let op = record.record.op.as_str();
            if TRANSITIONS.contains(&op) && !record.record.extra.contains_key("refusal") {
                let todo = record.record.todo.as_ref().map(slug).transpose()?;
                lines.push(format!("{} {op} {todo:?}", record.record.plan));
            }
        }
    }
    Ok(lines)
}

/// The F1b journey: each walkthrough fixture applied to one engine, and its program under
/// `python/yi_runtime/tests/programs` run as a cell of a real kernel over a second one. Dies
/// with the control: let the library send another op, order or edge than the JSON surface
/// does and the two stores no longer reach the same plans.
#[tokio::test]
#[ignore = "tier-2 journey: `just journeys`"]
async fn a_program_and_its_json_fixture_reach_the_same_plan_json() -> Fallible<()> {
    let programs =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../python/yi_runtime/tests/programs");
    for (stem, program) in FIXTURE_STEMS
        .iter()
        .zip(["arc-game", "campaign", "typo-fix"])
    {
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(
            fixtures_dir().join(format!("{stem}.json")),
        )?)?;
        let doc = object(&doc, "fixture")?;
        let mut failures = Vec::new();
        let json_dir = Scratch::new(&format!("yi-plan-program-json-{program}"))?;
        let json_store = PlanStore::open(json_dir.to_path_buf())?;
        let stub = Arc::new(Stub::default());
        let engine = build_engine(doc, &json_store, &stub, &mut failures)?;
        let steps = require(doc, "steps", "fixture")?
            .as_array()
            .ok_or("steps is not an array")?;
        for step in steps {
            let step = object(step, stem)?;
            if step.contains_key("work") {
                continue;
            }
            let args = step.get("args").and_then(Value::as_object).cloned();
            let (op, reap_fails) = parse_op(
                text(require(step, "op", stem)?, stem)?,
                &args.unwrap_or_default(),
            )?;
            stub.fail_reap.store(reap_fails, Ordering::SeqCst);
            let plan = match step.get("plan") {
                Some(raw) => Some(PlanId::new(text(raw, stem)?)?),
                None => None,
            };
            // A refusal is the fixture's expectation, which `run_fixture` already holds it to.
            let _refused = engine.apply(OpRequest {
                plan,
                actor: parse_actor(text(require(step, "actor", stem)?, stem)?)?,
                op,
                request_id: None,
                expected_revision: None,
            });
        }

        let cell_dir = Scratch::new(&format!("yi-plan-program-cell-{program}"))?;
        let cell_store = PlanStore::open(cell_dir.to_path_buf())?;
        let engine = build_engine(doc, &cell_store, &Arc::new(Stub::default()), &mut failures)?;
        let mut registry = HostRegistry::default();
        yi_runtime::plan::request::register(Arc::new(engine), Actor::Owner, &mut registry);
        registry.register_mcp_stubs();
        let service = Arc::new(yi_runtime::KernelService::new(
            yi_runtime::KernelServiceOptions {
                cwd: std::env::temp_dir(),
                home: std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_default(),
                session_dir: None,
                family_dir: None,
                host: Arc::new(registry),
                on_restore: None,
                sandbox: None,
                snapshot_key: None,
                cell_ceiling: None,
            },
        ));
        let source = std::fs::read_to_string(programs.join(format!("{program}.py")))?;
        let kernel = Arc::clone(&service);
        let outcome = tokio::task::spawn_blocking(move || {
            let cancelled: yi_tools::CancelFlag = Arc::new(|| false);
            yi_tools::KernelBridge::execute_cell(kernel.as_ref(), &source, &cancelled)
        })
        .await??;
        service.dispose().await;
        assert!(failures.is_empty(), "{failures:?}");
        assert!(
            outcome.result.error.is_none(),
            "{program}.py raised: {:?}",
            outcome.result.error
        );
        assert_eq!(
            reached(&cell_store)?,
            reached(&json_store)?,
            "{program}.py and {stem}.json part ways"
        );
    }
    Ok(())
}
