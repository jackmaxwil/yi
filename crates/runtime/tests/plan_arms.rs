//! A plan row blocked on an `exec://` wait arms a command that runs on the host, unsandboxed:
//! each spelling a plan call carries meets a bash call's gate, through the tool and `plan.op`.
use crate::scratch::Scratch;

use serde_json::{Map, Value, json};
use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
use std::thread;
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_runtime::AgentSession;
use yi_runtime::rules::{RuleDoc, RuleGap, RuleMode, RuleScope};
use yi_runtime::schedule::clock::{self, Firing};
use yi_types::event::AgentEvent;
use yi_types::message::StopReason;

type TestResult = Result<(), Box<dyn Error>>;

/// A root session wired as `yi` wires one, a user gate rule on `denied-marker` in force.
fn root_session(root: &Path) -> Result<AgentSession, Box<dyn Error>> {
    let (session, _) = crate::kernel_across_sessions::root_session(root, "rlm", "family")?;
    std::fs::create_dir_all(root.join("rlm"))?;
    session.attach_store(crate::support::memory_store("root"))?;
    (session.heartbeat_service().ok_or("no heartbeats")?).bind_session("root".to_owned());
    // Into the wiring's own engine: the tool adapters hold the one they were built with.
    let rules = session.rules_engine().ok_or("no rules engine")?;
    rules.insert(RuleDoc {
        name: "no-denied".to_owned(),
        body: "Never run denied-marker.".to_owned(),
        path: PathBuf::from("/rules/no-denied.md"),
        needles: vec!["denied-marker".to_owned()],
        scope: RuleScope::Tool("bash".to_owned()),
        gap: RuleGap::AfterTurns(0),
        mode: RuleMode::Gate,
        paths: Vec::new(),
        after: 1,
    });
    Ok(session)
}

fn args(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

/// One plan call per turn: what each returned and whether it was an error.
async fn plan_calls(
    session: &AgentSession,
    calls: Vec<Value>,
) -> Result<Vec<(String, bool)>, Box<dyn Error>> {
    let mut seen = Vec::new();
    for call in calls {
        session.provider().queue_faux(vec![
            faux_assistant_message(
                vec![faux_tool_call("c", "plan", args(call))],
                StopReason::ToolUse,
            ),
            faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
        ]);
        let mut events = session.subscribe();
        session.prompt("go")?;
        session.wait_idle().await;
        while let Ok(event) = events.try_recv() {
            if let AgentEvent::ToolExecutionEnd {
                result, is_error, ..
            } = event
            {
                let text = yi_types::message::join_text(&result.content, "\n");
                seen.push((text, is_error));
            }
        }
    }
    Ok(seen)
}

/// Fires every wait the session armed, so each one's `exec://` command runs once, then waits
/// for the allowed control's marker: a denied marker absent past that point never ran.
async fn fire_armed(session: &AgentSession, allowed: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    let heartbeats = session.heartbeat_service().ok_or("no heartbeats")?;
    let todos = session.todos().ok_or("no todo list")?;
    let jobs = heartbeats.store().snapshot().jobs;
    for job in &jobs {
        clock::fire(&todos, job, &Firing::at(yi_session::now_ms()))?;
    }
    for _ in 0..200 {
        if allowed.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    Ok(jobs.into_iter().filter_map(|job| job.label).collect())
}

fn blocked_row(label: &str, on: Value) -> Value {
    json!({"op": "set", "goal": "watch the probes", "todos": [{"label": label, "state": "blocked", "on": on}]})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rows_armed_command_meets_the_gate_through_the_tool() -> TestResult {
    let root = Scratch::new("yi-plan-arms-tool")?;
    let session = root_session(&root)?;
    let (denied, allowed) = (root.join("denied-marker"), root.join("allowed"));
    let probe = |path: &Path| json!({"external": {"probe": format!("touch {}", path.display())}});
    let channel = json!({"channel": {"address": format!("exec://touch {}", denied.display())}});
    let seen = plan_calls(
        &session,
        vec![
            blocked_row("probe the marker", probe(&denied)),
            blocked_row("watch the marker", channel),
            json!({"op": "set", "todos": blocked_row("probe in a string", probe(&denied))["todos"].to_string()}),
            blocked_row("probe the control", probe(&allowed)),
        ],
    )
    .await?;
    let [probed, watched, stringed, control] = seen.as_slice() else {
        return Err(format!("four calls, got {seen:?}").into());
    };
    for (text, is_error) in [probed, watched, stringed] {
        assert!(*is_error && text.contains("Denied by rule"), "{text}");
    }
    assert!(!control.1, "the control row: {}", control.0);
    let armed = fire_armed(&session, &allowed).await?;
    assert!(allowed.exists(), "the control never ran: {armed:?}");
    assert!(!denied.exists(), "a denied command ran: {armed:?}");
    assert!(
        armed.iter().all(|label| !label.contains("denied-marker")),
        "{armed:?}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plan_wait_unblocks_once_when_firings_race() -> TestResult {
    let root = Scratch::new("yi-plan-arms-race")?;
    let session = root_session(&root)?;
    let seen = plan_calls(
        &session,
        vec![blocked_row(
            "probe the control",
            json!({"external": {"probe": "true"}}),
        )],
    )
    .await?;
    assert!(!seen[0].1, "the control row: {}", seen[0].0);
    let heartbeats = session.heartbeat_service().ok_or("no heartbeats")?;
    let job = heartbeats
        .store()
        .snapshot()
        .jobs
        .into_iter()
        .find(|job| job.unblocks.is_some())
        .ok_or("no armed wait")?;
    let plan = job.plan.ok_or("wait without a plan")?;
    let address = job.label.ok_or("wait without an address")?;
    let label = job.unblocks.ok_or("job without a todo")?;
    let todos = session.todos().ok_or("no todo list")?;
    let barrier = Arc::new(Barrier::new(32));
    let handles: Vec<_> = (0..32)
        .map(|_| {
            let todos = Arc::clone(&todos);
            let barrier = Arc::clone(&barrier);
            let plan = plan.clone();
            let label = label.clone();
            let address = address.clone();
            thread::spawn(move || {
                barrier.wait();
                todos.unblock_in(&plan, &label, &address)
            })
        })
        .collect();
    let mut outcomes = Vec::new();
    for handle in handles {
        outcomes.push(handle.join().map_err(|_| "wait racer panicked")?);
    }
    assert!(outcomes.iter().all(|result| result.is_ok()), "{outcomes:?}");
    let unblocked = outcomes.into_iter().collect::<Result<Vec<_>, _>>()?;
    assert_eq!(unblocked.into_iter().filter(|done| *done).count(), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cells_plan_op_meets_the_gate_its_tool_meets() -> TestResult {
    let root = Scratch::new("yi-plan-arms-cell")?;
    let session = root_session(&root)?;
    let kernel = session.kernel_service().ok_or("no kernel")?;
    let (denied, allowed) = (root.join("denied-marker"), root.join("allowed"));
    let code = format!(
        r#"
await rlm.plan_op("set", {{"goal": "watch", "list": "- [ ] probe the marker\n- [ ] watch the marker\n- [ ] probe the control"}})
for label, on in [
    ("probe the marker", {{"external": {{"probe": "touch {denied}"}}}}),
    ("watch the marker", {{"channel": {{"address": "exec://touch {denied}"}}}}),
    ("probe the control", {{"external": {{"probe": "touch {allowed}"}}}}),
]:
    reply = await rlm.plan_op("block", {{"label": label, "on": on, "note": "the probe"}})
    print(label, "|", reply["ok"], "|", (reply.get("refusal") or {{}}).get("message"))
"#,
        denied = denied.display(),
        allowed = allowed.display()
    );
    let outcome = tokio::task::spawn_blocking(move || {
        let cancelled: yi_tools::CancelFlag = Arc::new(|| false);
        yi_tools::KernelBridge::execute_cell(kernel.as_ref(), &code, "", &cancelled, None)
    })
    .await??;
    let printed = format!("{}{}", outcome.result.stdout, outcome.result.stderr);
    let row = |label: &str| {
        (printed.lines())
            .find(|line| line.starts_with(&format!("{label} |")))
            .unwrap_or_default()
            .to_owned()
    };
    for label in ["probe the marker", "watch the marker"] {
        let line = row(label);
        assert!(
            line.contains("| False |") && line.contains("Denied by rule"),
            "{printed}"
        );
    }
    assert!(row("probe the control").contains("| True |"), "{printed}");
    let armed = fire_armed(&session, &allowed).await?;
    if let Some(kernel) = session.kernel_service() {
        kernel.dispose().await;
    }
    assert!(allowed.exists(), "the control never ran: {armed:?}");
    assert!(!denied.exists(), "a denied command ran: {armed:?}");
    Ok(())
}
