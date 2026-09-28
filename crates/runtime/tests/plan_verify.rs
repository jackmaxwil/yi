//! F0c, the verifier: `done` runs the todo's frozen contract in the kernel, and the
//! checker it launches is a hostile subprocess that happens to be on our side.
//!
//! Test plan. Plan section 6.3's done path, in order, is: apply and locate the todo,
//! its current attempt, its frozen contract and criteria and its output artifacts;
//! commit `verification_requested` with the token and the effect id; release the
//! lease; run the items on `spawn_blocking` under one whole-verification deadline and
//! one deadline per item; re-acquire the lease and compare the whole token; commit the
//! verdict and the transition, or commit `done_refused`. The lease is released across
//! the run because a checker may take ten minutes, and the token comparison at the far
//! end is what makes that window safe. Every row below is one of those steps under
//! attack.
//!
//! The four rows §3.7's trust-boundary table names for the host to checker boundary
//! and back. The assumption there is that repository code is attacker influenced: the
//! checker runs a command the product's own repository supplies, so everything the
//! host hands it is something an attacker chose to receive.
//!
//! | test | tier | what it pins | the control it dies with |
//! |---|---|---|---|
//! | `the_checker_sees_no_provider_key` | T0 | A checker that prints its whole environment shows `PATH`, `HOME`, `LANG`, `TMPDIR` and the names its own manifest declared, and nothing else: no `ANTHROPIC_API_KEY`, no `OPENROUTER_API_KEY`, no token of any shape the session holds. The manifest declares names, never values, so a criterion cannot smuggle a secret into the store either. | `env_clear()` plus the allowlist on the checker's `Command`. Today `run_captured` (`crates/tools/src/process.rs:202-232`) sets no `env_clear` anywhere under `crates/`, so this test is red on the tree as it stands: a checker running repository code inherits the whole session environment. Delete the `env_clear` and the assertion over the printed environment fails on the first key. |
//! | `a_checkers_grandchild_is_killed_at_the_deadline` | T1 | `sh -c 'sleep 600 &'` returns at once and leaves a grandchild behind. The verification's deadline expires, the whole process group is killed as a tree, the join returns, and nothing that the checker started is still running when the verdict is committed. The item is `Abstain` with a timeout reason, not `Fail`: an infrastructure timeout is not the product's failure and charges no refusal. | The process group (`process_group(0)`, `process.rs:231`) plus `kill_tree` (`process.rs:33`, `group_kill` :132-144) plus the `CancelFlag` the deadline drives (`goal/mod.rs:52`), and the join before the verdict. A `spawn_blocking` job cannot be aborted, so the flag is the only bound; drop the group and the direct child dies while the grandchild survives the run, which is the leak this test exists for. |
//! | `checker_cannot_gain_permission_through_contract` | T0 | A contract whose manifest asks for a cwd outside the snapshot, a `cwd_subdir` containing `..`, a path the session's wall denies, or an environment name the session does not hold is refused at `start`, when the contract is frozen, and never at `done`. A checker runs under the session's own permission mode and cancellation; declaring a criterion is not a way to ask for more than the declaring session has. | The manifest validation at freeze time plus running the checker under the session's existing permission mode rather than a mode of its own. Widen the cwd policy to accept an absolute path and the first case passes; move the check from `start` to `done` and a plan can sit in the store for a week naming a criterion nobody may run. The wall is cooperative either way (`wall.rs:108-109`), and the verdict says so: this is a refusal of a declaration, not a sandbox. |
//! | `an_examples_runner_that_cannot_spawn_abstains` | T0 | A runner whose working directory the host cannot enter abstains the item with the spawn reason, the same reason the cmd decider abstains and `Verifier::run` retries; a critical abstention aggregates to `Abstain`, which charges no refusal. | `CaseError::Host` in `run_examples`. Report the spawn as a failing case and the same host fault the cmd path retries and forgives fails the example item and is charged to the product. |
//! | `example_json_equality_ignores_whitespace_and_key_order` | T0 | The five cases of `fixtures/plans/contracts/cases.json`. Cases 0, 1 and 2 pass on three spellings of the same value, the second with its keys reordered and the third with padding; case 3 answers wrongly and case 4 prints prose after its JSON, and those two fail for reasons the item's detail distinguishes. Stdout must be one complete JSON value: several values, non-finite numbers, a timeout and output overflow are their own failures, not near misses. | Structural comparison over the parsed values, and the one-complete-value rule on the far side of it. Compare bytes and cases 1 and 2 fail, which makes the runner's formatter part of the contract; take the first JSON value and ignore the rest and case 4 passes, which teaches a model that a summary line after the answer is free. |
//! | `a_checker_that_edits_a_protected_path_fails` | T0 | A manifest lists `runner.sh` as protected and its command rewrites that file to `exit 0`: the item fails naming the path, even though the command itself exited 0. The same manifest with a command that leaves the path alone passes. | `protecting` in `verify.rs`, the digests of every protected path taken before and after the run. Drop the comparison and a checker that rewrites its own script mid-run passes on the rewritten script, which is the edit `contracts.md`'s `protected` row exists against. |
//! | `the_session_deadline_bounds_the_verification` | T0 | A verifier whose own clock allows a minute but whose session deadline (D177) has already passed abstains every item with the deadline reason and runs no command. | `Verifier::with_deadline` folded into the whole-verification deadline in `run`. Read only `timeout_ms` and a checker launched at the wire runs ten minutes past the run's own clock. |
//! | `example_failures_are_distinct_by_kind` | T0 | One case, ten runners. A right answer passes; a wrong one, prose after the JSON, two values, a non-finite number, unparsable output, no output, a non-zero exit, output past the capture cap and a runner that outruns its deadline each fail with their own reason in the item's detail. | The distinct reasons `one_json_value` and `run_case` return. Fold them into one "failed" and the detail stops naming what to repair, which is the only thing a refused author can act on; read the first value and drop the rest and `trailing_output` becomes a pass. |
//!
//! Fixtures this file reads: `fixtures/plans/contracts/examples-runner.json` and its
//! `cases.json`, `fixtures/plans/contracts/checker-manifest.example.json`, and
//! `fixtures/plans/contracts/contracts.md` for the manifest's field rules. The
//! red-then-green pair (`writer-cmd-red-then-green.json`) is driven from
//! `plan_ops.rs`, because what it pins is the engine's refusal path rather than the
//! checker's process shape.
//!
//! Not here. The judge tier is F3a and lives in `tests/judge.rs`: every `Snapshot` below
//! seats no jury (`jury: None`), which is the path where a `judge` item abstains.

use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use yi_runtime::plan::artifact::Artifacts;
use yi_runtime::plan::ops::{Actor, Op, OpRequest, PlanEngine, PlanOpError, TodoSpec};
use yi_runtime::plan::store::PlanStore;
use yi_runtime::plan::verify::{CheckerManifest, Snapshot, Verifier, freeze};
use yi_types::plan::PlanVersion;
use yi_types::plan::canonical::{ArtifactRef, Digest};
use yi_types::plan::contract::{Contract, ItemVerdict, Outcome, VerificationToken};
use yi_types::plan::doc::{AttemptId, GoalText, PlanId, TodoLabel, TodoState};

type TestResult = Result<(), Box<dyn Error>>;

fn manifest(command: &str, timeout_ms: u64, env: &[&str]) -> serde_json::Value {
    json!({
        "manifest": 1, "command": command, "cwd": "snapshot_root", "cwd_subdir": null,
        "protected": [], "timeout_ms": timeout_ms, "env": env, "reads_outside_snapshot": false
    })
}

fn put(artifacts: &Artifacts, value: &serde_json::Value) -> Result<ArtifactRef, Box<dyn Error>> {
    Ok(artifacts.put(
        &serde_json::to_vec(value)?,
        "application/vnd.yi.checker-manifest+json",
        "test",
    )?)
}

fn cmd_contract(checker: ArtifactRef, timeout_ms: u64) -> Result<Contract, Box<dyn Error>> {
    Ok(serde_json::from_value(json!({
        "class": "writer",
        "items": [{"id": "check", "critical": true, "weight": 100,
                   "decider": {"cmd": {"checker": checker, "timeout_ms": timeout_ms}}}],
        "threshold": 1000, "min_coverage": 1000
    }))?)
}

fn contracts_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plans/contracts")
}

fn fixture(stem: &str) -> Result<Value, Box<dyn Error>> {
    let path = contracts_dir().join(format!("{stem}.json"));
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

/// One declared blob's bytes: inline `text`, or a sibling file of the fixture.
fn blob(doc: &Value, alias: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let declared = &doc["artifacts"][alias];
    match (declared.get("text"), declared.get("file")) {
        (Some(Value::String(text)), _) => Ok(text.as_bytes().to_vec()),
        (_, Some(Value::String(file))) => Ok(std::fs::read(contracts_dir().join(file))?),
        _ => Err(format!("{alias} has neither text nor file").into()),
    }
}

/// Every blob the fixture declares, written into the plan's store and checked against the digest
/// and length it wrote down, so an edit that forgets them fails here rather than drifting.
fn stage(artifacts: &Artifacts, doc: &Value) -> Result<(), Box<dyn Error>> {
    for (alias, declared) in doc["artifacts"].as_object().ok_or("artifacts")? {
        let media = declared["media_type"].as_str().ok_or("media_type")?;
        let put = artifacts.put(&blob(doc, alias)?, media, "fixture")?;
        assert_eq!(
            Some(put.digest.to_string().as_str()),
            declared["digest"].as_str(),
            "{alias}: the fixture's digest must match its bytes"
        );
        assert_eq!(Some(put.length), declared["length"].as_u64(), "{alias}");
    }
    Ok(())
}

fn owner(plan: Option<PlanId>, op: Op) -> OpRequest {
    OpRequest {
        plan,
        actor: Actor::Owner,
        op,
        request_id: None,
        expected_revision: None,
    }
}

fn state_tag(state: &TodoState) -> &str {
    match state {
        TodoState::Pending => "pending",
        TodoState::Running { .. } => "running",
        TodoState::Blocked { .. } => "blocked",
        TodoState::Done { .. } => "done",
        TodoState::Failed { .. } => "failed",
        TodoState::Abandoned => "abandoned",
        TodoState::Other(tag) => tag.as_str(),
    }
}

fn token(contract: &Contract) -> Result<VerificationToken, Box<dyn Error>> {
    Ok(VerificationToken {
        plan: PlanId::new("verify-tests")?,
        version: PlanVersion(1),
        todo: TodoLabel::new("check it")?,
        attempt: AttemptId::FIRST,
        contract_digest: contract.digest()?,
        criteria_digest: contract.criteria_digest()?,
        output_digest: Digest::of(b""),
        snapshot: "tree:test".to_owned(),
        integration: None,
    })
}

// Dies with `env_clear()` in `run_check_in`: delete it and the first cargo variable of this
// test process shows up in the checker's printed environment.
#[test]
fn the_checker_sees_no_provider_key() -> TestResult {
    let temp = Scratch::new("yi-verify-env")?;
    let artifacts = Artifacts::under(&temp);
    let out = temp.join("env.txt");
    // CARGO_PKG_NAME stands in for a provider key: a name this process holds that no manifest
    // declared. Declared names pass through; every other one is gone.
    let checker = put(
        &artifacts,
        &manifest(
            &format!("env > {}", out.display()),
            5_000,
            &["CARGO_MANIFEST_DIR"],
        ),
    )?;
    let contract = cmd_contract(checker, 5_000)?;
    let verdict = Verifier::new(5_000).run(
        &token(&contract)?,
        &contract,
        &Snapshot {
            id: "tree:test",
            root: &temp,
            output: None,
            artifacts: &artifacts,
            jury: None,
        },
    );
    assert_eq!(verdict.outcome, Outcome::Pass, "{}", verdict.lines());
    let printed = std::fs::read_to_string(&out)?;
    let names: Vec<&str> = printed
        .lines()
        .filter_map(|line| line.split_once('=').map(|(name, _)| name))
        .collect();
    assert!(
        names.contains(&"PATH") && names.contains(&"CARGO_MANIFEST_DIR"),
        "{names:?}"
    );
    let leaked: Vec<&&str> = names
        .iter()
        .filter(|name| {
            !matches!(
                **name,
                "PATH" | "HOME" | "LANG" | "TMPDIR" | "CARGO_MANIFEST_DIR"
            ) && !name.starts_with("PWD")
                && !name.starts_with("SHLVL")
                && !name.starts_with('_')
                && !name.starts_with("OLDPWD")
        })
        .collect();
    assert!(leaked.is_empty(), "the checker inherited {leaked:?}");
    assert!(
        !names
            .iter()
            .any(|name| name.contains("KEY") || name.contains("TOKEN")),
        "{names:?}"
    );
    Ok(())
}

// Dies with the process group and `kill_tree` behind `run_captured`, driven by the deadline in
// the cancel flag: drop the group and the shell dies while its backgrounded sleep survives.
#[test]
fn a_checkers_grandchild_is_killed_at_the_deadline() -> TestResult {
    let temp = Scratch::new("yi-verify-grandchild")?;
    let artifacts = Artifacts::under(&temp);
    let checker = put(&artifacts, &manifest("sleep 5993 &", 30_000, &[]))?;
    let contract = cmd_contract(checker, 30_000)?;
    let started = std::time::Instant::now();
    let verdict = Verifier::new(1_000).run(
        &token(&contract)?,
        &contract,
        &Snapshot {
            id: "tree:test",
            root: &temp,
            output: None,
            artifacts: &artifacts,
            jury: None,
        },
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "the deadline bounded the run"
    );
    assert_eq!(
        verdict.outcome,
        Outcome::Abstain,
        "a timeout is not the product's failure"
    );
    assert!(
        matches!(&verdict.items[0].verdict, ItemVerdict::Abstain { reason } if reason.contains("timed out")),
        "{}",
        verdict.lines()
    );
    let mut alive = true;
    for _ in 0..20 {
        let survivors = yi_tools::run_captured(
            {
                let mut ps = yi_tools::command("/bin/sh");
                ps.args(["-c", "ps -axo command | grep -c '^sleep 5993' || true"]);
                ps
            },
            None,
            &(Arc::new(|| false) as yi_tools::CancelFlag),
            4_096,
        )?;
        alive = survivors.stdout.trim() != "0";
        if !alive {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(!alive, "the grandchild outlived the verdict");
    Ok(())
}

// Dies with `CheckerManifest::parse` and its call from `freeze` at start: widen the cwd policy
// or move the check to done and a plan can name a criterion nobody may run.
#[test]
fn checker_cannot_gain_permission_through_contract() -> TestResult {
    let bad = [
        (
            json!({"manifest": 1, "command": "true", "cwd": "snapshot_subdir", "cwd_subdir": "../elsewhere",
                "protected": [], "timeout_ms": 1000, "env": [], "reads_outside_snapshot": false}),
            "inside the snapshot",
        ),
        (
            json!({"manifest": 1, "command": "true", "cwd": "snapshot_subdir", "cwd_subdir": "/etc",
                "protected": [], "timeout_ms": 1000, "env": [], "reads_outside_snapshot": false}),
            "inside the snapshot",
        ),
        (
            json!({"manifest": 1, "command": "true", "cwd": "absolute", "cwd_subdir": null,
                "protected": [], "timeout_ms": 1000, "env": [], "reads_outside_snapshot": false}),
            "checker manifest",
        ),
        (
            manifest("true", 1000, &["OPENROUTER_API_KEY"]),
            "looks like a secret",
        ),
        (
            manifest("true", 1000, &["YI_VERIFY_TEST_UNSET_NAME"]),
            "holds no",
        ),
        (
            json!({"manifest": 1, "command": "true", "cwd": "snapshot_root", "cwd_subdir": null, "shell": "/bin/zsh",
                "protected": [], "timeout_ms": 1000, "env": [], "reads_outside_snapshot": false}),
            "unknown field",
        ),
        (
            json!({"manifest": 2, "command": "true", "cwd": "snapshot_root", "cwd_subdir": null,
                "protected": [], "timeout_ms": 1000, "env": [], "reads_outside_snapshot": false}),
            "format 2",
        ),
    ];
    for (value, expected) in bad {
        let refusal = CheckerManifest::parse(&serde_json::to_vec(&value)?)
            .err()
            .ok_or_else(|| format!("{value} was accepted"))?;
        assert!(refusal.contains(expected), "{value}: {refusal}");
    }
    CheckerManifest::parse(&serde_json::to_vec(&manifest("true", 1000, &["PATH"]))?)?;
    // Through the engine: the refusal lands at start, when the contract is frozen, never at done.
    let temp = Scratch::new("yi-verify-freeze")?;
    let store = PlanStore::open(temp.join("plans"))?;
    let engine = PlanEngine::new(
        store.clone(),
        Arc::new(yi_runtime::plan::authority::Unhosted),
    )
    .with_cwd(temp.to_path_buf());
    let opened = engine.apply(OpRequest {
        plan: None,
        actor: Actor::Owner,
        op: Op::Init {
            goal: GoalText::new("freeze a bad criterion")?,
            todos: vec![TodoSpec {
                label: TodoLabel::new("check it")?,
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
    let plan = opened.plan.id.clone();
    let escaping = put(
        &store.artifacts(&plan),
        &json!({"manifest": 1, "command": "true", "cwd": "snapshot_subdir", "cwd_subdir": "../elsewhere",
                "protected": [], "timeout_ms": 1000, "env": [], "reads_outside_snapshot": false}),
    )?;
    let contract = cmd_contract(escaping, 1000)?;
    assert!(freeze(&store.artifacts(&plan), &contract).is_err());
    let mut spec = TodoSpec {
        label: TodoLabel::new("check it")?,
        after: Vec::new(),
        delegation: None,
        contract: Some(contract),
        children: Vec::new(),
        cites: Default::default(),
    };
    engine.apply(OpRequest {
        plan: Some(plan.clone()),
        actor: Actor::Owner,
        op: Op::Set {
            goal: None,
            rows: vec![yi_runtime::plan::ops::SetRow {
                spec: spec.clone(),
                state: yi_types::plan::doc::TodoStateName::Pending,
            }],
        },
        request_id: None,
        expected_revision: None,
    })?;
    let started = engine.apply(OpRequest {
        plan: Some(plan.clone()),
        actor: Actor::Owner,
        op: Op::Start {
            label: TodoLabel::new("check it")?,
        },
        request_id: None,
        expected_revision: None,
    });
    assert!(
        matches!(started, Err(PlanOpError::Contract { .. })),
        "start freezes and refuses: {started:?}"
    );
    spec.contract = None;
    let todo = store.read(&plan)?;
    assert!(
        matches!(
            todo.todo(&TodoLabel::new("check it")?)
                .map(|todo| &todo.state),
            Some(yi_types::plan::doc::TodoState::Pending)
        ),
        "nothing started"
    );
    Ok(())
}

// Dies with the structural comparison and the one-complete-value rule: compare bytes and the
// reordered and padded cases fail, read only the first value and the chatty case passes.
#[test]
fn example_json_equality_ignores_whitespace_and_key_order() -> TestResult {
    let doc = fixture("examples-runner")?;
    let temp = Scratch::new("yi-f0c-examples")?;
    let store = PlanStore::open(temp.join("plans"))?;
    let ws = temp.join("ws");
    std::fs::create_dir_all(&ws)?;
    let engine = PlanEngine::new(
        store.clone(),
        Arc::new(yi_runtime::plan::authority::Unhosted),
    )
    .with_cwd(ws.clone())
    .with_verifier(Verifier::new(20_000));

    let declared = &doc["steps"][0]["args"]["todos"][0];
    let label = TodoLabel::new(declared["label"].as_str().ok_or("label")?)?;
    let opened = engine.apply(owner(
        None,
        Op::Init {
            goal: GoalText::new(doc["goal"].as_str().ok_or("goal")?)?,
            todos: vec![TodoSpec {
                label: label.clone(),
                after: Vec::new(),
                delegation: None,
                contract: Some(serde_json::from_value(declared["contract"].clone())?),
                children: Vec::new(),
                cites: Default::default(),
            }],
        },
    ))?;
    let plan = opened.plan.id.clone();
    assert_eq!(Some(plan.as_str()), doc["plan"].as_str());
    stage(&store.artifacts(&plan), &doc)?;
    engine.apply(owner(
        Some(plan.clone()),
        Op::Start {
            label: label.clone(),
        },
    ))?;

    // The runner and its case file are staged where the checker runs, not where it is declared.
    let step = &doc["steps"][2];
    for (path, alias) in step["workspace"].as_object().ok_or("workspace")? {
        std::fs::write(ws.join(path), blob(&doc, alias.as_str().ok_or("alias")?)?)?;
    }
    let refused = engine
        .apply(owner(
            Some(plan.clone()),
            Op::Done {
                label: label.clone(),
                output: None,
            },
        ))
        .err()
        .ok_or("done passed a case file two of whose cases fail")?;
    let verdict = match &refused {
        PlanOpError::Refused { verdict, .. } => verdict.clone(),
        other => return Err(format!("expected a refusal carrying a verdict: {other}").into()),
    };
    let want = &step["expect"]["verdict"];
    assert_eq!(
        verdict.outcome,
        serde_json::from_value::<Outcome>(want["outcome"].clone())?,
        "{}",
        verdict.lines()
    );
    assert_eq!(Some(u64::from(verdict.score)), want["score"].as_u64());
    assert_eq!(Some(u64::from(verdict.coverage)), want["coverage"].as_u64());
    let line = verdict
        .items
        .first()
        .ok_or("the verdict carries no item line")?;
    assert_eq!(Some(line.id.as_str()), want["items"][0]["id"].as_str());
    let ItemVerdict::Fail { detail } = &line.verdict else {
        return Err(format!("expected a failing item: {}", verdict.lines()).into());
    };
    if let Some(substring) = want["items"][0]["detail"].as_str() {
        assert!(detail.contains(substring), "{detail}");
    }
    let mut failing = Vec::new();
    for case in step["expect"]["cases"].as_array().ok_or("cases")? {
        let index = case["index"].as_u64().ok_or("index")?;
        if case["outcome"].as_str() == Some("fail") {
            let why = case["why"].as_str().ok_or("why")?;
            assert!(
                detail.contains(&format!("{index} {why}")),
                "case {index} must fail as {why}: {detail}"
            );
            failing.push(index.to_string());
        }
    }
    assert!(
        detail.starts_with(&format!("cases {} failed:", failing.join(", "))),
        "the cases the fixture passes are absent from the failing list: {detail}"
    );

    let file = store.read(&plan)?;
    for want in step["expect"]["todos"].as_array().ok_or("todos")? {
        let todo = file
            .todo(&TodoLabel::new(want["label"].as_str().ok_or("label")?)?)
            .ok_or("todo")?;
        assert_eq!(want["state"].as_str(), Some(state_tag(&todo.state)));
        assert_eq!(Some(u64::from(todo.refusals)), want["refusals"].as_u64());
    }
    assert_eq!(
        Some(u64::try_from(store.list()?.len())?),
        doc["final"]["planCount"].as_u64()
    );
    Ok(())
}

// Dies with the distinct reasons `one_json_value` and `run_case` return: fold them into one
// "failed" and the detail stops naming what the author has to repair.
#[test]
fn example_failures_are_distinct_by_kind() -> TestResult {
    let temp = Scratch::new("yi-verify-example-kinds")?;
    let artifacts = Artifacts::under(&temp);
    let cases = artifacts.put(
        &serde_json::to_vec(&json!([{"input": {"op": "one"}, "expected": {"result": 1}}]))?,
        "application/json",
        "test",
    )?;
    for (command, why) in [
        (r#"printf '{  "result" :  1  }'"#, ""),
        (r#"printf '{"result": 2}'"#, "wrong_answer"),
        (
            r#"printf '{"result": 1}\nAll cases passed!'"#,
            "trailing_output",
        ),
        (r#"printf '{"result": 1} {"result": 1}'"#, "several_values"),
        ("printf 'NaN'", "non_finite"),
        ("printf 'all good'", "not_json"),
        ("printf ''", "empty_output"),
        ("exit 3", "nonzero_exit"),
        (
            "awk 'BEGIN { for (i = 0; i < 40000; i++) printf \"a\" }'",
            "output_overflow",
        ),
        ("sleep 30", "timeout"),
    ] {
        let timeout_ms = if why == "timeout" { 300 } else { 10_000 };
        let runner = put(&artifacts, &manifest(command, timeout_ms, &[]))?;
        let contract: Contract = serde_json::from_value(json!({
            "class": "writer",
            "items": [{"id": "cases", "critical": true, "weight": 100,
                       "decider": {"example": {"cases": cases, "runner": runner,
                                               "timeout_ms": timeout_ms}}}],
            "threshold": 1000, "min_coverage": 1000
        }))?;
        let verdict = Verifier::new(20_000).run(
            &token(&contract)?,
            &contract,
            &Snapshot {
                id: "tree:test",
                root: &temp,
                output: None,
                artifacts: &artifacts,
                jury: None,
            },
        );
        if why.is_empty() {
            assert_eq!(verdict.outcome, Outcome::Pass, "{}", verdict.lines());
            continue;
        }
        assert_eq!(
            verdict.outcome,
            Outcome::Fail,
            "{command}: {}",
            verdict.lines()
        );
        let line = verdict.items.first().ok_or("no item line")?;
        let ItemVerdict::Fail { detail } = &line.verdict else {
            return Err(format!("{command}: {}", verdict.lines()).into());
        };
        assert_eq!(detail, &format!("cases 0 failed: 0 {why}"), "{command}");
    }
    Ok(())
}

// Dies with `protecting` in verify.rs: drop the before-and-after digests and a checker that
// rewrites its own script mid-run passes on the rewritten script.
#[test]
fn a_checker_that_edits_a_protected_path_fails() -> TestResult {
    let temp = Scratch::new("yi-verify-protected")?;
    let artifacts = Artifacts::under(&temp);
    std::fs::write(temp.join("runner.sh"), "exit 1\n")?;
    let mut editing = manifest("printf 'exit 0\\n' > runner.sh; exit 0", 5_000, &[]);
    editing["protected"] = json!(["runner.sh"]);
    let mut honest = manifest("sh ./runner.sh || exit 0", 5_000, &[]);
    honest["protected"] = json!(["runner.sh"]);
    let snapshot = Snapshot {
        id: "tree:test",
        root: &temp,
        output: None,
        artifacts: &artifacts,
        jury: None,
    };
    let contract = cmd_contract(put(&artifacts, &editing)?, 5_000)?;
    let verdict = Verifier::new(5_000).run(&token(&contract)?, &contract, &snapshot);
    assert_eq!(verdict.outcome, Outcome::Fail, "{}", verdict.lines());
    assert!(
        verdict.lines().contains("protected path runner.sh changed"),
        "{}",
        verdict.lines()
    );
    std::fs::write(temp.join("runner.sh"), "exit 1\n")?;
    let contract = cmd_contract(put(&artifacts, &honest)?, 5_000)?;
    let verdict = Verifier::new(5_000).run(&token(&contract)?, &contract, &snapshot);
    assert_eq!(verdict.outcome, Outcome::Pass, "{}", verdict.lines());
    Ok(())
}

// Dies with `with_deadline` folded into `run`'s whole deadline: read only `timeout_ms` and the
// command below runs, and passes, past the session's own clock.
#[test]
fn the_session_deadline_bounds_the_verification() -> TestResult {
    let temp = Scratch::new("yi-verify-deadline")?;
    let artifacts = Artifacts::under(&temp);
    let marker = temp.join("ran.txt");
    let checker = put(
        &artifacts,
        &manifest(&format!("touch {}", marker.display()), 5_000, &[]),
    )?;
    let contract = cmd_contract(checker, 5_000)?;
    let verdict = Verifier::new(60_000)
        .with_deadline(std::time::Instant::now())
        .run(
            &token(&contract)?,
            &contract,
            &Snapshot {
                id: "tree:test",
                root: &temp,
                output: None,
                artifacts: &artifacts,
                jury: None,
            },
        );
    assert_eq!(verdict.outcome, Outcome::Abstain, "{}", verdict.lines());
    assert!(
        verdict.lines().contains("deadline passed"),
        "{}",
        verdict.lines()
    );
    assert!(
        !marker.exists(),
        "the checker ran past the session deadline"
    );
    Ok(())
}

// Dies with `CaseError::Host` in `run_examples` (verify.rs): fold a runner the host could not
// spawn into a failing case and a host fault fails the item and is charged to the product.
#[test]
fn an_examples_runner_that_cannot_spawn_abstains() -> TestResult {
    let temp = Scratch::new("yi-verify-example-spawn")?;
    let artifacts = Artifacts::under(&temp);
    let cases = artifacts.put(
        &serde_json::to_vec(&json!([{"input": 1, "expected": 1}]))?,
        "application/json",
        "test",
    )?;
    let mut nowhere = manifest("cat", 5_000, &[]);
    nowhere["cwd"] = json!("snapshot_subdir");
    nowhere["cwd_subdir"] = json!("missing");
    let runner = put(&artifacts, &nowhere)?;
    let contract: Contract = serde_json::from_value(json!({
        "class": "writer",
        "items": [{"id": "cases", "critical": true, "weight": 100,
                   "decider": {"example": {"cases": cases, "runner": runner,
                                           "timeout_ms": 5_000}}}],
        "threshold": 1000, "min_coverage": 1000
    }))?;
    let verdict = Verifier::new(5_000).run(
        &token(&contract)?,
        &contract,
        &Snapshot {
            id: "tree:test",
            root: &temp,
            output: None,
            artifacts: &artifacts,
            jury: None,
        },
    );
    assert_eq!(verdict.outcome, Outcome::Abstain, "{}", verdict.lines());
    let line = verdict.items.first().ok_or("no item line")?;
    let ItemVerdict::Abstain { reason } = &line.verdict else {
        return Err(verdict.lines().into());
    };
    assert!(reason.starts_with("failed to spawn"), "{reason}");
    Ok(())
}
