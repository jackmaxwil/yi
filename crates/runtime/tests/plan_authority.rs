//! A user's confirmation of an op the plan owner may not apply alone stands for the plan as the
//! user saw it: however late the answer comes, and never once the plan moved under the question.

use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, json};
use yi_runtime::plan::authority::{Confirmer, Submission, SubmitError, Unhosted, submit};
use yi_runtime::plan::ops::{Actor, Op, OpRequest, PlanEngine, PlanOpError};
use yi_runtime::plan::store::PlanStore;
use yi_runtime::session_store::{CreateOptions, JsonlRepo, SessionRepo};
use yi_runtime::{AskOutcome, Asker, PermissionAsk, PermissionBroker, PermissionMode};
use yi_types::plan::doc::{GoalText, PlanId, TodoLabel};
use yi_types::plan::op::TodoSpec;

use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;
/// What the submission answered, and the kinds of the plan's journal records after it.
type Confirmed = (Result<(), SubmitError>, Vec<String>);

fn todo(label: &str) -> Result<TodoSpec, Box<dyn Error>> {
    Ok(serde_json::from_value(
        json!({ "label": TodoLabel::new(label)? }),
    )?)
}

fn owner(engine: &PlanEngine, plan: Option<PlanId>, op: Op) -> Result<(), PlanOpError> {
    engine
        .apply(OpRequest {
            plan,
            actor: Actor::Owner,
            op,
            request_id: None,
            expected_revision: None,
        })
        .map(drop)
}

/// A `fuse_reset` confirmed by a person who runs `meanwhile` before answering yes.
fn confirm_after(
    name: &str,
    meanwhile: impl Fn(&PlanEngine, &PlanId) + Send + Sync + 'static,
) -> Result<Confirmed, Box<dyn Error>> {
    let temp = Scratch::new(&format!("yi-plan-authority-{name}"))?;
    let store = PlanStore::open(temp.join("plans"))?;
    let engine = Arc::new(PlanEngine::new(store.clone(), Arc::new(Unhosted)));
    let goal = GoalText::new("confirm an op")?;
    let plan = PlanId::slug("confirm an op")?;
    owner(
        &engine,
        None,
        Op::Init {
            goal,
            todos: vec![todo("first")?],
        },
    )?;
    std::fs::create_dir_all(temp.join("sessions"))?;
    let mut repo = JsonlRepo::new(temp.join("sessions"), temp.to_string_lossy().into_owned());
    let session = repo.create(CreateOptions::default())?;
    let (asking, at) = (Arc::clone(&engine), plan.clone());
    let asker: Asker = Arc::new(move |_ask: &PermissionAsk<'_>| {
        meanwhile(&asking, &at);
        AskOutcome::AllowOnce
    });
    let (events, _nobody_listens) = tokio::sync::broadcast::channel(8);
    let broker = PermissionBroker::new(
        PermissionMode::Ask,
        temp.to_path_buf(),
        Vec::new(),
        Some(asker),
        events,
    );
    let confirmer = Confirmer {
        broker: Arc::new(broker),
        store: session,
    };
    let mut args = Map::new();
    args.insert("op".to_owned(), "fuse_reset".into());
    let submission = Submission {
        args,
        request_id: None,
        expected_revision: None,
    };
    let applied = submit(&engine, &Actor::Owner, Some(&confirmer), submission).map(drop);
    let records = store.journal(&plan).read()?.records;
    let kinds = records
        .iter()
        .map(|record| record.record.op.clone())
        .collect();
    Ok((applied, kinds))
}

/// Dies with the revision binding gone from the confirmed request: the plan gained a todo while
/// the person was asked, and their yes applied to a plan they never saw.
#[test]
fn a_plan_edited_while_its_confirmation_was_asked_is_refused() -> TestResult {
    let (applied, kinds) = confirm_after("edited", |engine, plan| {
        let todos = todo("later").map(|later| vec![later]).unwrap_or_default();
        let _appended = owner(engine, Some(plan.clone()), Op::Append { todos });
    })?;
    assert!(
        matches!(
            applied,
            Err(SubmitError::Op(PlanOpError::StaleRevision { .. }))
        ),
        "{applied:?}"
    );
    assert_eq!(kinds, ["init", "append"], "nothing applied after the yes");
    Ok(())
}

/// A yes to a plan that did not move while it was asked is applied, cited as the user.
#[test]
fn a_confirmation_of_an_unchanged_plan_is_applied() -> TestResult {
    let (applied, kinds) = confirm_after("unchanged", |_, _| {})?;
    assert!(applied.is_ok(), "{applied:?}");
    assert_eq!(kinds, ["init", "fuse_reset"]);
    Ok(())
}

/// Dies with the five-minute answer window back: a correct yes given after 301 s was discarded
/// with "ask again", though the plan had not moved.
#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn a_late_confirmation_of_an_unchanged_plan_is_applied() -> TestResult {
    let late = |_: &PlanEngine, _: &PlanId| std::thread::sleep(Duration::from_secs(301));
    let (applied, kinds) = confirm_after("late", late)?;
    assert!(applied.is_ok(), "{applied:?}");
    assert_eq!(kinds, ["init", "fuse_reset"]);
    Ok(())
}
