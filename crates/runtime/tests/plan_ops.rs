//! Engine-level tests for [`yi_runtime::plan::ops::PlanEngine`]: the op suite
//! that used to live beside the engine, plus one regression per confirmed
//! defect of the 2026-08-31 review — each was watched failing on the unfixed
//! engine before its fix landed.
//!
//! F0c, completion. Plan section 3.6 is the invariant every row below defends: a
//! managed todo is `Done` only when the verification token names the current attempt,
//! the frozen contract and criteria match, every critical item passed, score and
//! coverage meet the frozen policy, the accepted output snapshot is the verified one,
//! and the verdict and the transition are one committed journal record. `done`, `set`,
//! `import`, `repair`, `supersede`, the CLI and every shape go through that one
//! validator. The rows are ordered by what they defend: first that no surface can walk
//! around the validator, then that the verdict the validator reads is the right one,
//! then what a refusal costs.
//!
//! | test | tier | what it pins | the control it dies with |
//! |---|---|---|---|
//! | `set_cannot_complete_a_failing_task` | T0 | `set` may declare and rearrange work and may request legal transitions, and may not author `Done`. A row asking for `done` with no committed verdict refuses the whole transaction, naming the row; a second `set` asking only for `blocked` applies, which is the control against a driver that refuses every `set`. | The `done` row going through the same validator as `done` itself. `ops.rs` authors `Done` directly today, so this is red until the validator lands. Restore the direct write and the first `set` succeeds with no verdict anywhere in the plan. Fixture: `fixtures/plans/contracts/set-cannot-complete.json`. |
//! | `every_surface_requires_matching_verified_completion` | T1 | The same todo, in the same state, refused identically through the tool, through `plan.op`, through the CLI, through `import`, through `repair`, and through a view restored from a checkpoint. One list of surfaces, one refusal text, one journal record shape. | One validator, called from every path, rather than a check per surface. Give any one surface its own completion path and that surface becomes the way a model completes unverified work; the T1 tier is the point, since the failure this catches is a surface someone forgot. |
//! | `writer_requires_a_passing_critical_behavioral_check` | T0 | The writer floor of §6.2: at least one critical behavioural item, `cmd` or `example`. A schema may add shape and a judge item never stands alone, so a writer contract of one schema item plus one judge item is refused at `init`, `append`, `retry` and `supersede`, at declaration, not at `done`. | `floor_of` checked by `Plan::validate()` at every insert. Check the floor only at `done` and a plan sits in the store for a week promising a deliverable nothing can decide. The neighbouring failure, a trivial command added to satisfy the floor, is a specification failure no test catches; the fixture review looks for it and this row says so. |
//! | `missing_output_schema_or_resolver_never_passes` | T0 | An output that resolves to `Ok(None)`, and a schema document that resolves to `Ok(None)`, are refusals with distinct reasons, charging no refusal against the todo: resolved but unserved means the bytes are not here to adjudicate, which is the case where a pass is least founded. | The explicit `None` arms. Today `check_output` (`crates/runtime/src/plan/output.rs:103-105`) validates only when both resolve to `Some` and falls through to `Ok(())` otherwise, so this row is red on the tree as it stands. Restore the two-`Some` guard and the fixture passes `done` on a product nobody read. Fixture: `fixtures/plans/contracts/unserved-output-today-passes.json`. |
//! | `old_attempt_verdict_cannot_complete_restarted_task` | T0 | A verification committed under attempt 1 that returns after a `fail`, a `retry` and a `start` put the todo `Running` on attempt 2 with the same contract, output and workspace is refused `Stale` naming the attempt move, commits `verification_stale`, and charges no refusal: nothing about the product was decided. The next attempt verifies and completes normally, so the stale path is not a dead end. | The `attempt` field of the whole-token comparison at step 5: it is the only field that moved, so masking it completes a todo whose product no longer exists. Charge the refusal and three stale verdicts walk a healthy todo into `Blocked { on: User }`. Fixture: `fixtures/plans/contracts/stale-token.json`. |
//! | `changed_criterion_or_output_invalidates_verdict` | T0 | A `done` whose contract or criteria digest differs from the frozen one is refused `ContractDrift`, naming the label; an output digest that changed between the freeze and the comparison is refused the same way. The road back is `retry`, a new attempt with a new freeze, or `supersede`. | The contract and criteria digests being in the token and compared whole. Freeze the contract by reference rather than by digest and a product that edits its own checker passes, which is the one failure a verified `done` exists to prevent. |
//! | `concurrent_done_requests_share_verification_effect` | T0 | Two `done` calls for the same token run the checks once, charge one refusal at most, and return or await the same verdict. A third arriving after the verdict commits replays it. | The `verification_requested` record committed at step 2 being the effect's identity, and a second `done` joining it rather than starting its own. Key the effect on the request id instead of the token and two calls run the checker twice and charge two refusals for one product. |
//! | `another_engine_refuses_a_live_claim_and_charges_nothing` | T0 | A second engine over the same store (the CLI beside a session) calling `done` for the token a live process is verifying is refused as a verification in progress, naming the claimant's pid; the checker runs once, one `verification_requested` and one `done_refused` land, and the todo's refusals read 1. | The `claim` on the `verification_requested` record and `refuse_live_claim` in `done.rs`: the in-flight map is per process, so without the claim a second process adopts the effect, runs the checker again and charges a second refusal for one product. |
//! | `product_repair_passes_under_unchanged_criteria` | T1 | The red-then-green pair: the first product fails the frozen checker and `done` is refused with a recorded verdict; the product is repaired and the same frozen checker passes it. Every verdict in the run carries the same contract and criteria digest and a different output digest, which is the whole claim, and the todo's refusal counter still reads 1 after the pass. | The criteria being frozen at `start` and the refusal leaving the todo where it was. Let the contract be rewritten between attempts and the fixture still goes green while proving nothing, which is the specification failure this pair exists to make visible. Fixture: `fixtures/plans/contracts/writer-cmd-red-then-green.json`. |
//! | `inline_task_output_validates_product_not_sidecar` | T0 | An inline todo's contract runs against the product itself, not against a note, a summary or a sidecar file the same turn wrote. The inline floor is the writer or reader floor of its declared role; running the work inline is not an exemption from it. | The output artifact being the one the token names. Let an inline todo nominate any artifact and the cheapest passing move is to write a sidecar that satisfies the schema, which is the reward-hacking shape the floors exist against. |
//! | `the_third_refusal_blocks_the_todo_on_user_as_a_recorded_transition` | T0 | Three refused verdicts on one todo step it to `Blocked { on: User }` as its own committed transition with its own record, so the human inbox learns about it from the journal and not from a counter someone has to read. Stale and infrastructure abstentions do not count toward the three. | The counter bump being an explicit event and the cap being a transition rather than a return value. Make the cap a refusal text and a model retries forever; count stale verdicts and a slow checker walks a healthy todo into the inbox. |
//! | `a_retry_opens_a_fresh_refusal_count_so_only_retry_cap_bounds_a_scheduler` | T0 | The section 6.3 cap counts the refused verdicts of one attempt, not of the todo: two refusals, a `fail` and a `retry`, then two more, leave the todo running with four journaled refusals and no `block`. `todo.refusals` is the lifetime event counter and is never what the cap reads, so `RETRY_CAP` is the only durable bound on a scheduler that retries a failed todo (D212). | The `attempt` filter in `refused_verdicts`. Drop it and the third refusal of a todo's life parks every retrying shape in the human inbox, whichever attempt it belongs to. |
//! | `accept_records_accepted_by_user_never_verified_done` | T1 | The user's acceptance is its own op: refused to the owner, confirmed through `authority::submit` as the CLI and the console do, recorded as `accepted_by_user` with the citation as its actor, and landing `Done { AcceptedByUser }` with no `pass` verdict anywhere. | `check_actor` refusing the owner and `Actor::User` being minted only by the confirmed path. Let the owner accept and a model closes what its checker refused; write `VerifiedDone` here and the report cannot tell a checked todo from a waved-through one. |
//! | `a_stated_only_todo_needs_an_item_or_a_user` | T0 | A todo whose only requirement is a stated acceptance is refused `done` on the owner's word; it completes once a decidable item is added and passes, or once a user accepts it. | `needs_resolution` counting a stated-only delegation. Drop it and "it works" is a contract again, the failure D77 was retired for. |
//! | `a_retry_swapping_in_an_uncontracted_worktree_delegation_is_refused` | T0 | A `retry` that swaps a worktree delegation onto a todo with no contract is refused; the same swap onto a contracted todo lands. Every declaring op refuses the shape at parse (`TodoSpec`'s `try_from`, pinned in `plan/tool.rs`), and `retry` is the one op that changes a delegation without carrying the contract beside it. | `check_contracted` in `table.rs`. Drop it and the shape is declarable through `retry` alone and dead there, which is what both dogfood owners wrote. |
//! | `a_leftover_open_effect_does_not_hide_the_live_verification` | T0 | With an older `verification_requested` on the same todo left open under another token, two concurrent `done` calls still share one effect: the checker runs once, one refusal is charged, and the journal holds the leftover plus one live effect. | The token in `pending_verification`'s search. Match on the label alone and the oldest open effect is found first, the token filter drops it, and each call mints its own effect, runs the checker and charges a refusal. |
//! | `a_checker_that_writes_into_the_workspace_still_passes` | T0 | A passing checker that appends to a file in its working directory lands `Done { VerifiedDone }` and the checkout is untouched: the checker runs in a materialization of the step 1 tree (`workspace_of(snapshot)`, section 6.3 step 4), never in the live checkout. | The materialization in `run_verifier`. Run the checker in the checkout and `pytest` writing a cache moves the tree step 5 re-captures, so it refuses its own pass as stale on every call. |
//! | `a_workspace_edited_during_the_check_is_stale` | T0 | A checkout edited while the checker runs (a concurrent agent or the user) is refused `Stale` with `the workspace changed`, charging nothing: step 5 captures the workspace afresh and compares it with the token, as it does the attempt, version, digests and output. | The re-capture in step 5 (`evidence` in done.rs takes no frozen snapshot). Hand the step 1 id back in and the comparison passes by construction, so a tree that no longer exists lands `VerifiedDone`. |
//! | `an_abstained_verification_is_rerun_not_replayed` | T0 | A verification that abstained for an infrastructure reason (a verifier deadline of 1 ms) is run again by the next `done` for the same token and passes, with no refusal charged at either point. | The `Fail \| Escalate` match on the settled verdict in `prepare` (done.rs). Replay every settled outcome and one abstention refuses a correct product forever without running the checker, and the cap never blocks it either. |
//! | `set_cannot_complete_a_worktree_todo` | T0 | A `[x]` row for a contracted worktree todo is refused `AcceptanceUnavailable` exactly as `done` is, and the todo stays `Running` with its child and lane; a worktree todo completes only through acceptance. | The worktree test in `completion_of` (state.rs), the validator `set`, `reconcile` and `accept` share with `done`. Keep it in `prepare` alone and `set` completes the todo `done` refuses, reaping nothing. |
//! | `a_verbose_refusal_still_journals_under_the_record_cap` | T0 | A refusal whose six item tails would not fit the 64 KiB record once escaped still lands as `done_refused`: the journaled item details are clipped, the refusal is charged and the effect is settled. | `fitted` rehearsing the record under the cap before the commit. Commit unrehearsed and `seal` refuses the line, so the caller sees a store error, nothing is charged and the next `done` re-runs the same checker into the same wall. |
//! | `import_marks_legacy_success_unverified` | T0 | An imported format-1 todo whose `Check::Stated` says a child called it done lands as `LegacyUnverified`, displayed as history and never as new evidence, and a later `set` asking `done` on the plan's remaining todo is refused all the same. | `Resolution` having three members. Read `LegacyUnverified` as `VerifiedDone` and one import launders a year of unchecked claims into verified work; drop the member and the history the user asked to keep is lost. Fixture: `fixtures/plans/contracts/legacy-stated-unverified.json`. |
//!
//! F0d, worktree acceptance. Plan section 6.6 at the engine, where F0c left a refusal: a
//! worktree todo was refused `AcceptanceUnavailable` on every completion path, and these
//! three rows replace that refusal with the accept phase. They live in the same `contracts`
//! module and use its helpers, so the helper column names what each turns on;
//! `a_retry_swapping_in_an_uncontracted_worktree_delegation_is_refused` and
//! `set_cannot_complete_a_worktree_todo` stay beside them, because an uncontracted worktree
//! todo has nothing to accept and `set` still may not author `Done`. These rows run over
//! the `Stub` delegate and seeded records, no repository; the live acceptance rows, against
//! `fixtures/plans/worktree/repo.sh`, are in `lanes.rs`.
//!
//! | test | tier | helpers | what it pins | the control it dies with |
//! |---|---|---|---|---|
//! | `failed_unmerged_task_can_preserve_or_discard_and_finish` | T1 | `rig`, `planned`, `cmd_contract`, `contracted` with `Isolation::Worktree`, `start`, `done`, `kinds`, `todo_of` | The same failed worktree todo, run twice: once finishing with `Retained` and once with `Discarded`. Both reach a terminal state, both journal a `disposition` record before the slot is free, both leave `kept` resolving, and neither journals an `accepted` or moves the parent. The choice rides the op, not a default. | `step_todo`'s reap on every exit from `Running` (`ops.rs:699-708`) taking the disposition path instead of dropping the lane. Drop it and the only way to finish a failed worktree todo is a merge, which is the pressure that makes a model merge a branch it knows is red. |
//! | `full_worker_capacity_does_not_deadlock_verification` | T0 | `Capacity::for_slots` over `DEFAULT_MAX_CHILDREN` lanes | Every worker a parent may retain asks for a checkout of its own, at the product's constants: the worker share of the lane pool runs out before the parent's child cap does, the lanes it refuses are the verification reserve, and with the share full a verification still reserves. Both counters end at zero. The live half, a worktree todo accepted while the engine's own worker share is held to its cap, is `lanes::full_worker_capacity_does_not_deadlock_verification` (T1). | Verification capacity being reserved separately from worker slots (section 7.6). Charge verification against the worker share and the workers a parent retains occupy every lane their own verification needs, so the plan stops with every todo running and nothing able to finish. |
//! | `a_worktree_child_cannot_be_marked_done_before_acceptance` | T0 | `rig`, `planned`, `cmd_contract`, `contracted` with `Isolation::Worktree`, `start`, `done`, `refused`, `todo_of`, `kinds` | `done` before the candidate is submitted is refused and the todo stays `Running`; `done` after `candidate_verified` but before `integration_verified` is refused the same way and journals no `accepted`; `fail` is legal at every one of those points and takes the disposition path. The refusal names the phase that is missing. | `done` being legal only at the accept phase, tested from the records rather than from whether a lane is held. Test the lane and a todo whose lane was already taken looks acceptable, which is exactly the state a merge-less reap leaves behind. |

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

use std::error::Error;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Map;
use yi_runtime::plan::journal::JournalError;
use yi_runtime::plan::ops::{
    Actor, Delegate, Op, OpRequest, Outcome, OutputResolve, PlanEngine, PlanOpError, SetRow,
    TodoSpec, dispatch_width,
};
use yi_runtime::plan::store::{PLAN_CAP_BYTES, PlanStore, StoreError};
use yi_runtime::plan::table::RETRY_CAP;
use yi_types::plan::PlanVersion;
use yi_types::plan::doc::{
    AgentId, BlockedOn, Check, Delegation, GoalText, OutputSchema, PlanId, PlanState, RetryCount,
    SPAWN_CAP, SpawnSpec, Todo, TodoAddr, TodoLabel, TodoState, TodoStateName, TouchCount,
};
use yi_types::url::Url;

type TestResult = Result<(), Box<dyn Error>>;

#[derive(Default)]
struct Stub {
    next: AtomicU32,
    reaps: AtomicU32,
    fail_reap: AtomicBool,
    reap_last: Mutex<Option<Url>>,
}

impl Delegate for Stub {
    fn spawn(&self, _at: &TodoAddr, _delegation: &Delegation) -> Result<AgentId, String> {
        let serial = self.next.fetch_add(1, Ordering::SeqCst);
        AgentId::new(format!("child-{serial}")).map_err(|error| error.to_string())
    }

    fn reap(&self, _agent: &AgentId, _supplied: &[Url]) -> Result<Option<Url>, String> {
        if self.fail_reap.load(Ordering::SeqCst) {
            return Err("child is wedged".to_owned());
        }
        self.reaps.fetch_add(1, Ordering::SeqCst);
        match self.reap_last.lock() {
            Ok(last) => Ok(last.clone()),
            Err(_) => Err("poisoned".to_owned()),
        }
    }
}

fn width(value: usize) -> Result<NonZeroUsize, Box<dyn Error>> {
    NonZeroUsize::new(value).ok_or_else(|| "zero width".into())
}

type Harness = (Scratch, PlanStore, Arc<Stub>, PlanEngine);

fn harness(cap: usize) -> Result<Harness, Box<dyn Error>> {
    let temp = Scratch::new("yi-plan-ops")?;
    let store = PlanStore::open(temp.to_path_buf())?;
    let stub = Arc::new(Stub::default());
    let engine = PlanEngine::new(store.clone(), stub.clone()).with_width(width(cap)?);
    Ok((temp, store, stub, engine))
}

fn label(text: &str) -> Result<TodoLabel, Box<dyn Error>> {
    Ok(TodoLabel::new(text)?)
}

fn spec(text: &str) -> Result<TodoSpec, Box<dyn Error>> {
    Ok(TodoSpec {
        label: label(text)?,
        after: Vec::new(),
        delegation: None,
        contract: None,
        children: Vec::new(),
    })
}

fn delegation() -> Delegation {
    Delegation {
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
    }
}

fn delegated_spec(text: &str) -> Result<TodoSpec, Box<dyn Error>> {
    Ok(TodoSpec {
        label: label(text)?,
        after: Vec::new(),
        delegation: Some(delegation()),
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

fn at(plan: &PlanId, op: Op) -> OpRequest {
    OpRequest {
        plan: Some(plan.clone()),
        actor: Actor::Owner,
        op,
        request_id: None,
        expected_revision: None,
    }
}

fn init(engine: &PlanEngine, specs: Vec<TodoSpec>) -> Result<Outcome, PlanOpError> {
    engine.apply(owner(Op::Init {
        goal: GoalText::new("ship the widget end to end").map_err(PlanOpError::Doc)?,
        todos: specs,
    }))
}

/// The engine's own first write into a directory that did not exist leaves the store's
/// `.gitignore` behind, so the journal is never the file a first commit tracks.
#[test]
fn an_engine_init_on_a_fresh_directory_publishes_the_gitignore() -> TestResult {
    let temp = Scratch::new("yi-plan-ops-fresh")?;
    let dir = temp.join("nested/.yi/plans");
    assert!(!dir.exists());
    let store = PlanStore::open(dir.clone())?;
    let engine = PlanEngine::new(store, Arc::new(Stub::default()));
    init(&engine, vec![spec("cut")?])?;
    assert!(
        std::fs::read_to_string(dir.join(".gitignore"))?.contains("*/ops.jsonl"),
        "the store ignores its journals from the first write"
    );
    assert!(
        temp.join("nested/.yi/schemas/plan.schema.json").is_file(),
        "the schema is published with it"
    );
    Ok(())
}

#[test]
fn width_is_the_family_cap_not_the_host() -> TestResult {
    // This replaces width_clamps_low_and_high, which pinned width(1) = 1 and width(2) = 1.
    // That rule made fan-out unavailable in a 2-core container, which is every machine the
    // evals and CI run on, and no lever could raise it (#468). The width now measures what
    // actually bounds delegated children, and never the host's cores.
    let levers = yi_runtime::levers::get();
    assert_eq!(
        dispatch_width().get(),
        levers.family_max_children.min(levers.plan_width_max)
    );
    assert!(
        dispatch_width().get() > 1,
        "a small host must still admit fan-out"
    );
    Ok(())
}

#[test]
fn backpressure_holds_a_delegated_todo_pending() -> TestResult {
    let (_temp, store, _stub, engine) = harness(1)?;
    let out = init(
        &engine,
        vec![
            delegated_spec("first child job")?,
            delegated_spec("second child job")?,
        ],
    )?;
    // The engine starts what admission lets through with no owner op; the rest is held.
    assert!(
        out.dispatched.is_empty(),
        "a started todo is spawned, not dispatchable"
    );
    assert_eq!(out.spawned.len(), 1);
    assert_eq!(out.held, vec![label("second child job")?]);
    let first = out
        .plan
        .todo(&label("first child job")?)
        .ok_or("todo missing")?;
    assert!(matches!(first.state, TodoState::Running { .. }));
    let file = store.read(&out.plan.id)?;
    let held = file
        .todo(&label("second child job")?)
        .ok_or("held todo missing")?;
    assert_eq!(held.state, TodoState::Pending);
    let refused = engine.apply(owner(Op::Start {
        label: label("first child job")?,
    }));
    assert!(
        matches!(refused, Err(PlanOpError::IllegalStep { .. })),
        "the owner has nothing left to start: {refused:?}"
    );
    // Backpressure is a refusal, not an ordering hint: the held todo cannot take the
    // occupied slot even when its start is asked for by name.
    let refused = engine.apply(owner(Op::Start {
        label: label("second child job")?,
    }));
    match refused {
        Err(PlanOpError::Admission(refusal)) => {
            assert_eq!(refusal.slots, 0);
            assert_eq!(refusal.position, 1);
            assert_eq!(refusal.delegated_ready, 1);
        }
        other => return Err(format!("expected an admission refusal, got {other:?}").into()),
    }
    let out = engine.apply(owner(Op::Done {
        label: label("first child job")?,
        output: Some("kernel://main/cli_surface".parse::<Url>()?),
    }))?;
    let todo = out
        .plan
        .todo(&label("first child job")?)
        .ok_or("todo missing")?;
    assert_eq!(
        todo.state,
        TodoState::Done {
            output: Some("kernel://main/cli_surface".parse::<Url>()?),
            resolution: None,
        }
    );
    // The freed slot starts the todo that was held, inside the same op.
    assert_eq!(out.spawned.len(), 1);
    assert_eq!(out.plan.spawns().get(), 2);
    let second = out
        .plan
        .todo(&label("second child job")?)
        .ok_or("todo missing")?;
    assert!(matches!(second.state, TodoState::Running { .. }));
    Ok(())
}

/// Dies with `Actor::Engine` in `actor_word`: journal the engine's start as the owner's and
/// the record cannot tell a start nobody asked for from one the model made.
#[test]
fn an_engine_start_is_journaled_as_engine() -> TestResult {
    let (_temp, store, _stub, engine) = harness(8)?;
    let out = init(&engine, vec![delegated_spec("delegated job")?])?;
    let records = store.journal(&out.plan.id).read()?.records;
    let actors: Vec<(&str, &str)> = records
        .iter()
        .map(|record| (record.record.op.as_str(), record.record.actor.as_str()))
        .collect();
    assert_eq!(
        actors,
        vec![
            ("init", "main"),
            ("spawn_intent", "engine"),
            ("spawn_result", "engine"),
            ("start", "engine"),
        ]
    );
    Ok(())
}

/// Block is the hold and unblock the release: a blocked delegated todo whose edge clears is
/// not started, and the unblock starts it in the same op.
#[test]
fn a_blocked_todo_is_not_started() -> TestResult {
    let (_temp, _store, stub, engine) = harness(8)?;
    let mut gated = delegated_spec("delegated job")?;
    gated.after = vec![label("gate")?];
    init(&engine, vec![spec("gate")?, gated])?;
    engine.apply(owner(Op::Block {
        label: label("delegated job")?,
        on: BlockedOn::User,
        note: "hold it until the design is agreed".to_owned(),
    }))?;
    engine.apply(owner(Op::Start {
        label: label("gate")?,
    }))?;
    let out = engine.apply(owner(Op::Done {
        label: label("gate")?,
        output: None,
    }))?;
    assert!(out.spawned.is_empty(), "{:?}", out.spawned);
    let held = out
        .plan
        .todo(&label("delegated job")?)
        .ok_or("todo missing")?;
    assert!(matches!(held.state, TodoState::Blocked { .. }));
    assert_eq!(stub.next.load(Ordering::SeqCst), 0);
    let out = engine.apply(owner(Op::Unblock {
        label: label("delegated job")?,
    }))?;
    assert_eq!(out.spawned.len(), 1, "the release starts it");
    let released = out
        .plan
        .todo(&label("delegated job")?)
        .ok_or("todo missing")?;
    assert!(matches!(released.state, TodoState::Running { .. }));
    Ok(())
}

/// Refuses the first `fail_first` spawns, as a host whose `rlm.run` children fill it does.
struct Flaky {
    fail_first: u32,
    tried: AtomicU32,
}

impl Delegate for Flaky {
    fn spawn(&self, _at: &TodoAddr, _delegation: &Delegation) -> Result<AgentId, String> {
        let tried = self.tried.fetch_add(1, Ordering::SeqCst);
        if tried < self.fail_first {
            return Err("RLM child limit reached".to_owned());
        }
        AgentId::new(format!("child-{tried}")).map_err(|error| error.to_string())
    }

    fn reap(&self, _agent: &AgentId, _supplied: &[Url]) -> Result<Option<Url>, String> {
        Ok(None)
    }
}

/// Dies with every recordable refusal cached for the attempt (schedule.rs): a spawn the host
/// refused once for a full roster is never tried again, and the todo stays pending forever.
#[test]
fn a_transient_spawn_failure_is_tried_again_by_the_next_op() -> TestResult {
    let temp = Scratch::new("yi-plan-ops-flaky")?;
    let flaky = Arc::new(Flaky {
        fail_first: 1,
        tried: AtomicU32::new(0),
    });
    let engine = PlanEngine::new(PlanStore::open(temp.to_path_buf())?, flaky.clone());
    let out = init(&engine, vec![delegated_spec("delegated job")?])?;
    assert!(out.spawned.is_empty(), "{:?}", out.spawned);
    let seen = engine.apply(at(&out.plan.id, Op::View { full: false }))?;
    assert_eq!(
        seen.notices,
        [
            "the engine could not start \"delegated job\": could not spawn a child at ship-the-widget-end-to-end/delegated job: RLM child limit reached"
        ],
        "a caller reads the engine's refusal, not an idle todo"
    );
    let out = engine.apply(at(
        &out.plan.id,
        Op::Append {
            todos: vec![spec("write the notes")?],
        },
    ))?;
    assert_eq!(out.spawned.len(), 1, "the next op tries the start again");
    assert_eq!(flaky.tried.load(Ordering::SeqCst), 2);
    Ok(())
}

/// Dies with `raced` unread in `settle` (ops.rs): a start another pass got to first journals
/// an engine refusal and charges the running todo for it.
#[test]
fn a_raced_engine_start_journals_nothing() -> TestResult {
    let (_temp, store, _stub, engine) = harness(8)?;
    let out = init(&engine, vec![delegated_spec("delegated job")?])?;
    let before = store.journal(&out.plan.id).read()?.records.len();
    let raced = engine.apply(OpRequest {
        plan: Some(out.plan.id.clone()),
        actor: Actor::Engine,
        op: Op::Start {
            label: label("delegated job")?,
        },
        request_id: None,
        expected_revision: None,
    });
    assert!(
        matches!(raced, Err(PlanOpError::IllegalStep { .. })),
        "{raced:?}"
    );
    assert_eq!(store.journal(&out.plan.id).read()?.records.len(), before);
    let todo = store.read(&out.plan.id)?;
    assert_eq!(
        todo.todo(&label("delegated job")?)
            .ok_or("missing")?
            .refusals,
        0
    );
    Ok(())
}

/// Dies with `standing` unread in `view` (ops.rs): a script cannot tell a todo the width holds
/// from one the engine was refused, and reads every idle todo as waiting on a slot.
#[test]
fn a_view_names_the_todos_the_width_holds() -> TestResult {
    let (_temp, _store, _stub, engine) = harness(1)?;
    let out = init(
        &engine,
        vec![delegated_spec("first job")?, delegated_spec("second job")?],
    )?;
    let seen = engine.apply(at(&out.plan.id, Op::View { full: false }))?;
    assert_eq!(seen.held, [label("second job")?]);
    assert!(seen.notices.is_empty(), "{:?}", seen.notices);
    Ok(())
}

#[test]
fn fuse_refuses_at_cap_and_survives_supersede() -> TestResult {
    let (_temp, store, _stub, engine) = harness(8)?;
    let mut gated = delegated_spec("delegated job")?;
    gated.after = vec![label("gate")?];
    let out = init(&engine, vec![spec("gate")?, gated])?;
    let mut file = store.read(&out.plan.id)?;
    while file.spawns() < SPAWN_CAP {
        file.charge_spawn();
    }
    store.write(&file)?;
    engine.apply(owner(Op::Start {
        label: label("gate")?,
    }))?;
    let out = engine.apply(owner(Op::Done {
        label: label("gate")?,
        output: None,
    }))?;
    assert!(
        out.notices
            .iter()
            .any(|notice| notice.contains("delegated job") && notice.contains("spawn ceiling")),
        "the engine's refused start is the op's notice: {:?}",
        out.notices
    );
    let todo = out
        .plan
        .todo(&label("delegated job")?)
        .ok_or("todo missing")?;
    assert_eq!(todo.state, TodoState::Pending);
    let refused = engine.apply(owner(Op::Start {
        label: label("delegated job")?,
    }));
    match refused {
        Err(PlanOpError::SpawnCeilingExhausted { spent, cap }) => {
            assert_eq!(spent, SPAWN_CAP);
            assert_eq!(cap, SPAWN_CAP);
        }
        other => return Err(format!("expected ceiling refusal, got {other:?}").into()),
    }
    let out = engine.apply(owner(Op::Supersede {
        reason: "the cut was wrong".to_owned(),
        todos: vec![delegated_spec("replacement job")?],
    }))?;
    assert_eq!(out.plan.spawns(), SPAWN_CAP);
    assert!(
        out.notices
            .iter()
            .any(|notice| notice.contains("replacement job") && notice.contains("spawn ceiling")),
        "{:?}",
        out.notices
    );
    Ok(())
}

#[test]
fn supersede_is_atomic_when_a_reap_fails() -> TestResult {
    let (_temp, store, stub, engine) = harness(8)?;
    let out = init(&engine, vec![delegated_spec("delegated job")?])?;
    stub.fail_reap.store(true, Ordering::SeqCst);
    let refused = engine.apply(owner(Op::Supersede {
        reason: "rethink".to_owned(),
        todos: vec![spec("replacement job")?],
    }));
    assert!(matches!(refused, Err(PlanOpError::ReapFailed { .. })));
    let file = store.read(&out.plan.id)?;
    assert_eq!(file.version, PlanVersion(1));
    assert_eq!(file.touched, TouchCount(2));
    assert_eq!(file.state, PlanState::Active);
    let todo = file.todo(&label("delegated job")?).ok_or("todo missing")?;
    assert!(matches!(todo.state, TodoState::Running { .. }));
    Ok(())
}

#[test]
fn ephemeral_urls_are_refused_in_terminal_records() -> TestResult {
    let (_temp, _store, stub, engine) = harness(8)?;
    init(&engine, vec![delegated_spec("delegated job")?])?;
    let refused = engine.apply(owner(Op::Done {
        label: label("delegated job")?,
        output: Some("agent://somewhere/live".parse::<Url>()?),
    }));
    assert!(matches!(
        refused,
        Err(PlanOpError::EphemeralTerminal { .. })
    ));
    let refused = engine.apply(owner(Op::Done {
        label: label("delegated job")?,
        output: Some("kernel://child-0/scratch".parse::<Url>()?),
    }));
    assert!(matches!(
        refused,
        Err(PlanOpError::EphemeralTerminal { .. })
    ));
    if let Ok(mut last) = stub.reap_last.lock() {
        *last = Some("kernel://child/scratch".parse::<Url>()?);
    }
    let refused = engine.apply(owner(Op::Fail {
        label: label("delegated job")?,
        cause: "went sideways".to_owned(),
        disposition: None,
    }));
    assert!(matches!(
        refused,
        Err(PlanOpError::EphemeralTerminal { .. })
    ));
    if let Ok(mut last) = stub.reap_last.lock() {
        *last = Some("kernel://main/report".parse::<Url>()?);
    }
    let out = engine.apply(owner(Op::Fail {
        label: label("delegated job")?,
        cause: "went sideways".to_owned(),
        disposition: None,
    }))?;
    assert_eq!(out.reaped.len(), 1);
    let todo = out
        .plan
        .todo(&label("delegated job")?)
        .ok_or("todo missing")?;
    assert_eq!(
        todo.state,
        TodoState::Failed {
            cause: "went sideways".to_owned(),
            last: Some("kernel://main/report".parse::<Url>()?),
        }
    );
    assert_eq!(out.plan.state, PlanState::Done);
    let found = engine.apply(owner(Op::View { full: false }))?;
    assert_eq!(
        found.plan.id, out.plan.id,
        "a finished plan with a failed todo stays reachable unnamed"
    );
    Ok(())
}

#[test]
fn a_cycle_is_refused_at_insert() -> TestResult {
    let (_temp, store, _stub, engine) = harness(8)?;
    let second = TodoSpec {
        label: label("second job")?,
        after: vec![label("first job")?],
        delegation: None,
        contract: None,
        children: Vec::new(),
    };
    let out = init(&engine, vec![spec("first job")?, second])?;
    let refused = engine.apply(owner(Op::AddEdge {
        todo: label("first job")?,
        after: label("second job")?,
    }));
    match refused {
        Err(PlanOpError::Invalid { issue }) => {
            assert!(issue.to_string().contains("cycle"));
        }
        other => return Err(format!("expected cycle refusal, got {other:?}").into()),
    }
    let file = store.read(&out.plan.id)?;
    assert_eq!(file.touched, TouchCount(1));
    let refused = engine.apply(owner(Op::AddEdge {
        todo: label("no such job")?,
        after: label("first job")?,
    }));
    assert!(matches!(refused, Err(PlanOpError::UnknownLabel { .. })));
    Ok(())
}

#[test]
fn a_partial_reorder_is_refused() -> TestResult {
    let (_temp, _store, _stub, engine) = harness(8)?;
    init(&engine, vec![spec("first job")?, spec("second job")?])?;
    let refused = engine.apply(owner(Op::Reorder {
        labels: vec![label("first job")?],
    }));
    assert!(matches!(
        refused,
        Err(PlanOpError::NotAPermutation {
            got: 1,
            expected: 2
        })
    ));
    let out = engine.apply(owner(Op::Reorder {
        labels: vec![label("second job")?, label("first job")?],
    }))?;
    let first = out.plan.todos.first().ok_or("empty plan")?;
    assert_eq!(first.label, label("second job")?);
    Ok(())
}

#[test]
fn mutation_is_owner_gated_and_unblock_is_open_to_user_and_host() -> TestResult {
    let (_temp, _store, _stub, engine) = harness(8)?;
    init(&engine, vec![spec("first job")?])?;
    let refused = engine.apply(OpRequest {
        plan: None,
        actor: Actor::Child(AgentId::new("child-0")?),
        op: Op::Start {
            label: label("first job")?,
        },
        request_id: None,
        expected_revision: None,
    });
    match refused {
        Err(err @ PlanOpError::NotOwner { .. }) => {
            assert!(err.to_string().contains("propose to the owner"));
        }
        other => return Err(format!("expected owner refusal, got {other:?}").into()),
    }
    let engine_may = |op: Op| {
        engine.apply(OpRequest {
            plan: None,
            actor: Actor::Engine,
            op,
            request_id: None,
            expected_revision: None,
        })
    };
    let refused = engine_may(Op::Append {
        todos: vec![spec("engine job")?],
    });
    assert!(
        matches!(refused, Err(PlanOpError::NotOwner { .. })),
        "the engine steps todos and never edits the cut: {refused:?}"
    );
    let started = engine_may(Op::Start {
        label: label("first job")?,
    })?;
    let todo = started
        .plan
        .todo(&label("first job")?)
        .ok_or("todo missing")?;
    assert!(matches!(todo.state, TodoState::Running { .. }));
    engine.apply(owner(Op::Block {
        label: label("first job")?,
        on: BlockedOn::User,
        note: "needs a decision".to_owned(),
    }))?;
    let out = engine.apply(OpRequest {
        plan: None,
        actor: Actor::User("user://3".parse::<Url>()?),
        op: Op::Unblock {
            label: label("first job")?,
        },
        request_id: None,
        expected_revision: None,
    })?;
    let todo = out.plan.todo(&label("first job")?).ok_or("todo missing")?;
    assert_eq!(todo.state, TodoState::Pending);
    Ok(())
}

#[test]
fn a_sub_plan_dispatch_charges_the_root_fuse() -> TestResult {
    let (_temp, store, _stub, engine) = harness(8)?;
    let out = init(&engine, vec![spec("parent job")?])?;
    let root_id = out.plan.id.clone();
    engine.apply(owner(Op::Start {
        label: label("parent job")?,
    }))?;
    let out = engine.apply(owner(Op::Decompose {
        label: label("parent job")?,
        todos: vec![delegated_spec("small piece")?],
    }))?;
    let sub_id = out.subplan.ok_or("no sub-plan opened")?;
    assert_eq!(
        out.spawned.len(),
        1,
        "the engine starts the sub-plan's delegated todo"
    );
    assert_eq!(store.read(&root_id)?.spawns().get(), 1);
    assert!(store.read(&sub_id)?.spawns().is_zero());
    let refused = engine.apply(at(
        &sub_id,
        Op::Decompose {
            label: label("small piece")?,
            todos: vec![spec("depth three")?],
        },
    ));
    assert!(matches!(refused, Err(PlanOpError::DepthExhausted { .. })));
    Ok(())
}

// Regressions from the 2026-08-31 confirmed-defect review, one per finding.

#[test]
fn blocking_a_running_delegated_todo_reaps_the_child() -> TestResult {
    let (_temp, store, stub, engine) = harness(1)?;
    init(
        &engine,
        vec![delegated_spec("first job")?, delegated_spec("second job")?],
    )?;
    let out = engine.apply(owner(Op::Block {
        label: label("first job")?,
        on: BlockedOn::User,
        note: "waiting on a decision".to_owned(),
    }))?;
    assert_eq!(out.reaped.len(), 1, "the block exit from Running must reap");
    assert_eq!(stub.reaps.load(Ordering::SeqCst), 1);
    assert_eq!(out.spawned.len(), 1, "the freed slot starts the held todo");
    let file = store.read(&out.plan.id)?;
    let blocked = file.todo(&label("first job")?).ok_or("todo missing")?;
    assert!(matches!(blocked.state, TodoState::Blocked { .. }));
    Ok(())
}

#[test]
fn supersede_terminates_sub_plan_todos_and_frees_the_width() -> TestResult {
    let (_temp, store, stub, engine) = harness(1)?;
    init(&engine, vec![spec("parent job")?])?;
    engine.apply(owner(Op::Start {
        label: label("parent job")?,
    }))?;
    let out = engine.apply(owner(Op::Decompose {
        label: label("parent job")?,
        todos: vec![delegated_spec("small piece")?],
    }))?;
    let sub_id = out.subplan.ok_or("no sub-plan opened")?;
    if let Ok(mut last) = stub.reap_last.lock() {
        *last = Some("history://small-piece/e4".parse::<Url>()?);
    }
    let out = engine.apply(owner(Op::Supersede {
        reason: "wrong cut".to_owned(),
        todos: vec![delegated_spec("fresh job")?],
    }))?;
    assert_eq!(out.reaped.len(), 1);
    let sub = store.read(&sub_id)?;
    assert_eq!(sub.state, PlanState::Abandoned);
    let todo = sub.todo(&label("small piece")?).ok_or("sub todo missing")?;
    match &todo.state {
        TodoState::Failed { cause, last } => {
            assert!(cause.contains("superseded"), "{cause:?}");
            assert_eq!(last, &Some("history://small-piece/e4".parse::<Url>()?));
        }
        other => return Err(format!("expected a terminal reaped todo, got {other:?}").into()),
    }
    assert_eq!(
        out.spawned.len(),
        1,
        "the abandoned sub-plan must not hold a phantom width slot"
    );
    assert!(out.held.is_empty());
    Ok(())
}

#[test]
fn supersede_requires_an_active_plan() -> TestResult {
    let (_temp, store, _stub, engine) = harness(8)?;
    init(&engine, vec![spec("parent job")?])?;
    engine.apply(owner(Op::Start {
        label: label("parent job")?,
    }))?;
    let out = engine.apply(owner(Op::Decompose {
        label: label("parent job")?,
        todos: vec![spec("small piece")?],
    }))?;
    let sub_id = out.subplan.ok_or("no sub-plan opened")?;
    engine.apply(owner(Op::Supersede {
        reason: "restart".to_owned(),
        todos: vec![spec("new cut")?],
    }))?;
    assert_eq!(store.read(&sub_id)?.state, PlanState::Abandoned);
    let refused = engine.apply(at(
        &sub_id,
        Op::Supersede {
            reason: "resurrect".to_owned(),
            todos: vec![spec("zombie job")?],
        },
    ));
    assert!(matches!(refused, Err(PlanOpError::NotActive { .. })));
    assert_eq!(store.read(&sub_id)?.state, PlanState::Abandoned);
    Ok(())
}

#[test]
fn decompose_never_reissues_a_superseded_sub_plan_id() -> TestResult {
    let (_temp, store, _stub, engine) = harness(8)?;
    init(&engine, vec![spec("build the thing")?])?;
    engine.apply(owner(Op::Start {
        label: label("build the thing")?,
    }))?;
    let out = engine.apply(owner(Op::Decompose {
        label: label("build the thing")?,
        todos: vec![spec("first piece")?],
    }))?;
    let first_sub = out.subplan.ok_or("no sub-plan opened")?;
    engine.apply(owner(Op::Supersede {
        reason: "same label, new generation".to_owned(),
        todos: vec![spec("build the thing")?],
    }))?;
    engine.apply(owner(Op::Start {
        label: label("build the thing")?,
    }))?;
    let out = engine.apply(owner(Op::Decompose {
        label: label("build the thing")?,
        todos: vec![spec("second piece")?],
    }))?;
    let second_sub = out.subplan.ok_or("no second sub-plan")?;
    assert_ne!(
        second_sub, first_sub,
        "a superseded generation's ledger must never be overwritten"
    );
    let old = store.read(&first_sub)?;
    assert_eq!(old.state, PlanState::Abandoned);
    assert!(old.todo(&label("first piece")?).is_some());
    Ok(())
}

#[test]
fn add_edge_is_refused_on_an_abandoned_todo() -> TestResult {
    let (_temp, store, _stub, engine) = harness(8)?;
    let out = init(&engine, vec![spec("first job")?, spec("second job")?])?;
    engine.apply(owner(Op::Drop {
        label: label("second job")?,
        disposition: None,
    }))?;
    let refused = engine.apply(owner(Op::AddEdge {
        todo: label("second job")?,
        after: label("first job")?,
    }));
    assert!(matches!(refused, Err(PlanOpError::IllegalStep { .. })));
    let file = store.read(&out.plan.id)?;
    let dropped = file.todo(&label("second job")?).ok_or("todo missing")?;
    assert!(
        dropped.after.is_empty(),
        "an abandoned todo must not mutate"
    );
    Ok(())
}

#[test]
fn a_failed_last_todo_reopens_on_retry() -> TestResult {
    let (_temp, store, _stub, engine) = harness(8)?;
    let out = init(&engine, vec![spec("only job")?])?;
    let id = out.plan.id.clone();
    engine.apply(owner(Op::Start {
        label: label("only job")?,
    }))?;
    engine.apply(owner(Op::Fail {
        label: label("only job")?,
        cause: "first attempt sank".to_owned(),
        disposition: None,
    }))?;
    assert_eq!(store.read(&id)?.state, PlanState::Done);
    engine.apply(at(
        &id,
        Op::Retry {
            label: label("only job")?,
            delegation: None,
        },
    ))?;
    let file = store.read(&id)?;
    assert_eq!(file.state, PlanState::Active, "a retried plan must reopen");
    let out = engine.apply(owner(Op::View { full: false }))?;
    assert_eq!(out.plan.id, id, "resolve(None) must find the reopened plan");
    Ok(())
}

#[test]
fn apply_refuses_while_the_store_lease_is_held() -> TestResult {
    let (_temp, store, _stub, engine) = harness(8)?;
    init(&engine, vec![spec("first job")?])?;
    let held = store.lease()?;
    let refused = engine.apply(owner(Op::Append {
        todos: vec![spec("second job")?],
    }));
    assert!(matches!(
        refused,
        Err(PlanOpError::Store(StoreError::LeaseHeld { .. }))
    ));
    drop(held);
    engine.apply(owner(Op::Append {
        todos: vec![spec("second job")?],
    }))?;
    Ok(())
}

#[test]
fn a_refused_write_publishes_no_extra_files() -> TestResult {
    let (_temp, store, _stub, engine) = harness(8)?;
    let out = init(&engine, vec![spec("big piece")?])?;
    let id = out.plan.id.clone();
    engine.apply(owner(Op::Start {
        label: label("big piece")?,
    }))?;
    let mut file = store.read(&id)?;
    let sub_id = file.id.child(&label("big piece")?)?;
    let mut probe = file.clone();
    for todo in &mut probe.todos {
        todo.subplan = Some(sub_id.clone());
    }
    let seed = "x".repeat(10);
    probe.extra.insert("pad".to_owned(), seed.clone().into());
    let rendered = PlanStore::render(&probe)?;
    let filler = PLAN_CAP_BYTES
        .saturating_add(1)
        .saturating_sub(rendered.len().saturating_sub(seed.len()));
    file.extra
        .insert("pad".to_owned(), "x".repeat(filler).into());
    store.write(&file)?;
    let refused = engine.apply(owner(Op::Decompose {
        label: label("big piece")?,
        todos: vec![spec("small piece")?],
    }));
    assert!(matches!(
        refused,
        Err(PlanOpError::Store(StoreError::PlanOverCap { .. }))
    ));
    assert!(
        !store.exists(&sub_id),
        "a refusal that says nothing was written must have written nothing"
    );
    Ok(())
}

#[test]
fn start_refuses_unmet_after_edges() -> TestResult {
    let (_temp, store, _stub, engine) = harness(8)?;
    let follows = TodoSpec {
        label: label("second job")?,
        after: vec![label("first job")?],
        delegation: Some(delegation()),
        contract: None,
        children: Vec::new(),
    };
    let out = init(&engine, vec![spec("first job")?, follows])?;
    let refused = engine.apply(owner(Op::Start {
        label: label("second job")?,
    }));
    match refused {
        Err(PlanOpError::UnmetEdge { label: at, after }) => {
            assert_eq!(at, label("second job")?);
            assert_eq!(after, label("first job")?);
        }
        other => return Err(format!("expected an unmet-edge refusal, got {other:?}").into()),
    }
    let file = store.read(&out.plan.id)?;
    let second = file.todo(&label("second job")?).ok_or("todo missing")?;
    assert_eq!(
        second.state,
        TodoState::Pending,
        "the engine left it waiting too"
    );
    engine.apply(owner(Op::Start {
        label: label("first job")?,
    }))?;
    let out = engine.apply(owner(Op::Done {
        label: label("first job")?,
        output: None,
    }))?;
    assert_eq!(
        out.spawned.len(),
        1,
        "the cleared edge starts it in the same op"
    );
    let second = out.plan.todo(&label("second job")?).ok_or("todo missing")?;
    assert!(matches!(second.state, TodoState::Running { .. }));
    Ok(())
}

#[test]
fn retry_refuses_past_the_cap() -> TestResult {
    let (_temp, store, _stub, engine) = harness(8)?;
    let out = init(&engine, vec![spec("flaky job")?])?;
    let id = out.plan.id.clone();
    let mut file = store.read(&id)?;
    for todo in &mut file.todos {
        todo.state = TodoState::Failed {
            cause: "worn out".to_owned(),
            last: None,
        };
        todo.retries = RETRY_CAP;
    }
    store.write(&file)?;
    let refused = engine.apply(at(
        &id,
        Op::Retry {
            label: label("flaky job")?,
            delegation: None,
        },
    ));
    match refused {
        Err(PlanOpError::RetriesExhausted { spent, cap, .. }) => {
            assert_eq!(spent, RETRY_CAP);
            assert_eq!(cap, RETRY_CAP);
        }
        other => return Err(format!("expected a retry-cap refusal, got {other:?}").into()),
    }
    let file = store.read(&id)?;
    let todo = file.todo(&label("flaky job")?).ok_or("todo missing")?;
    assert_eq!(
        todo.retries, RETRY_CAP,
        "the counter must not move on refusal"
    );
    Ok(())
}

// Regressions from the 2026-09-01 recheck, one per confirmed defect.

#[test]
fn an_abandoned_sub_plan_is_not_drivable() -> TestResult {
    let (_temp, store, stub, engine) = harness(8)?;
    init(&engine, vec![spec("parent job")?])?;
    engine.apply(owner(Op::Start {
        label: label("parent job")?,
    }))?;
    let out = engine.apply(owner(Op::Decompose {
        label: label("parent job")?,
        todos: vec![delegated_spec("small piece")?],
    }))?;
    let sub_id = out.subplan.ok_or("no sub-plan opened")?;
    if let Ok(mut last) = stub.reap_last.lock() {
        *last = Some("history://small-piece/e4".parse::<Url>()?);
    }
    engine.apply(owner(Op::Supersede {
        reason: "wrong cut".to_owned(),
        todos: vec![spec("fresh job")?],
    }))?;
    assert_eq!(store.read(&sub_id)?.state, PlanState::Abandoned);
    let spawned_before = stub.next.load(Ordering::SeqCst);
    let refused = engine.apply(at(
        &sub_id,
        Op::Retry {
            label: label("small piece")?,
            delegation: None,
        },
    ));
    assert!(
        matches!(refused, Err(PlanOpError::NotActive { .. })),
        "retry drove an abandoned sub-plan: {refused:?}"
    );
    let refused = engine.apply(at(
        &sub_id,
        Op::Start {
            label: label("small piece")?,
        },
    ));
    assert!(
        matches!(refused, Err(PlanOpError::NotActive { .. })),
        "start drove an abandoned sub-plan: {refused:?}"
    );
    assert_eq!(
        stub.next.load(Ordering::SeqCst),
        spawned_before,
        "a ghost child was spawned into abandoned work"
    );
    let piece = store
        .read(&sub_id)?
        .todo(&label("small piece")?)
        .cloned()
        .ok_or("sub todo missing")?;
    assert!(matches!(piece.state, TodoState::Failed { .. }));
    engine.apply(at(&sub_id, Op::View { full: false }))?;
    Ok(())
}

#[test]
fn a_plan_whose_last_todo_failed_stays_reachable_unnamed() -> TestResult {
    let (_temp, store, _stub, engine) = harness(8)?;
    let out = init(&engine, vec![spec("only job")?])?;
    let id = out.plan.id.clone();
    engine.apply(owner(Op::Start {
        label: label("only job")?,
    }))?;
    engine.apply(owner(Op::Fail {
        label: label("only job")?,
        cause: "first attempt sank".to_owned(),
        disposition: None,
    }))?;
    assert_eq!(store.read(&id)?.state, PlanState::Done);
    let out = engine.apply(owner(Op::View { full: false }))?;
    assert_eq!(
        out.plan.id, id,
        "unnamed view must find the plan whose last todo failed"
    );
    let out = engine.apply(owner(Op::Retry {
        label: label("only job")?,
        delegation: None,
    }))?;
    assert_eq!(out.plan.id, id);
    assert_eq!(out.plan.state, PlanState::Active, "retry must reopen it");
    engine.apply(owner(Op::Start {
        label: label("only job")?,
    }))?;
    engine.apply(owner(Op::Done {
        label: label("only job")?,
        output: None,
    }))?;
    let refused = engine.apply(owner(Op::View { full: false }));
    assert!(
        matches!(refused, Err(PlanOpError::NoPlan)),
        "a genuinely finished plan must never be resurrected: {refused:?}"
    );
    Ok(())
}

struct Probe;

impl OutputResolve for Probe {
    fn resolve(&self, url: &Url) -> Result<Option<String>, String> {
        if url.to_string().contains("never-written") {
            Err("not found".to_owned())
        } else {
            Ok(None)
        }
    }
}

/// Serves bytes for both halves of the check: the declared schema and the
/// product the `done` names.
struct Served(&'static str, &'static str);

impl OutputResolve for Served {
    fn resolve(&self, url: &Url) -> Result<Option<String>, String> {
        let rendered = url.to_string();
        if rendered.contains("schema") {
            Ok(Some(self.0.to_owned()))
        } else {
            Ok(Some(self.1.to_owned()))
        }
    }
}

fn declaring(schema: &str) -> Result<Delegation, Box<dyn Error>> {
    let mut declared = delegation();
    declared.output = Some(OutputSchema {
        schema: schema.parse::<Url>()?,
        extra: Map::new(),
    });
    Ok(declared)
}

const REPORT_SCHEMA: &str =
    r#"{"type":"object","required":["passed"],"properties":{"passed":{"type":"boolean"}}}"#;

#[test]
fn a_declared_output_is_validated_against_its_schema() -> TestResult {
    let (_temp, _store, _stub, engine) = harness(8)?;
    let engine = engine.with_output_resolve(Arc::new(Served(REPORT_SCHEMA, r#"{"passed":"yes"}"#)));
    init(
        &engine,
        vec![TodoSpec {
            label: label("write the report")?,
            after: Vec::new(),
            delegation: Some(declaring("local://schemas/report.json")?),
            contract: None,
            children: Vec::new(),
        }],
    )?;
    let refused = engine.apply(owner(Op::Done {
        label: label("write the report")?,
        output: Some("kernel://main/report".parse::<Url>()?),
    }));
    match refused {
        Err(error @ PlanOpError::OutputMismatch { .. }) => {
            let told = error.to_string();
            assert!(
                told.contains("expected boolean"),
                "the refusal must name the mismatch: {told}"
            );
        }
        other => return Err(format!("expected a schema mismatch, got {other:?}").into()),
    }
    Ok(())
}

#[test]
fn a_product_that_satisfies_its_schema_completes() -> TestResult {
    let (_temp, _store, _stub, engine) = harness(8)?;
    let engine = engine.with_output_resolve(Arc::new(Served(REPORT_SCHEMA, r#"{"passed":true}"#)));
    init(
        &engine,
        vec![TodoSpec {
            label: label("write the report")?,
            after: Vec::new(),
            delegation: Some(declaring("local://schemas/report.json")?),
            contract: None,
            children: Vec::new(),
        }],
    )?;
    let out = engine.apply(owner(Op::Done {
        label: label("write the report")?,
        output: Some("local://reports/final.json".parse::<Url>()?),
    }))?;
    let todo = out
        .plan
        .todo(&label("write the report")?)
        .ok_or("todo missing")?;
    assert!(matches!(
        todo.state,
        TodoState::Done {
            output: Some(_),
            ..
        }
    ));
    Ok(())
}

#[test]
fn a_schema_that_is_not_json_refuses_the_done_naming_the_schema() -> TestResult {
    let (_temp, _store, _stub, engine) = harness(8)?;
    let engine = engine.with_output_resolve(Arc::new(Served("not json at all", "{}")));
    init(
        &engine,
        vec![TodoSpec {
            label: label("write the report")?,
            after: Vec::new(),
            delegation: Some(declaring("local://schemas/report.json")?),
            contract: None,
            children: Vec::new(),
        }],
    )?;
    let refused = engine.apply(owner(Op::Done {
        label: label("write the report")?,
        output: Some("local://reports/final.json".parse::<Url>()?),
    }));
    match refused {
        Err(error @ PlanOpError::UnusableSchema { .. }) => {
            assert!(
                error.to_string().contains("local://schemas/report.json"),
                "the refusal must name the schema: {error}"
            );
        }
        other => return Err(format!("expected an unusable-schema refusal, got {other:?}").into()),
    }
    Ok(())
}

#[test]
fn a_supersede_refused_on_its_new_cut_kills_no_child() -> TestResult {
    let (_temp, store, stub, engine) = harness(8)?;
    let out = init(&engine, vec![delegated_spec("delegated job")?])?;
    let reaped_before = stub.reaps.load(Ordering::SeqCst);
    let refused = engine.apply(owner(Op::Supersede {
        reason: "rethink".to_owned(),
        todos: vec![spec("same label")?, spec("same label")?],
    }));
    assert!(matches!(refused, Err(PlanOpError::LabelNotUnique { .. })));
    assert_eq!(
        stub.reaps.load(Ordering::SeqCst),
        reaped_before,
        "a supersede refused on its own new cut must not have killed anything first"
    );
    let file = store.read(&out.plan.id)?;
    let todo = file.todo(&label("delegated job")?).ok_or("todo missing")?;
    assert!(matches!(todo.state, TodoState::Running { .. }));
    Ok(())
}

#[test]
fn a_declared_output_must_resolve_at_done() -> TestResult {
    let (_temp, _store, _stub, engine) = harness(8)?;
    let engine = engine.with_output_resolve(Arc::new(Probe));
    let mut declared = delegation();
    declared.output = Some(OutputSchema {
        schema: "local://schemas/report.json".parse::<Url>()?,
        extra: Map::new(),
    });
    init(
        &engine,
        vec![TodoSpec {
            label: label("write the report")?,
            after: Vec::new(),
            delegation: Some(declared),
            contract: None,
            children: Vec::new(),
        }],
    )?;
    let unresolved = "local://nowhere/never-written.txt";
    let refused = engine.apply(owner(Op::Done {
        label: label("write the report")?,
        output: Some(unresolved.parse::<Url>()?),
    }));
    match refused {
        Err(error @ PlanOpError::UnresolvedOutput { .. }) => {
            assert!(
                error.to_string().contains(unresolved),
                "the refusal must name the url that did not resolve: {error}"
            );
        }
        other => {
            return Err(format!("expected an unresolved-output refusal, got {other:?}").into());
        }
    }
    // F0c: a product that resolves but is not served is a refusal, never the implicit pass
    // this branch used to pin (plan section 6.3, step 4).
    let unserved = engine.apply(owner(Op::Done {
        label: label("write the report")?,
        output: Some("local://reports/final.txt".parse::<Url>()?),
    }));
    assert!(
        matches!(unserved, Err(PlanOpError::UnservedOutput { .. })),
        "{unserved:?}"
    );
    Ok(())
}

fn row(text: &str, state: TodoStateName, children: &[&str]) -> Result<SetRow, Box<dyn Error>> {
    let mut kids = Vec::new();
    for child in children {
        kids.push(Todo {
            label: label(child)?,
            after: Vec::new(),
            state: TodoState::Pending,
            delegation: None,
            subplan: None,
            retries: RetryCount::default(),
            children: Vec::new(),
            note: None,
            attempt: yi_types::plan::doc::AttemptId::FIRST,
            refusals: 0,
            contract: None,
            contract_hash: None,
            extra: Map::new(),
        });
    }
    let mut spec = spec(text)?;
    spec.children = kids;
    Ok(SetRow { spec, state })
}

#[test]
fn set_needs_a_goal_to_open_a_plan_then_replaces_the_whole_cut() -> TestResult {
    let (_temp, _store, _stub, engine) = harness(4)?;
    let refused = engine.apply(owner(Op::Set {
        goal: None,
        rows: vec![row("mapper", TodoStateName::Pending, &[])?],
    }));
    assert!(
        matches!(refused, Err(PlanOpError::NoPlan)),
        "a goal-less set with no plan open is the todo tool's job, not a plan named checklist"
    );
    let first = engine.apply(owner(Op::Set {
        goal: Some(GoalText::new("checklist")?),
        rows: vec![
            row("mapper", TodoStateName::Done, &[])?,
            row("rebase", TodoStateName::Running, &["remap", "prompt"])?,
            row("bridge", TodoStateName::Pending, &[])?,
        ],
    }))?;
    assert_eq!(first.plan.goal.as_str(), "checklist");
    let states: Vec<TodoStateName> = first
        .plan
        .todos
        .iter()
        .map(|todo| TodoStateName::of(&todo.state))
        .collect();
    assert_eq!(
        states,
        [
            TodoStateName::Done,
            TodoStateName::Running,
            TodoStateName::Pending
        ]
    );
    assert_eq!(first.plan.todos[1].children.len(), 2);
    let progress = yi_types::plan::doc::progress(&first.plan.todos);
    assert_eq!((progress.done, progress.total), (1, 5));
    assert_eq!(
        progress.running.as_ref().map(TodoLabel::as_str),
        Some("rebase")
    );

    engine.apply(at(
        &first.plan.id,
        Op::AddEdge {
            todo: label("bridge")?,
            after: label("rebase")?,
        },
    ))?;
    let second = engine.apply(owner(Op::Set {
        goal: None,
        rows: vec![
            row("rebase", TodoStateName::Done, &["remap"])?,
            row("bridge", TodoStateName::Running, &[])?,
            row("grep", TodoStateName::Pending, &[])?,
        ],
    }))?;
    assert_eq!(second.plan.id, first.plan.id, "set keeps the plan");
    assert!(
        second.plan.version > first.plan.version,
        "set bumps the version"
    );
    let labels: Vec<&str> = second
        .plan
        .todos
        .iter()
        .map(|todo| todo.label.as_str())
        .collect();
    assert_eq!(
        labels,
        ["rebase", "bridge", "grep"],
        "mapper left, grep arrived"
    );
    assert!(matches!(second.plan.todos[0].state, TodoState::Done { .. }));
    assert_eq!(second.plan.todos[0].children.len(), 1);
    assert_eq!(
        second.plan.todos[1].after,
        vec![label("rebase")?],
        "a surviving label keeps its edge"
    );
    assert!(matches!(
        second.plan.todos[1].state,
        TodoState::Running { .. }
    ));
    Ok(())
}

/// Guards the fuse's second writer: `Plan::reset_spawns` runs only under the confirmed
/// `fuse_reset` op, which the owner cannot mint and a supersede never triggers.
#[test]
fn fuse_reset_is_the_only_writer_that_lowers_spawns() -> TestResult {
    let (_temp, store, _stub, engine) = harness(8)?;
    let out = init(
        &engine,
        vec![delegated_spec("first job")?, delegated_spec("second job")?],
    )?;
    let id = out.plan.id.clone();
    assert_eq!(store.read(&id)?.spawns().get(), 2);
    let superseded = engine.apply(owner(Op::Supersede {
        reason: "the cut was wrong".to_owned(),
        todos: vec![spec("plain job")?],
    }))?;
    assert_eq!(
        superseded.plan.spawns().get(),
        2,
        "a supersede never lowers the fuse"
    );
    let refused = engine.apply(owner(Op::FuseReset));
    assert!(
        matches!(refused, Err(PlanOpError::NotOwner { .. })),
        "the owner cannot reset the fuse: {refused:?}"
    );
    let reset = engine.apply(OpRequest {
        plan: None,
        actor: Actor::User("user://7".parse::<Url>()?),
        op: Op::FuseReset,
        request_id: None,
        expected_revision: None,
    })?;
    assert_eq!(reset.plan.spawns().get(), 0);
    let records = store.journal(&id).read()?.records;
    let record = records.last().ok_or("no record")?;
    assert_eq!(record.record.op, "fuse_reset");
    assert_eq!(
        record.record.actor, "user://7",
        "the citation is stored verbatim"
    );
    assert_eq!(
        record.record.extra.get("prior"),
        Some(&serde_json::json!(2))
    );
    let recovered = yi_runtime::plan::state::reduce(&records)?;
    assert_eq!(recovered.plan(&id)?.spawns().get(), 0, "the reducer agrees");
    engine.apply(owner(Op::Start {
        label: label("plain job")?,
    }))?;
    assert_eq!(
        store.read(&id)?.spawns().get(),
        0,
        "an inline start charges nothing"
    );
    Ok(())
}

/// A retry after a crash between `spawn_intent` and its result is never a second spawn: the
/// standing intent refuses the start until repair reconciles it, and the fuse was charged once.
#[test]
fn a_pending_spawn_intent_refuses_a_second_start() -> TestResult {
    let (_temp, store, stub, engine) = harness(8)?;
    let mut gated = delegated_spec("delegated job")?;
    gated.after = vec![label("gate")?];
    let out = init(&engine, vec![spec("gate")?, gated])?;
    let id = out.plan.id.clone();
    let intent = store.journal(&id).read()?;
    let last = intent.records.last().ok_or("no init")?;
    let mut draft = last.clone();
    draft.record.op = "spawn_intent".to_owned();
    draft.record.todo = Some(label("delegated job")?);
    draft.record.from = None;
    draft.record.to = None;
    draft.args =
        serde_json::json!({"label": "delegated job", "attempt": 1, "effect_id": "e-crash"});
    draft.request_id = yi_types::plan::ledger::RequestId::new("r-crash/intent")?;
    draft.attempt = Some(yi_types::plan::ledger::AttemptId::FIRST);
    let journal = store.journal(&id);
    let sealed = journal.seal(draft, Some(last))?;
    journal.append(&sealed)?;
    let refusals = || -> Result<usize, Box<dyn Error>> {
        Ok(store
            .journal(&id)
            .read()?
            .records
            .iter()
            .filter(|record| record.record.extra.contains_key("refusal"))
            .count())
    };
    engine.apply(owner(Op::Start {
        label: label("gate")?,
    }))?;
    let out = engine.apply(owner(Op::Done {
        label: label("gate")?,
        output: None,
    }))?;
    assert!(
        out.spawned.is_empty(),
        "the engine skips the standing intent"
    );
    assert_eq!(refusals()?, 0, "and records no refusal for it");
    let refused = engine.apply(owner(Op::Start {
        label: label("delegated job")?,
    }));
    assert!(
        matches!(refused, Err(PlanOpError::NeedsReconciliation { .. })),
        "{refused:?}"
    );
    engine.apply(owner(Op::Append {
        todos: vec![spec("later job")?],
    }))?;
    assert_eq!(
        refusals()?,
        1,
        "the owner's start is the one refusal; later ops add none"
    );
    assert_eq!(stub.next.load(Ordering::SeqCst), 0, "nothing was spawned");
    assert_eq!(
        store.read(&id)?.spawns().get(),
        1,
        "the committed intent charged the fuse once"
    );
    Ok(())
}

/// The confirmed path the console rpc drives: the owner's `fuse_reset` is put to the human
/// through the session's broker, the answer lands as an attributed user message, and the record
/// cites that message verbatim. With no prompt to ask, or a no, nothing is journaled.
#[test]
fn a_user_op_is_recorded_with_its_user_citation() -> TestResult {
    use yi_runtime::plan::authority::{Confirmer, Submission, SubmitError, submit};
    use yi_runtime::session_store::{CreateOptions, JsonlRepo, SessionRepo};
    use yi_runtime::{AskOutcome, Asker, PermissionAsk, PermissionBroker, PermissionMode};
    use yi_types::message::UserContent;

    let (temp, store, _stub, engine) = harness(8)?;
    let out = init(&engine, vec![delegated_spec("delegated job")?])?;
    let id = out.plan.id.clone();
    assert_eq!(store.read(&id)?.spawns().get(), 1);
    std::fs::create_dir_all(temp.join("sessions"))?;
    let mut repo = JsonlRepo::new(temp.join("sessions"), temp.to_string_lossy().into_owned());
    let session = repo.create(CreateOptions::default())?;
    let asked: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let broker_with = |answer: AskOutcome| -> Result<Arc<PermissionBroker>, Box<dyn Error>> {
        let seen = Arc::clone(&asked);
        let asker: Asker = Arc::new(move |ask: &PermissionAsk<'_>| {
            if let Ok(mut log) = seen.lock() {
                log.push(ask.text());
            }
            answer
        });
        let (events, _nobody_listens) = tokio::sync::broadcast::channel(8);
        Ok(Arc::new(PermissionBroker::new(
            PermissionMode::Ask,
            temp.to_path_buf(),
            Vec::new(),
            Some(asker),
            events,
        )))
    };
    let submission = |request: &str| -> Result<Submission, Box<dyn Error>> {
        let mut args = Map::new();
        args.insert("op".to_owned(), "fuse_reset".into());
        Ok(Submission {
            args,
            request_id: Some(yi_types::plan::ledger::RequestId::new(request)?),
            expected_revision: None,
        })
    };

    let refused = submit(&engine, &Actor::Owner, None, submission("confirm-1")?);
    assert!(
        matches!(refused, Err(SubmitError::NoConfirmer { .. })),
        "no prompt, no user: {refused:?}"
    );
    let declining = Confirmer {
        broker: broker_with(AskOutcome::Reject)?,
        store: Arc::clone(&session),
    };
    let declined = submit(
        &engine,
        &Actor::Owner,
        Some(&declining),
        submission("confirm-1")?,
    );
    assert!(
        matches!(declined, Err(SubmitError::Declined { .. })),
        "{declined:?}"
    );
    assert_eq!(store.read(&id)?.spawns().get(), 1, "a no changes nothing");
    assert!(yi_runtime::fetch::user_inputs(&session)?.is_empty());

    let confirming = Confirmer {
        broker: broker_with(AskOutcome::AllowAlways(0))?,
        store: Arc::clone(&session),
    };
    let seen_revision = store.read(&id)?.touched.0;
    let applied = submit(
        &engine,
        &Actor::Owner,
        Some(&confirming),
        submission("confirm-2")?,
    )?;
    assert_eq!(applied.outcome.plan.spawns().get(), 0);
    let records = store.journal(&id).read()?.records;
    let record = records.last().ok_or("no record")?;
    assert_eq!(record.record.op, "fuse_reset");
    assert_eq!(
        record.record.actor, "user://1",
        "the citation is the answer's own ordinal, stored verbatim"
    );
    assert_eq!(
        record.expected_revision, seen_revision,
        "bound to the revision the human saw"
    );
    let inputs = yi_runtime::fetch::user_inputs(&session)?;
    let answer = match inputs.as_slice() {
        [UserContent::Text(text)] => text.clone(),
        other => return Err(format!("expected one attributed message, got {other:?}").into()),
    };
    assert!(
        answer.contains(id.as_str())
            && answer.contains("fuse_reset")
            && answer.contains(&format!("revision {seen_revision}")),
        "{answer}"
    );
    let asks = asked.lock().map_err(|_| "poisoned")?.clone();
    assert_eq!(
        asks.len(),
        2,
        "one question per submission that reached a prompt"
    );
    assert!(
        asks[1].contains("(args "),
        "the binding names the args hash: {}",
        asks[1]
    );
    Ok(())
}

/// The record cap is a refusal before the first effect: a supersede whose record would not fit
/// (a reason no render bounds) kills no child and leaves the plan on its old cut.
#[test]
fn a_record_over_the_cap_is_refused_before_any_reap() -> TestResult {
    let (_temp, store, stub, engine) = harness(8)?;
    let out = init(&engine, vec![delegated_spec("delegated job")?])?;
    let reaped_before = stub.reaps.load(Ordering::SeqCst);
    let refused = engine.apply(owner(Op::Supersede {
        reason: "x".repeat(70 * 1024),
        todos: vec![spec("the next cut")?],
    }));
    assert!(
        matches!(
            refused,
            Err(PlanOpError::Store(StoreError::Journal(
                JournalError::RecordOverCap { .. }
            )))
        ),
        "{refused:?}"
    );
    assert_eq!(
        stub.reaps.load(Ordering::SeqCst),
        reaped_before,
        "the reap ran before the record cap was checked"
    );
    let file = store.read(&out.plan.id)?;
    let todo = file.todo(&label("delegated job")?).ok_or("todo missing")?;
    assert!(matches!(todo.state, TodoState::Running { .. }));
    assert_eq!(file.version, out.plan.version, "the cut moved");
    Ok(())
}

/// Admission is the engine's refusal, with the count: the ninth delegated start at width
/// eight is refused before any spawn intent, and nothing is silently held.
#[test]
fn a_ninth_delegated_start_is_refused_by_the_engine_with_the_count() -> TestResult {
    let (_temp, store, stub, engine) = harness(8)?;
    let specs = (1..=9)
        .map(|index| delegated_spec(&format!("job {index}")))
        .collect::<Result<Vec<_>, _>>()?;
    let out = init(&engine, specs)?;
    assert_eq!(
        out.spawned.len(),
        8,
        "the engine starts the eight the width admits"
    );
    assert_eq!(out.held, vec![label("job 9")?]);
    let refused = engine.apply(owner(Op::Start {
        label: label("job 9")?,
    }));
    match refused {
        Err(PlanOpError::Admission(refusal)) => {
            assert_eq!(refusal.slots, 0);
            assert_eq!(refusal.position, 1);
            assert_eq!(refusal.delegated_ready, 1);
        }
        other => return Err(format!("expected an admission refusal, got {other:?}").into()),
    }
    assert_eq!(
        stub.next.load(Ordering::SeqCst),
        8,
        "the refused start spawned"
    );
    let engine_refusals = store
        .journal(&out.plan.id)
        .read()?
        .records
        .iter()
        .filter(|record| {
            record.record.actor == "engine" && record.record.extra.contains_key("refusal")
        })
        .count();
    assert_eq!(
        engine_refusals, 0,
        "a held todo is not tried, so nothing is refused"
    );
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// F0c: completion is verified on every path.

/// Incident: every one of these refusals named the rule it enforced and not the move the
/// caller evidently wanted, and F0e sessions repeated the same call up to three times (#472).
mod refusals {
    use super::*;
    use yi_runtime::plan::tool::PlanTool;
    use yi_tools::{Tool, ToolContext};

    fn refusal(tool: &PlanTool, args: serde_json::Value) -> String {
        let input = args.as_object().cloned().unwrap_or_default();
        let output = tool.execute(input, &ToolContext::new(std::env::temp_dir()));
        assert!(output.is_error, "the call was admitted: {output:?}");
        output
            .result
            .content
            .iter()
            .map(|content| match content {
                yi_types::message::Content::Text { text, .. } => text.clone(),
                _ => String::new(),
            })
            .collect()
    }

    fn tool() -> Result<(Scratch, PlanTool), Box<dyn Error>> {
        let (temp, _store, _stub, engine) = harness(2)?;
        Ok((temp, PlanTool::new(Arc::new(engine), Actor::Owner)))
    }

    #[test]
    fn done_on_a_pending_todo_names_the_legal_move() -> TestResult {
        let (_temp, tool) = tool()?;
        let opened = tool.execute(
            serde_json::json!({"op": "init", "goal": "ship it", "todos": [{"label": "tablefmt"}]})
                .as_object()
                .cloned()
                .unwrap_or_default(),
            &ToolContext::new(std::env::temp_dir()),
        );
        assert!(!opened.is_error, "{opened:?}");
        let text = refusal(
            &tool,
            serde_json::json!({"op": "done", "label": "tablefmt"}),
        );
        assert!(text.contains("in state pending"), "{text}");
        assert!(text.contains("start it first"), "{text}");
        assert!(text.contains("- [x]"), "{text}");
        Ok(())
    }

    #[test]
    fn a_prose_output_names_the_url_shapes_and_the_evidence_field() -> TestResult {
        let (_temp, tool) = tool()?;
        let text = refusal(
            &tool,
            serde_json::json!({
                "op": "done",
                "label": "patchfuzz",
                "output": "python3 check.py patchfuzz -> ok 8 of 8 public cases pass",
            }),
        );
        assert!(text.contains("output is a url of the product"), "{text}");
        assert!(text.contains("file:///abs/path"), "{text}");
        assert!(text.contains("todo tool's evidence"), "{text}");
        Ok(())
    }

    #[test]
    fn an_over_long_set_label_is_measured_not_called_a_bad_row() -> TestResult {
        let (_temp, tool) = tool()?;
        let long = "gateway: root failure, causal chain, blast radius from an interleaved log (check: python3 /app/check.py gateway)";
        let text = refusal(
            &tool,
            serde_json::json!({"op": "set", "goal": "ship it", "list": format!("- [ ] {long}\n")}),
        );
        assert!(
            text.contains(&format!(
                "label is {} chars, the cap is 80",
                long.chars().count()
            )),
            "{text}"
        );
        assert!(!text.contains("is not a checklist row"), "{text}");
        Ok(())
    }

    #[test]
    fn a_set_with_no_open_plan_names_goal() -> TestResult {
        let (_temp, tool) = tool()?;
        let text = refusal(
            &tool,
            serde_json::json!({"op": "set", "list": "- [ ] one\n"}),
        );
        assert!(text.contains("add goal to this set to open one"), "{text}");
        Ok(())
    }

    /// Three F0e trials lost a turn to this cap with briefs of 1428, 1490 and 1777 bytes; the
    /// cap stays, because a plan of forty noted delegations has a frontmatter budget (#471).
    #[test]
    fn an_over_long_inline_note_names_the_artifact_road() -> TestResult {
        let (_temp, tool) = tool()?;
        let opened = tool.execute(
            serde_json::json!({"op": "init", "goal": "ship it", "todos": [{"label": "seam"}]})
                .as_object()
                .cloned()
                .unwrap_or_default(),
            &ToolContext::new(std::env::temp_dir()),
        );
        assert!(!opened.is_error, "{opened:?}");
        let brief = "x".repeat(1490);
        let text = refusal(
            &tool,
            serde_json::json!({
                "op": "append",
                "todos": [{
                    "label": "gateway",
                    "delegation": {"spec": {}, "accept": {"command": "true"}, "note": brief},
                }],
            }),
        );
        assert!(text.contains("1490 bytes exceeds 1024"), "{text}");
        assert!(text.contains("artifact"), "{text}");
        assert!(text.contains("context"), "{text}");
        Ok(())
    }
}

mod contracts {
    use super::*;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    use serde_json::{Value, json};
    use yi_kernel::client::HostHandlers;
    use yi_runtime::HostRegistry;
    use yi_runtime::plan::authority::{Confirmer, Submission, SubmitError, submit};
    use yi_runtime::plan::capacity::{Capacity, Purpose};
    use yi_runtime::plan::journal::{Journal, RealFs};
    use yi_runtime::plan::verify::Verifier;
    use yi_runtime::session_store::{CreateOptions, JsonlRepo, SessionRepo};
    use yi_runtime::subagent::DEFAULT_MAX_CHILDREN;
    use yi_runtime::{AskOutcome, Asker, PermissionAsk, PermissionBroker, PermissionMode};
    use yi_types::plan::canonical::{ArtifactRef, Digest};
    use yi_types::plan::contract::{
        Contract, ContractItem, ItemVerdict, JurorLine, Outcome as VerdictOutcome, Resolution,
        Verdict, VerificationToken, Vote,
    };
    use yi_types::plan::doc::{AttemptId, Isolation};
    use yi_types::plan::ledger::JournalRecord;

    /// The `OutputResolve` stub: a url maps to its text, or to `None` for resolved but unserved.
    #[derive(Default)]
    pub(super) struct Serve(Mutex<HashMap<String, Option<String>>>);

    impl Serve {
        fn set(&self, url: &str, text: Option<&str>) {
            if let Ok(mut map) = self.0.lock() {
                map.insert(url.to_owned(), text.map(str::to_owned));
            }
        }
    }

    impl OutputResolve for Serve {
        fn resolve(&self, url: &Url) -> Result<Option<String>, String> {
            let map = self.0.lock().map_err(|_| "poisoned".to_owned())?;
            match map.get(&url.to_string()) {
                Some(answer) => Ok(answer.clone()),
                None => Err(format!("{url} is not served")),
            }
        }
    }

    struct Rig {
        _temp: Scratch,
        store: PlanStore,
        serve: Arc<Serve>,
        engine: Arc<PlanEngine>,
        ws: PathBuf,
    }

    fn rig(
        name: &str,
        hook: Option<yi_runtime::plan::ops::VerifyHook>,
    ) -> Result<Rig, Box<dyn Error>> {
        rig_verified(name, hook, Verifier::new(20_000))
    }

    fn rig_verified(
        name: &str,
        hook: Option<yi_runtime::plan::ops::VerifyHook>,
        verifier: Verifier,
    ) -> Result<Rig, Box<dyn Error>> {
        let temp = Scratch::new(name)?;
        let store = PlanStore::open(temp.join("plans"))?;
        let ws = temp.join("ws");
        std::fs::create_dir_all(&ws)?;
        let serve = Arc::new(Serve::default());
        let mut engine = PlanEngine::new(store.clone(), Arc::new(Stub::default()))
            .with_width(width(4)?)
            .with_output_resolve(serve.clone())
            .with_cwd(ws.clone())
            .with_verifier(verifier);
        if let Some(hook) = hook {
            engine = engine.with_verify_hook(hook);
        }
        Ok(Rig {
            _temp: temp,
            store,
            serve,
            engine: Arc::new(engine),
            ws,
        })
    }

    fn fixture(stem: &str) -> Result<Value, Box<dyn Error>> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/plans/contracts")
            .join(format!("{stem}.json"));
        Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
    }

    /// Every blob the fixture declares, written into the plan's store and checked against its
    /// declared digest.
    fn stage(store: &PlanStore, plan: &PlanId, doc: &Value) -> Result<(), Box<dyn Error>> {
        let artifacts = store.artifacts(plan);
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plans/contracts");
        for (alias, blob) in doc["artifacts"].as_object().ok_or("artifacts")? {
            let bytes = match (blob.get("text"), blob.get("file")) {
                (Some(Value::String(text)), _) => text.as_bytes().to_vec(),
                (_, Some(Value::String(file))) => std::fs::read(base.join(file))?,
                _ => return Err(format!("{alias} has neither text nor file").into()),
            };
            let media = blob["media_type"].as_str().ok_or("media_type")?;
            let put = artifacts.put(&bytes, media, &store.nonce())?;
            assert_eq!(
                put.digest.to_string(),
                blob["digest"].as_str().unwrap_or_default(),
                "{alias}: the fixture's digest must match its bytes"
            );
        }
        Ok(())
    }

    fn blob(doc: &Value, alias: &str) -> Result<String, Box<dyn Error>> {
        doc["artifacts"][alias]["text"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("{alias} has no inline text").into())
    }

    /// The init step's todo specs, contracts included, exactly as the fixture spells them.
    fn init_specs(doc: &Value) -> Result<(GoalText, Vec<TodoSpec>), Box<dyn Error>> {
        let step = doc["steps"][0].clone();
        let goal = GoalText::new(step["args"]["goal"].as_str().ok_or("goal")?)?;
        let mut specs = Vec::new();
        for todo in step["args"]["todos"].as_array().ok_or("todos")? {
            specs.push(TodoSpec {
                label: TodoLabel::new(todo["label"].as_str().ok_or("label")?)?,
                after: Vec::new(),
                delegation: match todo.get("delegation") {
                    Some(raw) => Some(serde_json::from_value(raw.clone())?),
                    None => None,
                },
                contract: match todo.get("contract") {
                    Some(raw) => Some(serde_json::from_value(raw.clone())?),
                    None => None,
                },
                children: Vec::new(),
            });
        }
        Ok((goal, specs))
    }

    /// A contract built here, not from a fixture: one critical cmd item over a manifest.
    fn cmd_contract(
        store: &PlanStore,
        plan: &PlanId,
        command: &str,
        class: &str,
    ) -> Result<Contract, Box<dyn Error>> {
        let manifest = json!({
            "manifest": 1, "command": command, "cwd": "snapshot_root", "cwd_subdir": null,
            "protected": [], "timeout_ms": 10_000, "env": [], "reads_outside_snapshot": false
        });
        let put = store.artifacts(plan).put(
            &serde_json::to_vec(&manifest)?,
            "application/vnd.yi.checker-manifest+json",
            &store.nonce(),
        )?;
        Ok(serde_json::from_value(json!({
            "class": class,
            "items": [{"id": "check", "critical": true, "weight": 100,
                       "decider": {"cmd": {"checker": put, "timeout_ms": 10_000}}}],
            "threshold": 1000, "min_coverage": 1000
        }))?)
    }

    /// The id `init` allocates for the shared goal, so artifacts can be staged before it.
    fn planned() -> Result<PlanId, Box<dyn Error>> {
        Ok(PlanId::slug("ship the widget end to end")?)
    }

    fn contracted(label: &str, contract: Contract) -> Result<TodoSpec, Box<dyn Error>> {
        let mut spec = spec(label)?;
        spec.contract = Some(contract);
        Ok(spec)
    }

    fn done(
        engine: &PlanEngine,
        plan: &PlanId,
        label: &str,
        output: Option<&str>,
    ) -> Result<Outcome, PlanOpError> {
        let output = match output {
            Some(url) => Some(url.parse::<Url>().map_err(|_| PlanOpError::NoPlan)?),
            None => None,
        };
        engine.apply(at(
            plan,
            Op::Done {
                label: TodoLabel::new(label).map_err(PlanOpError::Doc)?,
                output,
            },
        ))
    }

    fn start(engine: &PlanEngine, plan: &PlanId, label: &str) -> Result<Outcome, PlanOpError> {
        engine.apply(at(
            plan,
            Op::Start {
                label: TodoLabel::new(label).map_err(PlanOpError::Doc)?,
            },
        ))
    }

    fn kinds(store: &PlanStore, plan: &PlanId) -> Result<Vec<String>, Box<dyn Error>> {
        let journal = Journal::open(store.journal_path(plan), Arc::new(RealFs));
        Ok(journal
            .read()?
            .records
            .iter()
            .map(|record| record.record.op.clone())
            .collect())
    }

    fn verdicts(store: &PlanStore, plan: &PlanId) -> Result<Vec<Verdict>, Box<dyn Error>> {
        let journal = Journal::open(store.journal_path(plan), Arc::new(RealFs));
        journal
            .read()?
            .records
            .iter()
            .filter_map(|record| record.verdict.clone())
            .map(|value| Ok(serde_json::from_value(value)?))
            .collect()
    }

    fn todo_of(store: &PlanStore, plan: &PlanId, label: &str) -> Result<Todo, Box<dyn Error>> {
        Ok(store
            .read(plan)?
            .todo(&TodoLabel::new(label)?)
            .cloned()
            .ok_or("todo missing")?)
    }

    fn refused(result: Result<Outcome, PlanOpError>) -> Result<Verdict, Box<dyn Error>> {
        match result {
            Err(PlanOpError::Refused { verdict, .. }) => Ok(*verdict),
            other => Err(format!("expected a refusal with a verdict, got {other:?}").into()),
        }
    }

    // Dies with the `TodoStateName::Done` arm of `apply_set` in state.rs: restore the direct
    // `TodoState::Done` write and the first set completes a todo nothing verified.
    #[test]
    fn set_cannot_complete_a_failing_task() -> TestResult {
        let doc = fixture("set-cannot-complete")?;
        let rig = rig("yi-f0c-set", None)?;
        let (goal, specs) = init_specs(&doc)?;
        let opened = rig.engine.apply(owner(Op::Init { goal, todos: specs }))?;
        let plan = opened.plan.id.clone();
        stage(&rig.store, &plan, &doc)?;
        start(&rig.engine, &plan, "Package the tarball")?;
        let mut announce = spec("Announce the release")?;
        announce.contract = None;
        let refused = rig.engine.apply(at(
            &plan,
            Op::Set {
                goal: None,
                rows: vec![
                    SetRow {
                        spec: spec("Package the tarball")?,
                        state: TodoStateName::Done,
                    },
                    SetRow {
                        spec: announce,
                        state: TodoStateName::Pending,
                    },
                ],
            },
        ));
        let message = match refused {
            Err(error @ PlanOpError::NoVerifiedCompletion { .. }) => error.to_string(),
            other => return Err(format!("expected no verified completion, got {other:?}").into()),
        };
        assert_eq!(
            message,
            "no verified completion for \"Package the tarball\""
        );
        let todo = todo_of(&rig.store, &plan, "Package the tarball")?;
        assert!(
            matches!(todo.state, TodoState::Running { .. }),
            "the set applied nothing"
        );
        assert_eq!(todo.refusals, 1, "the refusal is charged to the named row");
        assert!(
            rig.store
                .read(&plan)?
                .todo(&label("Announce the release")?)
                .is_none(),
            "one transaction"
        );
        assert_eq!(
            kinds(&rig.store, &plan)?.last().map(String::as_str),
            Some("set")
        );
        // The control: a set asking only for blocked applies.
        rig.engine.apply(at(
            &plan,
            Op::Set {
                goal: None,
                rows: vec![SetRow {
                    spec: spec("Package the tarball")?,
                    state: TodoStateName::Blocked,
                }],
            },
        ))?;
        let todo = todo_of(&rig.store, &plan, "Package the tarball")?;
        assert!(matches!(
            todo.state,
            TodoState::Blocked {
                on: BlockedOn::User,
                ..
            }
        ));
        Ok(())
    }

    // Dies with the `needs_resolution` guards in state.rs: the checker's refusal on surfaces 1
    // and 3, `apply_set`'s guard on surface 2, `apply_reconcile`'s on surface 5 (the `Op::Done`
    // arm's `completion` is reached only by a replayed record). Give any surface its own
    // completion path and that surface is how unverified work completes.
    #[tokio::test]
    async fn every_surface_requires_matching_verified_completion() -> TestResult {
        let rig = rig("yi-f0c-surfaces", None)?;
        let plan = planned()?;
        let contract = cmd_contract(&rig.store, &plan, "exit 1", "writer")?;
        let opened = init(&rig.engine, vec![contracted("ship it", contract)?])?;
        assert_eq!(opened.plan.id, plan);
        start(&rig.engine, &plan, "ship it")?;
        // 1. The tool: done runs the checker and is refused with a verdict.
        let verdict = refused(done(&rig.engine, &plan, "ship it", None))?;
        assert_eq!(verdict.outcome, VerdictOutcome::Fail);
        // 2. plan.op: a set asking for done is refused by the same validator.
        let mut registry = HostRegistry::default();
        yi_runtime::plan::request::register(Arc::clone(&rig.engine), Actor::Owner, &mut registry);
        let mut payload = Map::new();
        payload.insert(
            "request_id".to_owned(),
            Value::String("surface-1".to_owned()),
        );
        payload.insert("op".to_owned(), Value::String("set".to_owned()));
        payload.insert("plan".to_owned(), Value::String(plan.as_str().to_owned()));
        payload.insert("args".to_owned(), json!({"list": "- [x] ship it"}));
        let reply = registry
            .dispatch("plan.op", payload)
            .ok_or("plan.op is not registered")?
            .await?;
        assert_eq!(reply["ok"], Value::Bool(false));
        assert!(
            reply["refusal"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("no verified completion"),
            "{reply:?}"
        );
        // 3. The CLI surface shares `authority::submit` over the same engine.
        let submission = yi_runtime::plan::authority::Submission {
            args: json!({"op": "done", "label": "ship it", "plan": plan.as_str()})
                .as_object()
                .cloned()
                .ok_or("args")?,
            request_id: None,
            expected_revision: None,
        };
        let cli = yi_runtime::plan::authority::submit(&rig.engine, &Actor::Owner, None, submission);
        assert!(
            cli.is_err(),
            "the CLI cannot complete what the checker refused"
        );
        let accept = submit(
            &rig.engine,
            &Actor::Owner,
            None,
            accept_submission(&plan, "ship it")?,
        );
        assert!(
            matches!(
                accept,
                Err(SubmitError::NoConfirmer {
                    op: "accepted_by_user"
                })
            ),
            "the CLI's accept is the user's alone, and no prompt is here: {accept:?}"
        );
        // 4. Import: a clone's checkpoint claiming the todo done lands as legacy history.
        let mut claimed = rig.store.read(&plan)?;
        for todo in &mut claimed.todos {
            todo.state = TodoState::Done {
                output: None,
                resolution: Some(Resolution::VerifiedDone),
            };
        }
        claimed.id = PlanId::new("ship-it-clone")?;
        let clone_dir = rig.ws.join("clone/ship-it-clone");
        std::fs::create_dir_all(&clone_dir)?;
        std::fs::write(clone_dir.join("plan.json"), PlanStore::render(&claimed)?)?;
        let imported = rig.engine.apply(owner(Op::Import {
            source: format!("local://{}", clone_dir.join("plan.json").display()).parse()?,
        }))?;
        let legacy = imported
            .plan
            .todo(&label("ship it")?)
            .ok_or("imported todo")?;
        assert!(
            matches!(
                legacy.state,
                TodoState::Done {
                    resolution: Some(Resolution::LegacyUnverified),
                    ..
                }
            ),
            "an imported done is history, never evidence: {:?}",
            legacy.state
        );
        // 5. Repair and reconcile: a reused result cannot complete a contracted todo.
        let reused = rig.engine.apply(OpRequest {
            plan: Some(plan.clone()),
            actor: Actor::Host,
            op: Op::Reconcile {
                label: label("ship it")?,
                effect_id: None,
                outcome: yi_runtime::plan::ops::Reconciliation::Reused {
                    output: "local://out.json".parse()?,
                },
            },
            request_id: None,
            expected_revision: None,
        });
        assert!(
            matches!(reused, Err(PlanOpError::NoVerifiedCompletion { .. })),
            "{reused:?}"
        );
        // 6. A restored view: an edited checkpoint marking the todo done never overwrites the journal.
        let mut edited = rig.store.read(&plan)?;
        for todo in &mut edited.todos {
            todo.state = TodoState::Done {
                output: None,
                resolution: Some(Resolution::VerifiedDone),
            };
        }
        std::fs::write(rig.store.path(&plan), PlanStore::render(&edited)?)?;
        let view = rig.engine.apply(at(&plan, Op::View { full: true }))?;
        let shown = view.plan.todo(&label("ship it")?).ok_or("todo")?;
        assert!(
            matches!(shown.state, TodoState::Running { .. }),
            "the journal, not the edited file, is what a view restores: {:?}",
            shown.state
        );
        let after = rig.engine.apply(at(
            &plan,
            Op::Fail {
                label: label("ship it")?,
                cause: "x".to_owned(),
                disposition: None,
            },
        ))?;
        let failed = after.plan.todo(&label("ship it")?).ok_or("todo")?;
        assert!(matches!(failed.state, TodoState::Failed { .. }));
        assert!(
            !verdicts(&rig.store, &plan)?
                .iter()
                .any(|verdict| verdict.outcome == VerdictOutcome::Pass),
            "no surface produced a pass"
        );
        Ok(())
    }

    // Dies with `Contract::validate` in `Plan::validate`: check the floor only at done and a
    // plan sits in the store promising a deliverable nothing can decide.
    #[test]
    fn writer_requires_a_passing_critical_behavioral_check() -> TestResult {
        let rig = rig("yi-f0c-floor", None)?;
        let schema_only: Contract = serde_json::from_value(json!({
            "class": "writer",
            "items": [{"id": "shape", "critical": true, "weight": 100,
                       "decider": {"schema": {"schema": {"digest": yi_types::plan::canonical::Digest::of(b"{}").to_string(), "media_type": "application/schema+json", "length": 2}}}}],
            "threshold": 1000, "min_coverage": 1000
        }))?;
        let mut shaped = spec("write it")?;
        shaped.contract = Some(schema_only.clone());
        let at_init = init(&rig.engine, vec![shaped.clone()]);
        assert!(
            matches!(at_init, Err(PlanOpError::Invalid { .. })),
            "{at_init:?}"
        );
        let opened = init(&rig.engine, vec![spec("seed")?])?;
        let plan = opened.plan.id.clone();
        let at_append = rig.engine.apply(at(
            &plan,
            Op::Append {
                todos: vec![shaped.clone()],
            },
        ));
        assert!(
            matches!(at_append, Err(PlanOpError::Invalid { .. })),
            "{at_append:?}"
        );
        let at_supersede = rig.engine.apply(at(
            &plan,
            Op::Supersede {
                reason: "reshape".to_owned(),
                todos: vec![shaped.clone()],
            },
        ));
        assert!(
            matches!(at_supersede, Err(PlanOpError::Invalid { .. })),
            "{at_supersede:?}"
        );
        let at_set = rig.engine.apply(at(
            &plan,
            Op::Set {
                goal: None,
                rows: vec![SetRow {
                    spec: shaped,
                    state: TodoStateName::Pending,
                }],
            },
        ));
        assert!(
            matches!(at_set, Err(PlanOpError::Invalid { .. })),
            "{at_set:?}"
        );
        // A judge item never stands alone: a live decider (F3a), and still below every floor.
        let judged: Contract = serde_json::from_value(json!({
            "class": "writer",
            "items": [{"id": "taste", "critical": true, "weight": 100,
                       "decider": {"judge": {"rubric": {"digest": yi_types::plan::canonical::Digest::of(b"r").to_string(), "media_type": "text/markdown", "length": 1}, "evidence": [], "policy": {"n": 3}}}}],
            "threshold": 1000, "min_coverage": 1000
        }))?;
        let mut judged_spec = spec("judge it")?;
        judged_spec.contract = Some(judged);
        let at_judge = rig.engine.apply(at(
            &plan,
            Op::Append {
                todos: vec![judged_spec],
            },
        ));
        assert!(
            matches!(at_judge, Err(PlanOpError::Invalid { .. })),
            "{at_judge:?}"
        );
        // The control: a critical cmd item clears the writer floor.
        let behavioral = cmd_contract(&rig.store, &plan, "true", "writer")?;
        let mut ok = spec("prove it")?;
        ok.contract = Some(behavioral);
        rig.engine
            .apply(at(&plan, Op::Append { todos: vec![ok] }))?;
        Ok(())
    }

    // Dies with the `ok_or_else(UnservedOutput)` and `ok_or_else(UnservedSchema)` arms of
    // `check_output`: restore the two-Some guard and done passes on a product nobody read.
    #[test]
    fn missing_output_schema_or_resolver_never_passes() -> TestResult {
        let doc = fixture("unserved-output-today-passes")?;
        let rig = rig("yi-f0c-unserved", None)?;
        let (goal, specs) = init_specs(&doc)?;
        let opened = rig.engine.apply(owner(Op::Init { goal, todos: specs }))?;
        let plan = opened.plan.id.clone();
        let schema = blob(&doc, "report-schema")?;
        rig.serve.set("local://schema/report.json", Some(&schema));
        rig.serve.set("kernel://main/build_report", None);
        let product = done(
            &rig.engine,
            &plan,
            "Build the tarball",
            Some("kernel://main/build_report"),
        );
        assert!(
            matches!(product, Err(PlanOpError::UnservedOutput { .. })),
            "{product:?}"
        );
        assert!(
            product
                .as_ref()
                .err()
                .map(ToString::to_string)
                .unwrap_or_default()
                .contains("unserved product")
        );
        let todo = todo_of(&rig.store, &plan, "Build the tarball")?;
        assert!(matches!(todo.state, TodoState::Running { .. }));
        assert_eq!(todo.refusals, 0, "an unserved product charges nothing");
        rig.serve.set("local://build/docs.json", Some(&schema));
        rig.serve.set("kernel://main/report_schema", None);
        let criterion = done(
            &rig.engine,
            &plan,
            "Build the docs",
            Some("local://build/docs.json"),
        );
        assert!(
            matches!(criterion, Err(PlanOpError::UnservedSchema { .. })),
            "{criterion:?}"
        );
        assert!(
            criterion
                .as_ref()
                .err()
                .map(ToString::to_string)
                .unwrap_or_default()
                .contains("unserved schema")
        );
        assert_eq!(todo_of(&rig.store, &plan, "Build the docs")?.refusals, 0);
        Ok(())
    }

    // Dies with the whole-token comparison in `settle_verdict`: compare only the label and a
    // stale verdict completes a todo whose product no longer exists.
    #[test]
    fn old_attempt_verdict_cannot_complete_restarted_task() -> TestResult {
        let doc = fixture("stale-token")?;
        let temp = Scratch::new("yi-f0c-stale")?;
        let store = PlanStore::open(temp.join("plans"))?;
        let ws = temp.join("ws");
        std::fs::create_dir_all(ws.join("dist"))?;
        std::fs::write(
            ws.join("dist/logrotate-lite.tar.gz"),
            blob(&doc, "tarball")?,
        )?;
        let (goal, specs) = init_specs(&doc)?;
        let plan = PlanId::new(doc["plan"].as_str().ok_or("plan")?)?;
        // A second engine over the same store moves the todo while the first verifies it.
        let other = Arc::new(
            PlanEngine::new(store.clone(), Arc::new(Stub::default())).with_cwd(ws.clone()),
        );
        let mover = Arc::clone(&other);
        let moved_plan = plan.clone();
        let hook: yi_runtime::plan::ops::VerifyHook = Arc::new(move || {
            let label = TodoLabel::new("Package the tarball").ok();
            if let Some(label) = label {
                let _failed = mover.apply(at(
                    &moved_plan,
                    Op::Fail {
                        label: label.clone(),
                        cause: "the build host lost the disk".to_owned(),
                        disposition: None,
                    },
                ));
                let _retried = mover.apply(at(
                    &moved_plan,
                    Op::Retry {
                        label: label.clone(),
                        delegation: None,
                    },
                ));
                // Running again on attempt 2: the attempt is the only token field that moved.
                let _started = mover.apply(at(&moved_plan, Op::Start { label }));
            }
        });
        let engine = PlanEngine::new(store.clone(), Arc::new(Stub::default()))
            .with_cwd(ws.clone())
            .with_verifier(Verifier::new(20_000))
            .with_verify_hook(hook);
        let opened = engine.apply(owner(Op::Init { goal, todos: specs }))?;
        assert_eq!(opened.plan.id, plan);
        stage(&store, &plan, &doc)?;
        start(&engine, &plan, "Package the tarball")?;
        let stale = done(&engine, &plan, "Package the tarball", None);
        match stale {
            Err(PlanOpError::Stale { token, .. }) => assert_eq!(token.attempt.get(), 1),
            other => return Err(format!("expected stale, got {other:?}").into()),
        }
        let tail: Vec<String> = kinds(&store, &plan)?.into_iter().rev().take(5).collect();
        assert_eq!(
            tail,
            [
                "verification_stale",
                "start",
                "retry",
                "fail",
                "verification_requested"
            ]
        );
        let journal = Journal::open(store.journal_path(&plan), Arc::new(RealFs));
        let stale_record = journal.read()?.records.pop().ok_or("stale record")?;
        let detail = stale_record.record.extra["refusal"]["detail"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        assert_eq!(
            detail, "the token names attempt 1; the todo is on attempt 2",
            "the stale detail names the attempt move and nothing else"
        );
        let todo = todo_of(&store, &plan, "Package the tarball")?;
        assert!(
            matches!(todo.state, TodoState::Running { .. }),
            "{:?}",
            todo.state
        );
        assert_eq!(
            (todo.attempt.get(), todo.refusals, todo.retries),
            (2, 0, RetryCount(1))
        );
        // The next attempt verifies and completes normally: the stale path is not a dead end.
        let quiet = PlanEngine::new(store.clone(), Arc::new(Stub::default()))
            .with_cwd(ws)
            .with_verifier(Verifier::new(20_000));
        let passed = done(&quiet, &plan, "Package the tarball", None)?;
        let todo = passed
            .plan
            .todo(&label("Package the tarball")?)
            .ok_or("todo")?;
        assert!(matches!(
            todo.state,
            TodoState::Done {
                resolution: Some(Resolution::VerifiedDone),
                ..
            }
        ));
        assert_eq!(todo.attempt.get(), 2);
        let last = verdicts(&store, &plan)?.pop().ok_or("a verdict")?;
        assert_eq!(
            (last.outcome, last.token.attempt.get()),
            (VerdictOutcome::Pass, 2)
        );
        Ok(())
    }

    // Dies with the contract digest in the token and its comparison against the frozen hash:
    // freeze by reference and a product that edits its own checker passes.
    #[test]
    fn changed_criterion_or_output_invalidates_verdict() -> TestResult {
        let serve_swap: Arc<Mutex<Option<Arc<Serve>>>> = Arc::new(Mutex::new(None));
        let swap = Arc::clone(&serve_swap);
        let hook: yi_runtime::plan::ops::VerifyHook = Arc::new(move || {
            if let Ok(slot) = swap.lock()
                && let Some(serve) = slot.as_ref()
            {
                serve.set("local://out.txt", Some("changed while verifying"));
            }
        });
        let rig = rig("yi-f0c-drift", Some(hook))?;
        let plan = planned()?;
        let opened = init(
            &rig.engine,
            vec![
                contracted(
                    "write it",
                    cmd_contract(&rig.store, &plan, "true", "writer")?,
                )?,
                contracted(
                    "read it",
                    cmd_contract(&rig.store, &plan, "true", "writer")?,
                )?,
            ],
        )?;
        assert_eq!(opened.plan.id, plan);
        start(&rig.engine, &plan, "write it")?;
        // The criterion changes after the freeze: a set rewrites the running todo's contract.
        let mut rewritten = spec("write it")?;
        rewritten.contract = Some(cmd_contract(&rig.store, &plan, "true # edited", "writer")?);
        rig.engine.apply(at(
            &plan,
            Op::Set {
                goal: None,
                rows: vec![
                    SetRow {
                        spec: rewritten,
                        state: TodoStateName::Running,
                    },
                    SetRow {
                        spec: spec("read it")?,
                        state: TodoStateName::Pending,
                    },
                ],
            },
        ))?;
        let drift = done(&rig.engine, &plan, "write it", None);
        assert!(
            matches!(drift, Err(PlanOpError::ContractDrift { .. })),
            "{drift:?}"
        );
        assert_eq!(
            todo_of(&rig.store, &plan, "write it")?.refusals,
            1,
            "drift is a recorded refusal"
        );
        // The output changes between the freeze and the comparison: stale, and nothing charged.
        rig.serve.set("local://out.txt", Some("as submitted"));
        if let Ok(mut slot) = serve_swap.lock() {
            *slot = Some(Arc::clone(&rig.serve));
        }
        start(&rig.engine, &plan, "read it")?;
        let stale = done(&rig.engine, &plan, "read it", Some("local://out.txt"));
        assert!(matches!(stale, Err(PlanOpError::Stale { .. })), "{stale:?}");
        assert_eq!(todo_of(&rig.store, &plan, "read it")?.refusals, 0);
        assert_eq!(
            kinds(&rig.store, &plan)?.last().map(String::as_str),
            Some("verification_stale")
        );
        Ok(())
    }

    // Dies with the `verification_requested` effect and the in-process flight keyed on the
    // token: key it on the request id and two calls run the checker twice and charge twice.
    #[test]
    fn concurrent_done_requests_share_verification_effect() -> TestResult {
        let rig = rig("yi-f0c-shared", None)?;
        let marker = rig._temp.join("runs.txt");
        let command = format!("sleep 1; echo run >> {}; exit 1", marker.display());
        let plan = planned()?;
        let opened = init(
            &rig.engine,
            vec![contracted(
                "race it",
                cmd_contract(&rig.store, &plan, &command, "writer")?,
            )?],
        )?;
        assert_eq!(opened.plan.id, plan);
        start(&rig.engine, &plan, "race it")?;
        let results: Vec<Result<Outcome, PlanOpError>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..2)
                .map(|_| {
                    let engine = Arc::clone(&rig.engine);
                    let plan = plan.clone();
                    scope.spawn(move || done(&engine, &plan, "race it", None))
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap_or(Err(PlanOpError::NoPlan)))
                .collect()
        });
        for result in results {
            let verdict = refused(result)?;
            assert_eq!(verdict.outcome, VerdictOutcome::Fail);
        }
        assert_eq!(
            std::fs::read_to_string(&marker)?.lines().count(),
            1,
            "the checker ran once"
        );
        assert_eq!(
            todo_of(&rig.store, &plan, "race it")?.refusals,
            1,
            "one refusal for one product"
        );
        let requested = kinds(&rig.store, &plan)?
            .iter()
            .filter(|kind| *kind == "verification_requested")
            .count();
        assert_eq!(requested, 1);
        // A third arriving after the verdict committed replays it.
        let replayed = refused(done(&rig.engine, &plan, "race it", None))?;
        assert_eq!(replayed.outcome, VerdictOutcome::Fail);
        assert_eq!(std::fs::read_to_string(&marker)?.lines().count(), 1);
        assert_eq!(todo_of(&rig.store, &plan, "race it")?.refusals, 1);
        Ok(())
    }

    // Dies with `refuse_live_claim` in done.rs: drop the claim check and the second engine
    // adopts the effect, the marker reads two runs and the todo is charged twice.
    #[test]
    fn another_engine_refuses_a_live_claim_and_charges_nothing() -> TestResult {
        let temp = Scratch::new("yi-f0c-claim")?;
        let store = PlanStore::open(temp.join("plans"))?;
        let ws = temp.join("ws");
        std::fs::create_dir_all(&ws)?;
        let marker = temp.join("runs.txt");
        let command = format!("echo run >> {}; exit 1", marker.display());
        let plan = planned()?;
        let other = Arc::new(
            PlanEngine::new(store.clone(), Arc::new(Stub::default()))
                .with_cwd(ws.clone())
                .with_verifier(Verifier::new(20_000)),
        );
        let seen: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let hook: yi_runtime::plan::ops::VerifyHook = {
            let other = Arc::clone(&other);
            let plan = plan.clone();
            let seen = Arc::clone(&seen);
            Arc::new(move || {
                let refusal = done(&other, &plan, "race it", None)
                    .err()
                    .map(|error| error.to_string());
                if let Ok(mut slot) = seen.lock() {
                    *slot = refusal;
                }
            })
        };
        let engine = PlanEngine::new(store.clone(), Arc::new(Stub::default()))
            .with_cwd(ws)
            .with_verifier(Verifier::new(20_000))
            .with_verify_hook(hook);
        init(
            &engine,
            vec![contracted(
                "race it",
                cmd_contract(&store, &plan, &command, "writer")?,
            )?],
        )?;
        start(&engine, &plan, "race it")?;
        let verdict = refused(done(&engine, &plan, "race it", None))?;
        assert_eq!(verdict.outcome, VerdictOutcome::Fail);
        let refusal = seen
            .lock()
            .map_err(|_| "poisoned")?
            .clone()
            .ok_or("the second engine's done was not refused")?;
        assert!(
            refusal.contains("in progress since")
                && refusal.contains(&format!("pid {}", std::process::id())),
            "{refusal}"
        );
        assert_eq!(std::fs::read_to_string(&marker)?.lines().count(), 1);
        assert_eq!(todo_of(&store, &plan, "race it")?.refusals, 1);
        let all = kinds(&store, &plan)?;
        assert_eq!(
            all.iter()
                .filter(|kind| *kind == "verification_requested")
                .count(),
            1
        );
        assert_eq!(all.iter().filter(|kind| *kind == "done_refused").count(), 1);
        Ok(())
    }

    // Dies with the criteria being frozen at start and the refusal leaving the todo where it
    // was: let the contract be rewritten between attempts and the pair goes green proving nothing.
    #[test]
    fn product_repair_passes_under_unchanged_criteria() -> TestResult {
        let doc = fixture("writer-cmd-red-then-green")?;
        let rig = rig("yi-f0c-red-green", None)?;
        let (goal, specs) = init_specs(&doc)?;
        let opened = rig.engine.apply(owner(Op::Init { goal, todos: specs }))?;
        let plan = opened.plan.id.clone();
        stage(&rig.store, &plan, &doc)?;
        let todo = todo_of(&rig.store, &plan, "Generate the report")?;
        assert_eq!((todo.attempt.get(), todo.refusals), (1, 0));
        start(&rig.engine, &plan, "Generate the report")?;
        assert!(
            todo_of(&rig.store, &plan, "Generate the report")?
                .contract_hash
                .is_some(),
            "start froze the contract"
        );
        let red = blob(&doc, "product-red")?;
        std::fs::write(rig.ws.join("report.json"), &red)?;
        rig.serve.set("local://build/report.json", Some(&red));
        let first = refused(done(
            &rig.engine,
            &plan,
            "Generate the report",
            Some("local://build/report.json"),
        ))?;
        assert_eq!(
            (first.outcome, first.score, first.coverage),
            (VerdictOutcome::Fail, 0, 1000)
        );
        assert_eq!(first.items[0].id.as_str(), "suite-green");
        assert!(matches!(
            first.items[0].verdict,
            yi_types::plan::contract::ItemVerdict::Fail { .. }
        ));
        assert_eq!(first.token.attempt.get(), 1);
        let tail: Vec<String> = kinds(&rig.store, &plan)?
            .into_iter()
            .rev()
            .take(2)
            .collect();
        assert_eq!(tail, ["done_refused", "verification_requested"]);
        let todo = todo_of(&rig.store, &plan, "Generate the report")?;
        assert!(matches!(todo.state, TodoState::Running { .. }));
        assert_eq!((todo.attempt.get(), todo.refusals), (1, 1));
        let green = blob(&doc, "product-green")?;
        std::fs::write(rig.ws.join("report.json"), &green)?;
        rig.serve.set("local://build/report.json", Some(&green));
        let passed = done(
            &rig.engine,
            &plan,
            "Generate the report",
            Some("local://build/report.json"),
        )?;
        let todo = passed
            .plan
            .todo(&label("Generate the report")?)
            .ok_or("todo")?;
        assert!(
            matches!(
                &todo.state,
                TodoState::Done { output: Some(url), resolution: Some(Resolution::VerifiedDone) }
                    if url.to_string() == "local://build/report.json"
            ),
            "{:?}",
            todo.state
        );
        assert_eq!((todo.attempt.get(), todo.refusals), (1, 1));
        let tail: Vec<String> = kinds(&rig.store, &plan)?
            .into_iter()
            .rev()
            .take(2)
            .collect();
        assert_eq!(tail, ["done", "verification_requested"]);
        let all = verdicts(&rig.store, &plan)?;
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].outcome, VerdictOutcome::Pass);
        assert_eq!(
            all[0].token.contract_digest, all[1].token.contract_digest,
            "the contract never moved"
        );
        assert_eq!(all[0].token.criteria_digest, all[1].token.criteria_digest);
        assert_ne!(
            all[0].token.output_digest, all[1].token.output_digest,
            "the product did"
        );
        Ok(())
    }

    // Dies with the verifier reading `snapshot.output`, the bytes the token's output digest
    // names: let an inline todo nominate any artifact and a sidecar that fits the schema passes.
    #[test]
    fn inline_task_output_validates_product_not_sidecar() -> TestResult {
        let doc = fixture("reader-schema")?;
        let rig = rig("yi-f0c-inline", None)?;
        let schema_ref: ArtifactRef = serde_json::from_value(doc["steps"][0]["args"]["todos"][0]["contract"]["items"][0]["decider"]["schema"]["schema"].clone())?;
        let inline: Contract = serde_json::from_value(json!({
            "class": "inline",
            "items": [{"id": "findings-shape", "critical": true, "weight": 100, "decider": {"schema": {"schema": schema_ref}}}],
            "threshold": 1000, "min_coverage": 1000
        }))?;
        let mut todo = spec("summarize the docs")?;
        todo.contract = Some(inline);
        let opened = init(&rig.engine, vec![todo])?;
        let plan = opened.plan.id.clone();
        stage(&rig.store, &plan, &doc)?;
        rig.serve.set(
            "local://findings/product.json",
            Some(&blob(&doc, "findings-bare")?),
        );
        rig.serve.set(
            "local://findings/sidecar.json",
            Some(&blob(&doc, "findings-good")?),
        );
        start(&rig.engine, &plan, "summarize the docs")?;
        let bare = refused(done(
            &rig.engine,
            &plan,
            "summarize the docs",
            Some("local://findings/product.json"),
        ))?;
        assert_eq!(bare.outcome, VerdictOutcome::Fail);
        let detail = bare.items[0].verdict.to_string();
        assert!(
            detail.contains("$.findings[0]") && detail.contains("provenance"),
            "the refusal names the path the schema rejected: {detail}"
        );
        let none = done(&rig.engine, &plan, "summarize the docs", None);
        assert!(
            matches!(none, Err(PlanOpError::OutputRequired { .. })),
            "{none:?}"
        );
        assert_eq!(
            todo_of(&rig.store, &plan, "summarize the docs")?.refusals,
            2
        );
        Ok(())
    }

    // Dies with the `capped` block in `refuse`: make the cap a refusal text and a model retries
    // forever; count stale verdicts and a slow checker walks a healthy todo into the inbox.
    #[test]
    fn the_third_refusal_blocks_the_todo_on_user_as_a_recorded_transition() -> TestResult {
        let rig = rig("yi-f0c-cap", None)?;
        let plan = planned()?;
        let opened = init(
            &rig.engine,
            vec![contracted(
                "land it",
                cmd_contract(&rig.store, &plan, "exit 1", "writer")?,
            )?],
        )?;
        assert_eq!(opened.plan.id, plan);
        start(&rig.engine, &plan, "land it")?;
        for attempt in 1..=3 {
            rig.serve
                .set("local://out.txt", Some(&format!("product {attempt}")));
            let verdict = refused(done(&rig.engine, &plan, "land it", Some("local://out.txt")))?;
            assert_eq!(verdict.outcome, VerdictOutcome::Fail);
        }
        let todo = todo_of(&rig.store, &plan, "land it")?;
        assert_eq!(todo.refusals, 3);
        assert!(
            matches!(&todo.state, TodoState::Blocked { on: BlockedOn::User, note } if note.contains("3 refused verdicts")),
            "{:?}",
            todo.state
        );
        let all = kinds(&rig.store, &plan)?;
        assert_eq!(all.iter().filter(|kind| *kind == "done_refused").count(), 3);
        let tail: Vec<String> = all.into_iter().rev().take(2).collect();
        assert_eq!(
            tail,
            ["block", "done_refused"],
            "the cap is its own committed transition"
        );
        let journal = Journal::open(rig.store.journal_path(&plan), Arc::new(RealFs));
        let block = journal.read()?.records.pop().ok_or("block record")?;
        assert_eq!(
            (block.record.from.clone(), block.record.to.clone()),
            (Some(TodoStateName::Running), Some(TodoStateName::Blocked))
        );
        Ok(())
    }

    /// A jury that never settles: it counts its sittings and abstains with one juror's line,
    /// whose reason claims the phrase the quote check writes while its flag says otherwise.
    struct HungJury(AtomicU32);

    impl yi_runtime::plan::verify::Judge for HungJury {
        fn judge(
            &self,
            _item: &ContractItem,
            snapshot: &yi_runtime::plan::verify::Snapshot<'_>,
            _until: std::time::Instant,
        ) -> (ItemVerdict, Vec<JurorLine>) {
            self.0.fetch_add(1, Ordering::SeqCst);
            let seated = snapshot.jury.as_ref().map(|seat| seat.permit.purpose());
            let line = JurorLine {
                model: "openrouter/z-ai/glm-5.3-flash".to_owned(),
                vote: Vote::Abstain,
                reason: format!("unbacked quote: seated under {seated:?}"),
                unbacked: false,
            };
            let reason = "no quorum".to_owned();
            (ItemVerdict::Abstain { reason }, vec![line])
        }
    }

    // Dies with the `JUDGE_CAP_PER_TODO` arm in `Verifier::run`, with `juries` counting the
    // journal's requests, with the escalation arm of `refuse`'s cap and its own note, and with
    // `juror_votes` reading the check's flag rather than a reason a juror writes.
    #[test]
    fn the_fourth_jury_on_one_todo_escalates_to_the_user() -> TestResult {
        let jury = Arc::new(HungJury(AtomicU32::new(0)));
        let verifier = Verifier::new(20_000).with_judge(jury.clone());
        let rig = rig_verified("yi-f3a-jury-cap", None, verifier)?;
        let plan = planned()?;
        let artifacts = rig.store.artifacts(&plan);
        let rubric = artifacts.put(b"it reads well", "text/markdown", &rig.store.nonce())?;
        let essay = artifacts.put(b"an essay", "text/markdown", &rig.store.nonce())?;
        let mut contract =
            serde_json::to_value(cmd_contract(&rig.store, &plan, "true", "writer")?)?;
        contract["items"]
            .as_array_mut()
            .ok_or("items")?
            .push(json!({
                "id": "taste", "critical": false, "weight": 1,
                "decider": {"judge": {"rubric": rubric, "evidence": [essay], "policy": {"n": 3}}}
            }));
        init(
            &rig.engine,
            vec![contracted("land it", serde_json::from_value(contract)?)?],
        )?;
        start(&rig.engine, &plan, "land it")?;
        for sitting in 1..=3 {
            let verdict = refused(done(&rig.engine, &plan, "land it", None))?;
            assert_eq!(
                verdict.outcome,
                VerdictOutcome::Abstain,
                "sitting {sitting}"
            );
            let taste = verdict.items.get(1).ok_or("the judged line")?;
            assert_eq!(taste.jurors.len(), 1, "the juror's line rides the verdict");
            assert!(taste.jurors[0].reason.contains("Verification"), "{taste:?}");
        }
        let journal = Journal::open(rig.store.journal_path(&plan), Arc::new(RealFs));
        let sat = journal.read()?.records.pop().ok_or("the third refusal")?;
        assert_eq!(
            sat.record.extra.get("jurors"),
            Some(&json!({"pass": 0, "fail": 0, "abstain": 1, "unbacked": 0})),
            "the votes are counted off the check's flag, never a juror's reason"
        );
        let todo = todo_of(&rig.store, &plan, "land it")?;
        assert!(
            matches!(todo.state, TodoState::Running { .. }),
            "{:?}",
            todo.state
        );
        assert_eq!(rig.engine.capacity().held(Purpose::Verification), 0);

        let verdict = refused(done(&rig.engine, &plan, "land it", None))?;
        assert_eq!(verdict.outcome, VerdictOutcome::Escalate);
        assert_eq!(jury.0.load(Ordering::SeqCst), 3, "no fourth jury sits");
        let todo = todo_of(&rig.store, &plan, "land it")?;
        assert!(
            matches!(&todo.state, TodoState::Blocked { on: BlockedOn::User, note }
                if note.contains("juries") && !note.contains("refused verdicts")),
            "{:?}",
            todo.state
        );
        let tail: Vec<String> = kinds(&rig.store, &plan)?
            .into_iter()
            .rev()
            .take(2)
            .collect();
        assert_eq!(tail, ["block", "done_refused"]);
        Ok(())
    }

    // Dies with the `attempt` filter in `refused_verdicts`: count the todo's lifetime refusals
    // and every retrying scheduler walks a healthy todo into the human inbox instead of RETRY_CAP.
    #[test]
    fn a_retry_opens_a_fresh_refusal_count_so_only_retry_cap_bounds_a_scheduler() -> TestResult {
        let rig = rig("yi-f0c-cap-attempt", None)?;
        let plan = planned()?;
        init(
            &rig.engine,
            vec![contracted(
                "land it",
                cmd_contract(&rig.store, &plan, "exit 1", "writer")?,
            )?],
        )?;
        for attempt in 1..=2 {
            start(&rig.engine, &plan, "land it")?;
            for call in 1..=2 {
                // A fresh product each time, or the second `done` replays the settled verdict free.
                rig.serve.set(
                    "local://out.txt",
                    Some(&format!("product {attempt}.{call}")),
                );
                let verdict =
                    refused(done(&rig.engine, &plan, "land it", Some("local://out.txt")))?;
                assert_eq!(verdict.outcome, VerdictOutcome::Fail);
            }
            let todo = todo_of(&rig.store, &plan, "land it")?;
            assert!(
                matches!(todo.state, TodoState::Running { .. }),
                "attempt {attempt} of two refusals must not block: {:?}",
                todo.state
            );
            rig.engine.apply(at(
                &plan,
                Op::Fail {
                    label: TodoLabel::new("land it")?,
                    cause: "the shape settles a refused done".to_owned(),
                    disposition: None,
                },
            ))?;
            rig.engine.apply(at(
                &plan,
                Op::Retry {
                    label: TodoLabel::new("land it")?,
                    delegation: None,
                },
            ))?;
        }
        let todo = todo_of(&rig.store, &plan, "land it")?;
        assert_eq!(
            todo.refusals, 4,
            "the lifetime counter is an event, not the cap"
        );
        assert_eq!(todo.retries, RetryCount(2));
        assert!(
            !kinds(&rig.store, &plan)?.iter().any(|kind| kind == "block"),
            "four refusals across two attempts must never reach the section 6.3 cap"
        );
        Ok(())
    }

    // Dies with `mark_legacy` in import.rs: drop it and one import launders a file's claims into
    // verified work; read it as VerifiedDone and the fuzz lane's pass-verdict check dies too.
    fn accept_submission(plan: &PlanId, label: &str) -> Result<Submission, Box<dyn Error>> {
        Ok(Submission {
            args: json!({"op": "accept", "plan": plan.as_str(), "label": label, "note": "waved through after review"})
                .as_object()
                .cloned()
                .ok_or("args")?,
            request_id: None,
            expected_revision: None,
        })
    }

    /// The process that owns the prompt, answering `answer` to every ask.
    fn confirmer(rig: &Rig, answer: AskOutcome) -> Result<Confirmer, Box<dyn Error>> {
        let sessions = rig._temp.join("sessions");
        std::fs::create_dir_all(&sessions)?;
        let mut repo = JsonlRepo::new(sessions, rig.ws.to_string_lossy().into_owned());
        let store = repo.create(CreateOptions::default())?;
        let asker: Asker = Arc::new(move |_ask: &PermissionAsk<'_>| answer);
        let (events, _nobody_listens) = tokio::sync::broadcast::channel(8);
        let broker = Arc::new(PermissionBroker::new(
            PermissionMode::Ask,
            rig.ws.clone(),
            Vec::new(),
            Some(asker),
            events,
        ));
        Ok(Confirmer { broker, store })
    }

    // Dies with the `Accept` arm of `check_actor` and with `apply_op`'s hard-coded
    // `AcceptedByUser`: let the owner accept, or write `VerifiedDone` here, and a refused
    // checker is waved through as verified work.
    #[test]
    fn accept_records_accepted_by_user_never_verified_done() -> TestResult {
        let rig = rig("yi-f0c-accept", None)?;
        let plan = planned()?;
        let contract = cmd_contract(&rig.store, &plan, "exit 1", "writer")?;
        init(&rig.engine, vec![contracted("ship it", contract)?])?;
        start(&rig.engine, &plan, "ship it")?;
        let verdict = refused(done(&rig.engine, &plan, "ship it", None))?;
        assert_eq!(verdict.outcome, VerdictOutcome::Fail);
        let accept = Op::Accept {
            label: label("ship it")?,
            note: "waved through after review".to_owned(),
            output: None,
        };
        let owner_refused = rig.engine.apply(at(&plan, accept.clone()));
        assert!(
            matches!(owner_refused, Err(PlanOpError::NotOwner { .. })),
            "{owner_refused:?}"
        );
        let declined = submit(
            &rig.engine,
            &Actor::Owner,
            Some(&confirmer(&rig, AskOutcome::Reject)?),
            accept_submission(&plan, "ship it")?,
        );
        assert!(
            matches!(declined, Err(SubmitError::Declined { .. })),
            "{declined:?}"
        );
        assert!(matches!(
            todo_of(&rig.store, &plan, "ship it")?.state,
            TodoState::Running { .. }
        ));
        let applied = submit(
            &rig.engine,
            &Actor::Owner,
            Some(&confirmer(&rig, AskOutcome::AllowOnce)?),
            accept_submission(&plan, "ship it")?,
        )?;
        let accepted = applied
            .outcome
            .plan
            .todo(&label("ship it")?)
            .ok_or("todo")?;
        assert_eq!(
            accepted.state,
            TodoState::Done {
                output: None,
                resolution: Some(Resolution::AcceptedByUser),
            }
        );
        assert!(
            applied.text().contains("(accepted_by_user)"),
            "the view names the resolution: {}",
            applied.text()
        );
        let records = rig.store.journal(&plan).read()?.records;
        let record = records.last().ok_or("no record")?;
        assert_eq!(record.record.op, "accepted_by_user");
        assert_eq!(record.record.actor, "user://1", "the citation is the actor");
        assert_eq!(
            record.record.extra.get("resolution"),
            Some(&json!("accepted_by_user"))
        );
        assert_eq!(record.args["note"], json!("waved through after review"));
        assert!(
            !verdicts(&rig.store, &plan)?
                .iter()
                .any(|verdict| verdict.outcome == VerdictOutcome::Pass),
            "acceptance is not a pass"
        );
        assert_eq!(todo_of(&rig.store, &plan, "ship it")?.refusals, 1);
        Ok(())
    }

    // Dies with `needs_resolution` in state.rs: count only contracted todos and "it works"
    // completes a delegated todo on the owner's word again.
    #[test]
    fn a_stated_only_todo_needs_an_item_or_a_user() -> TestResult {
        let rig = rig("yi-f0c-stated", None)?;
        let plan = planned()?;
        let mut stated = delegated_spec("write the manpage")?;
        if let Some(delegation) = &mut stated.delegation {
            delegation.accept = Check::Stated("the manpage reads well".to_owned());
        }
        let mut later = delegated_spec("write the changelog")?;
        if let Some(delegation) = &mut later.delegation {
            delegation.accept = Check::Stated("the changelog is complete".to_owned());
        }
        // Held behind an inline todo, so the engine does not start it before its item exists.
        later.after = vec![label("sign off")?];
        init(&rig.engine, vec![stated, later.clone(), spec("sign off")?])?;
        let refused = done(&rig.engine, &plan, "write the manpage", None);
        assert!(
            matches!(refused, Err(PlanOpError::NoVerifiedCompletion { .. })),
            "a stated acceptance decides nothing: {refused:?}"
        );
        assert!(matches!(
            todo_of(&rig.store, &plan, "write the manpage")?.state,
            TodoState::Running { .. }
        ));
        // The user's road: accept.
        let accepted = rig.engine.apply(OpRequest {
            plan: Some(plan.clone()),
            actor: Actor::User("user://4".parse::<Url>()?),
            op: Op::Accept {
                label: label("write the manpage")?,
                note: "read it, it is fine".to_owned(),
                output: None,
            },
            request_id: None,
            expected_revision: None,
        })?;
        assert!(matches!(
            accepted
                .plan
                .todo(&label("write the manpage")?)
                .map(|todo| &todo.state),
            Some(TodoState::Done {
                resolution: Some(Resolution::AcceptedByUser),
                ..
            })
        ));
        // The owner's road: a decidable item, then a verified done.
        later.contract = Some(cmd_contract(&rig.store, &plan, "true", "writer")?);
        rig.engine.apply(at(
            &plan,
            Op::Set {
                goal: None,
                rows: vec![
                    SetRow {
                        spec: spec("write the manpage")?,
                        state: TodoStateName::Done,
                    },
                    SetRow {
                        spec: later,
                        state: TodoStateName::Pending,
                    },
                    SetRow {
                        spec: spec("sign off")?,
                        state: TodoStateName::Pending,
                    },
                ],
            },
        ))?;
        start(&rig.engine, &plan, "sign off")?;
        let signed = done(&rig.engine, &plan, "sign off", None)?;
        assert_eq!(
            signed.spawned.len(),
            1,
            "the cleared edge starts the changelog"
        );
        let verified = done(&rig.engine, &plan, "write the changelog", None)?;
        assert!(matches!(
            verified
                .plan
                .todo(&label("write the changelog")?)
                .map(|todo| &todo.state),
            Some(TodoState::Done {
                resolution: Some(Resolution::VerifiedDone),
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn import_marks_legacy_success_unverified() -> TestResult {
        let doc = fixture("legacy-stated-unverified")?;
        let rig = rig("yi-f0c-legacy", None)?;
        let id = doc["plan"].as_str().ok_or("plan")?;
        std::fs::create_dir_all(rig.store.dir())?;
        let legacy = rig.store.dir().join(format!("{id}.md"));
        std::fs::write(&legacy, blob(&doc, "legacy-md")?)?;
        let imported = rig.engine.apply(owner(Op::Import {
            source: format!("local://{}", legacy.display()).parse()?,
        }))?;
        let plan = imported.plan.id.clone();
        let done_todo = imported
            .plan
            .todo(&label("Write the Makefile dist target")?)
            .ok_or("todo")?;
        assert!(
            matches!(
                done_todo.state,
                TodoState::Done {
                    resolution: Some(Resolution::LegacyUnverified),
                    ..
                }
            ),
            "{:?}",
            done_todo.state
        );
        assert!(
            done_todo
                .delegation
                .as_ref()
                .is_some_and(|d| matches!(d.accept, Check::Stated(_))),
            "the stated check stays as compatibility input"
        );
        assert_eq!(kinds(&rig.store, &plan)?, ["import"]);
        let view = rig.engine.apply(at(&plan, Op::View { full: true }))?;
        let shown = view
            .plan
            .todo(&label("Write the Makefile dist target")?)
            .ok_or("todo")?;
        assert!(matches!(
            shown.state,
            TodoState::Done {
                resolution: Some(Resolution::LegacyUnverified),
                ..
            }
        ));
        assert!(PlanStore::render(&view.plan)?.contains("\"resolution\": \"legacy_unverified\""));
        // A contracted todo added later still needs its own verdict; the legacy row buys nothing.
        let mut next = spec("Drive the whole build through make dist")?;
        next.contract = Some(cmd_contract(&rig.store, &plan, "exit 1", "writer")?);
        let refused = rig.engine.apply(at(
            &plan,
            Op::Set {
                goal: None,
                rows: vec![
                    SetRow {
                        spec: spec("Write the Makefile dist target")?,
                        state: TodoStateName::Done,
                    },
                    SetRow {
                        spec: next,
                        state: TodoStateName::Done,
                    },
                ],
            },
        ));
        assert!(
            matches!(refused, Err(PlanOpError::NoVerifiedCompletion { .. })),
            "{refused:?}"
        );
        let _isolation = Isolation::None;
        Ok(())
    }

    // Dies with `check_contracted` (table.rs): `retry` swaps a delegation onto a todo the
    // parse never saw beside its contract, so a worktree without one lands and never completes.
    #[test]
    fn a_retry_swapping_in_an_uncontracted_worktree_delegation_is_refused() -> TestResult {
        let rig = rig("yi-f0c-worktree-retry", None)?;
        let plan = planned()?;
        let mut apart = delegated_spec("build it apart")?;
        if let Some(delegation) = &mut apart.delegation {
            delegation.spec.isolation = Some(Isolation::Worktree);
        }
        let mut contracted = spec("build it later")?;
        contracted.contract = Some(cmd_contract(&rig.store, &plan, "true", "writer")?);
        init(&rig.engine, vec![spec("hold the plan open")?, contracted])?;
        let fail_then_retry = |label_text: &str| -> Result<(), Box<dyn Error>> {
            start(&rig.engine, &plan, label_text)?;
            rig.engine.apply(at(
                &plan,
                Op::Fail {
                    label: label(label_text)?,
                    cause: "needs its own tree".to_owned(),
                    disposition: None,
                },
            ))?;
            rig.engine.apply(at(
                &plan,
                Op::Retry {
                    label: label(label_text)?,
                    delegation: apart.delegation.clone().map(Box::new),
                },
            ))?;
            Ok(())
        };
        // With a contract on the todo the swap lands, which is the road the refusal names.
        fail_then_retry("build it later")?;
        let swapped = todo_of(&rig.store, &plan, "build it later")?;
        assert!(
            swapped.delegation.is_some_and(|delegation| {
                delegation.spec.isolation == Some(Isolation::Worktree)
            })
        );
        let retried = fail_then_retry("hold the plan open");
        assert!(
            retried.as_ref().is_err_and(|error| {
                matches!(
                    error.downcast_ref::<PlanOpError>(),
                    Some(PlanOpError::Contract { .. })
                )
            }),
            "{retried:?}"
        );
        Ok(())
    }

    // Dies with the token in `pending_verification`'s search (state.rs): match on the label
    // alone and an older effect left open by a dead claimant hides the live one, so two
    // concurrent `done` calls each mint an effect, run the checker twice and charge twice.
    #[test]
    fn a_leftover_open_effect_does_not_hide_the_live_verification() -> TestResult {
        let rig = rig("yi-f0c-leftover", None)?;
        let marker = rig._temp.join("runs.txt");
        let command = format!("sleep 1; echo run >> {}; exit 1", marker.display());
        let plan = planned()?;
        init(
            &rig.engine,
            vec![contracted(
                "race it",
                cmd_contract(&rig.store, &plan, &command, "writer")?,
            )?],
        )?;
        start(&rig.engine, &plan, "race it")?;
        // A verification requested under another snapshot by a claimant that never settled it.
        let journal = rig.store.journal(&plan);
        let last = journal
            .read()?
            .records
            .last()
            .cloned()
            .ok_or("empty journal")?;
        let stale = VerificationToken {
            plan: plan.clone(),
            version: PlanVersion(1),
            todo: TodoLabel::new("race it")?,
            attempt: AttemptId::FIRST,
            contract_digest: Digest::of(b""),
            criteria_digest: Digest::of(b""),
            output_digest: Digest::of(b""),
            snapshot: "tree:orphan".to_owned(),
            integration: None,
        };
        let mut value = serde_json::to_value(&last)?;
        value["op"] = json!("verification_requested");
        value["todo"] = json!("race it");
        value["requestId"] = json!("orphan-seed");
        value["attempt"] = json!(1);
        value["args"] = json!({"label": "race it", "token": stale, "effect_id": "e-0-orphan"});
        if let Some(fields) = value.as_object_mut() {
            fields.remove("from");
            fields.remove("to");
        }
        let orphan: JournalRecord = serde_json::from_value(value)?;
        journal.append(&journal.seal(orphan, Some(&last))?)?;
        let results: Vec<Result<Outcome, PlanOpError>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..2)
                .map(|_| {
                    let engine = Arc::clone(&rig.engine);
                    let plan = plan.clone();
                    scope.spawn(move || done(&engine, &plan, "race it", None))
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap_or(Err(PlanOpError::NoPlan)))
                .collect()
        });
        for result in results {
            assert_eq!(refused(result)?.outcome, VerdictOutcome::Fail);
        }
        assert_eq!(
            std::fs::read_to_string(&marker)?.lines().count(),
            1,
            "the checker ran once"
        );
        assert_eq!(todo_of(&rig.store, &plan, "race it")?.refusals, 1);
        let requested = kinds(&rig.store, &plan)?
            .iter()
            .filter(|kind| *kind == "verification_requested")
            .count();
        assert_eq!(requested, 2, "the leftover and the one live effect");
        Ok(())
    }

    // Dies with the materialization in `run_verifier` (done.rs): run the checker in the live
    // checkout instead and a checker that writes anything into it moves the tree step 5
    // re-captures, so its own pass reads as stale, forever.
    #[test]
    fn a_checker_that_writes_into_the_workspace_still_passes() -> TestResult {
        let rig = rig("yi-f0c-writing-checker", None)?;
        let plan = planned()?;
        std::fs::write(rig.ws.join("product.txt"), "the product")?;
        init(
            &rig.engine,
            vec![contracted(
                "ship it",
                cmd_contract(
                    &rig.store,
                    &plan,
                    "test -f product.txt && date +%s%N >> build.log; exit 0",
                    "writer",
                )?,
            )?],
        )?;
        start(&rig.engine, &plan, "ship it")?;
        let landed = done(&rig.engine, &plan, "ship it", None);
        assert!(landed.is_ok(), "{landed:?}");
        assert!(
            !rig.ws.join("build.log").exists(),
            "the checker wrote into its own materialization, never the checkout"
        );
        assert!(matches!(
            todo_of(&rig.store, &plan, "ship it")?.state,
            TodoState::Done {
                resolution: Some(Resolution::VerifiedDone),
                ..
            }
        ));
        Ok(())
    }

    // Dies with the re-capture in step 5 (`evidence` in done.rs called with no frozen
    // snapshot): hand the step 1 tree id back in and the comparison passes by construction,
    // so a checkout edited during a ten-minute check lands `VerifiedDone` for a tree that no
    // longer exists.
    #[test]
    fn a_workspace_edited_during_the_check_is_stale() -> TestResult {
        let rig = rig("yi-f0c-moved-workspace", None)?;
        let plan = planned()?;
        // The checker passes in its materialization and, as a concurrent editor would, writes
        // into the live checkout by absolute path.
        let command = format!(
            "echo edited > {}; exit 0",
            rig.ws.join("edited.txt").display()
        );
        init(
            &rig.engine,
            vec![contracted(
                "hold still",
                cmd_contract(&rig.store, &plan, &command, "writer")?,
            )?],
        )?;
        start(&rig.engine, &plan, "hold still")?;
        let stale = done(&rig.engine, &plan, "hold still", None);
        assert!(matches!(stale, Err(PlanOpError::Stale { .. })), "{stale:?}");
        assert!(rig.ws.join("edited.txt").is_file());
        let todo = todo_of(&rig.store, &plan, "hold still")?;
        assert!(matches!(todo.state, TodoState::Running { .. }));
        assert_eq!(
            todo.refusals, 0,
            "a moved workspace is not the product's failure"
        );
        let journal = Journal::open(rig.store.journal_path(&plan), Arc::new(RealFs));
        let last = journal
            .read()?
            .records
            .last()
            .cloned()
            .ok_or("empty journal")?;
        assert_eq!(last.record.op, "verification_stale");
        let detail = last.record.extra["refusal"]["detail"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        assert!(detail.contains("the workspace changed"), "{detail}");
        Ok(())
    }

    // Dies with the `Fail | Escalate` match on the settled verdict in `prepare` (done.rs):
    // replay every settled outcome and one abstention, an infrastructure failure, refuses a
    // correct product on every later `done` without running the checker again.
    #[test]
    fn an_abstained_verification_is_rerun_not_replayed() -> TestResult {
        let temp = Scratch::new("yi-f0c-abstain-rerun")?;
        let store = PlanStore::open(temp.join("plans"))?;
        let ws = temp.join("ws");
        std::fs::create_dir_all(&ws)?;
        let plan = planned()?;
        let starved = PlanEngine::new(store.clone(), Arc::new(Stub::default()))
            .with_cwd(ws.clone())
            .with_verifier(Verifier::new(1));
        init(
            &starved,
            vec![contracted(
                "slow but right",
                cmd_contract(&store, &plan, "sleep 0.3; exit 0", "writer")?,
            )?],
        )?;
        start(&starved, &plan, "slow but right")?;
        let abstained = refused(done(&starved, &plan, "slow but right", None))?;
        assert_eq!(abstained.outcome, VerdictOutcome::Abstain, "{abstained:?}");
        assert_eq!(todo_of(&store, &plan, "slow but right")?.refusals, 0);
        // The same token, a verifier with time to run: the checks run again and pass.
        let patient = PlanEngine::new(store.clone(), Arc::new(Stub::default()))
            .with_cwd(ws)
            .with_verifier(Verifier::new(20_000));
        let landed = done(&patient, &plan, "slow but right", None);
        assert!(landed.is_ok(), "{landed:?}");
        let todo = todo_of(&store, &plan, "slow but right")?;
        assert!(matches!(
            todo.state,
            TodoState::Done {
                resolution: Some(Resolution::VerifiedDone),
                ..
            }
        ));
        assert_eq!(todo.refusals, 0);
        assert_eq!(
            kinds(&store, &plan)?
                .iter()
                .filter(|kind| *kind == "verification_requested")
                .count(),
            2,
            "the abstention did not settle the token for good"
        );
        Ok(())
    }

    // Dies with the worktree test in `completion_of` (state.rs), the validator `set` shares
    // with `done`: leave it in `prepare` alone and a `[x]` row completes the worktree todo
    // `done` refuses, with the child still running and its lane still held.
    #[test]
    fn set_cannot_complete_a_worktree_todo() -> TestResult {
        let rig = rig("yi-f0c-worktree-set", None)?;
        let plan = planned()?;
        let mut spec = delegated_spec("build it apart")?;
        if let Some(delegation) = &mut spec.delegation {
            delegation.spec.isolation = Some(Isolation::Worktree);
        }
        spec.contract = Some(cmd_contract(&rig.store, &plan, "true", "writer")?);
        init(&rig.engine, vec![spec.clone()])?;
        let refused = rig.engine.apply(at(
            &plan,
            Op::Set {
                goal: None,
                rows: vec![SetRow {
                    spec,
                    state: TodoStateName::Done,
                }],
            },
        ));
        assert!(
            matches!(refused, Err(PlanOpError::AcceptanceUnavailable { .. })),
            "{refused:?}"
        );
        assert!(matches!(
            todo_of(&rig.store, &plan, "build it apart")?.state,
            TodoState::Running { .. }
        ));
        Ok(())
    }

    // Dies with `fitted` in done.rs: commit the refusal unrehearsed and a checker whose tail
    // does not fit the record cap leaves no `done_refused`, no charge and an open effect.
    #[test]
    fn a_verbose_refusal_still_journals_under_the_record_cap() -> TestResult {
        let rig = rig("yi-f0c-verbose-refusal", None)?;
        let plan = planned()?;
        // Six items, each ending in 2,000 control characters: six bytes apiece once escaped.
        let manifest = json!({
            "manifest": 1,
            "command": "awk 'BEGIN { for (i = 0; i < 2000; i++) printf \"\\001\" }'; exit 1",
            "cwd": "snapshot_root", "cwd_subdir": null, "protected": [], "timeout_ms": 10_000,
            "env": [], "reads_outside_snapshot": false
        });
        let put = rig.store.artifacts(&plan).put(
            &serde_json::to_vec(&manifest)?,
            "application/vnd.yi.checker-manifest+json",
            &rig.store.nonce(),
        )?;
        let items: Vec<Value> = (0..6)
            .map(|index| {
                json!({"id": format!("loud-{index}"), "critical": true, "weight": 100,
                       "decider": {"cmd": {"checker": put, "timeout_ms": 10_000}}})
            })
            .collect();
        let contract: Contract = serde_json::from_value(json!({
            "class": "writer", "items": items, "threshold": 1000, "min_coverage": 1000
        }))?;
        init(&rig.engine, vec![contracted("shout", contract)?])?;
        start(&rig.engine, &plan, "shout")?;
        let verdict = refused(done(&rig.engine, &plan, "shout", None))?;
        assert_eq!(verdict.outcome, VerdictOutcome::Fail);
        assert_eq!(todo_of(&rig.store, &plan, "shout")?.refusals, 1);
        assert!(
            kinds(&rig.store, &plan)?
                .iter()
                .any(|kind| kind == "done_refused")
        );
        let journaled = verdicts(&rig.store, &plan)?;
        let last = journaled.last().ok_or("no journaled verdict")?;
        assert!(
            last.lines().contains("[clipped]"),
            "the journaled details are clipped: {}",
            last.lines().len()
        );
        Ok(())
    }

    fn worktree_spec(label: &str, contract: Contract) -> Result<TodoSpec, Box<dyn Error>> {
        let mut spec = delegated_spec(label)?;
        if let Some(delegation) = &mut spec.delegation {
            delegation.spec.isolation = Some(Isolation::Worktree);
        }
        spec.contract = Some(contract);
        Ok(spec)
    }

    fn records(store: &PlanStore, plan: &PlanId) -> Result<Vec<JournalRecord>, Box<dyn Error>> {
        Ok(store.journal(plan).read()?.records)
    }

    /// One journal record appended behind the engine's back, the way a crashed or foreign
    /// process would leave it: sealed onto the chain, never reduced by this call.
    fn seed(
        store: &PlanStore,
        plan: &PlanId,
        kind: &str,
        label: &str,
        args: Value,
        verdict: Option<Value>,
    ) -> Result<(), Box<dyn Error>> {
        let journal = store.journal(plan);
        let last = journal
            .read()?
            .records
            .last()
            .cloned()
            .ok_or("empty journal")?;
        let mut value = serde_json::to_value(&last)?;
        value["op"] = json!(kind);
        value["todo"] = json!(label);
        value["requestId"] = json!(format!("seed-{kind}-{}", store.nonce()));
        value["attempt"] = json!(1);
        value["args"] = args;
        if let Some(fields) = value.as_object_mut() {
            fields.remove("from");
            fields.remove("to");
            fields.remove("verdict");
            fields.remove("effect_id");
            fields.remove("resolution");
            if let Some(verdict) = verdict {
                fields.insert("verdict".to_owned(), verdict);
                fields.insert("effect_id".to_owned(), json!("e-seeded"));
            }
        }
        let record: JournalRecord = serde_json::from_value(value)?;
        journal.append(&journal.seal(record, Some(&last))?)?;
        Ok(())
    }

    // Dies with `dispose` in `reap_leaving` (acceptance.rs): drop the lane on the reap and a
    // failed worktree todo finishes with no record naming its branch, which is the pressure
    // that makes a model merge a branch it knows is red.
    #[test]
    fn failed_unmerged_task_can_preserve_or_discard_and_finish() -> TestResult {
        for (choice, member) in [
            (yi_types::plan::op::Choice::Retained, "retained"),
            (yi_types::plan::op::Choice::Discarded, "discarded"),
        ] {
            let rig = rig(&format!("yi-f0d-{member}"), None)?;
            let plan = planned()?;
            init(
                &rig.engine,
                vec![worktree_spec(
                    "build it apart",
                    cmd_contract(&rig.store, &plan, "true", "writer")?,
                )?],
            )?;
            rig.engine.apply(at(
                &plan,
                Op::Fail {
                    label: TodoLabel::new("build it apart")?,
                    cause: "the checker was red".to_owned(),
                    disposition: Some(choice),
                },
            ))?;
            assert!(matches!(
                todo_of(&rig.store, &plan, "build it apart")?.state,
                TodoState::Failed { .. }
            ));
            let kinds = kinds(&rig.store, &plan)?;
            let disposition = kinds
                .iter()
                .position(|kind| kind == "disposition")
                .ok_or("no disposition record")?;
            let fail = kinds
                .iter()
                .position(|kind| kind == "fail")
                .ok_or("no fail record")?;
            assert!(disposition < fail, "the disposition lands first: {kinds:?}");
            assert!(!kinds.contains(&"accepted".to_owned()), "{kinds:?}");
            let records = records(&rig.store, &plan)?;
            let record = &records[disposition];
            let body = &record.args["disposition"][member];
            assert!(body.is_object(), "{member}: {:?}", record.args);
            assert_eq!(record.args["slot_released"], true);
            let kept: Url = body["kept"][0].as_str().ok_or("kept is empty")?.parse()?;
            assert_eq!(kept.to_string(), "history://child-0", "the pin resolves");
        }
        Ok(())
    }

    // Dies with `phase_of` in `try_publish` (acceptance.rs): test the lane instead of the
    // records and a todo whose lane was already taken looks acceptable.
    #[test]
    fn a_worktree_child_cannot_be_marked_done_before_acceptance() -> TestResult {
        let rig = rig("yi-f0d-phase", None)?;
        let plan = planned()?;
        let contract = cmd_contract(&rig.store, &plan, "true", "writer")?;
        init(
            &rig.engine,
            vec![worktree_spec("build it apart", contract.clone())?],
        )?;
        let refused = done(&rig.engine, &plan, "build it apart", None);
        assert!(
            matches!(
                &refused,
                Err(PlanOpError::PhaseMissing { phase, missing, .. })
                    if *phase == "unsubmitted" && *missing == "candidate_submitted"
            ),
            "{refused:?}"
        );
        assert!(matches!(
            todo_of(&rig.store, &plan, "build it apart")?.state,
            TodoState::Running { .. }
        ));
        // The records of a candidate that passed on its own branch, and nothing after.
        let token = VerificationToken {
            plan: plan.clone(),
            version: PlanVersion(1),
            todo: TodoLabel::new("build it apart")?,
            attempt: AttemptId::FIRST,
            contract_digest: contract.digest()?,
            criteria_digest: contract.criteria_digest()?,
            output_digest: Digest::of(b""),
            snapshot: "4c1f0b6d9e2a7358f0b1c4d5e6a7b8c9d0e1f234".to_owned(),
            integration: None,
        };
        seed(
            &rig.store,
            &plan,
            "candidate_submitted",
            "build it apart",
            json!({"label": "build it apart", "branch": "yi/child-0",
                   "candidate": token.snapshot, "parent_base": "9a7e3d21c0b8f4567a9e0d1c2b3a4958e6f7d8c9",
                   "outputs": [], "quiescent": {"at": 0, "running_commands": 0}}),
            None,
        )?;
        seed(
            &rig.store,
            &plan,
            "verification_requested",
            "build it apart",
            json!({"label": "build it apart", "token": token, "effect_id": "e-seeded"}),
            None,
        )?;
        let verdict = json!({"token": token, "outcome": "pass", "score": 1000, "coverage": 1000,
                             "items": [], "reproducible": true, "elapsed_ms": 1, "at": 1});
        seed(
            &rig.store,
            &plan,
            "candidate_verified",
            "build it apart",
            json!({"label": "build it apart", "token": token, "effect_id": "e-seeded"}),
            Some(verdict),
        )?;
        // A verified candidate whose integration never landed is prepared again by `done`
        // (section 6.6 row three); over the stub there is no lane pool to prepare it in, so
        // the preparation is refused as infrastructure and nothing completes.
        let refused = done(&rig.engine, &plan, "build it apart", None);
        assert!(
            matches!(&refused, Err(PlanOpError::Verification { .. })),
            "{refused:?}"
        );
        let kinds = kinds(&rig.store, &plan)?;
        assert!(!kinds.contains(&"accepted".to_owned()), "{kinds:?}");
        assert!(
            !kinds.contains(&"integration_prepared".to_owned()),
            "nothing was prepared without a pool: {kinds:?}"
        );
        assert!(matches!(
            todo_of(&rig.store, &plan, "build it apart")?.state,
            TodoState::Running { .. }
        ));
        // `fail` is legal here and takes the disposition path with the seeded refs.
        rig.engine.apply(at(
            &plan,
            Op::Fail {
                label: TodoLabel::new("build it apart")?,
                cause: "abandoning the candidate".to_owned(),
                disposition: None,
            },
        ))?;
        let records = records(&rig.store, &plan)?;
        let disposition = records
            .iter()
            .rev()
            .find(|record| record.record.op == "disposition")
            .ok_or("no disposition record")?;
        assert_eq!(
            disposition.args["disposition"]["retained"]["branch"], "yi/child-0",
            "the branch the records named"
        );
        assert!(matches!(
            todo_of(&rig.store, &plan, "build it apart")?.state,
            TodoState::Failed { .. }
        ));
        Ok(())
    }

    // Dies with `Purpose::Verification` being its own counter (capacity.rs): charge the
    // candidate check against the worker share and the workers a parent retains own every lane
    // their own verification needs, so the plan stops with every todo running. The engine
    // half is `lanes::full_worker_capacity_does_not_deadlock_verification`.
    #[test]
    fn full_worker_capacity_does_not_deadlock_verification() -> TestResult {
        // D203 took `DEFAULT_SLOTS` past the child cap, so the share only runs out in a
        // pool the parent's own children can fill; that pool is the child cap itself.
        let slots = u8::try_from(DEFAULT_MAX_CHILDREN)?;
        let capacity = Capacity::for_slots(slots);
        let share = usize::from(capacity.cap(Purpose::Worker));
        // Every worker the parent may retain asks for a checkout of its own. The share runs
        // out first, and it runs out one lane short of the pool: that lane is the reserve.
        let mut held = Vec::new();
        let mut refused = Vec::new();
        for _ in 0..DEFAULT_MAX_CHILDREN {
            match capacity.reserve(Purpose::Worker) {
                Ok(permit) => held.push(permit),
                Err(over) => refused.push(over),
            }
        }
        assert_eq!(held.len(), share, "the worker share is what workers get");
        assert_eq!(refused.len(), DEFAULT_MAX_CHILDREN - share);
        let over = refused.first().ok_or("the worker share never ran out")?;
        assert_eq!(
            (over.purpose, over.held, over.cap),
            (
                Purpose::Worker,
                capacity.cap(Purpose::Worker),
                capacity.cap(Purpose::Worker)
            ),
            "the refusal carries the count"
        );
        assert_eq!(
            usize::from(capacity.cap(Purpose::Verification)),
            usize::from(slots) - share,
            "the lanes the workers did not get are the verification's"
        );
        // With every worker lane held, the verification a retained worker's own candidate
        // needs still reserves, and a second one waits its turn: one candidate at a time.
        let verifying = capacity.reserve(Purpose::Verification)?;
        assert!(capacity.reserve(Purpose::Verification).is_err());
        drop(verifying);
        drop(held);
        assert_eq!(capacity.held(Purpose::Worker), 0);
        assert_eq!(capacity.held(Purpose::Verification), 0);
        Ok(())
    }
}
