use crate::scratch;
use scratch::Scratch;

use std::collections::HashSet;
use std::error::Error;
use std::sync::{Arc, Condvar, Mutex};

use yi_ai::faux::{faux_assistant_message, faux_text};
use yi_kernel::client::HostHandlers;
use yi_loop::ExecutionMode;
use yi_runtime::schedule::clock::{self, Fired};
use yi_runtime::schedule::{
    DeliverFn, Firing, HeartbeatCommand, HeartbeatService, INTERRUPTED_ERROR, JobSpec, JobStore,
    RunOutcome, Scheduler, claim_due_in_state, new_job, parse_heartbeat_command, parse_iso_ms,
    parse_schedule,
};
use yi_runtime::todo::{Op, Target, TodoStore};
use yi_runtime::{AgentSession, HostRegistry, ProviderStream, SessionConfig};
use yi_types::message::{AgentMessage, Content, StopReason, UserContent};
use yi_types::model::{Model, ModelCost};
use yi_types::plan::doc::{BlockedOn, Todo, TodoState};
use yi_types::schedule::{
    CatchUp, CronSchedule, DispatchRecord, JobSource, JobStatus, Overlap, ScheduleKind,
    ScheduleState,
};

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

fn temp_store(name: &str) -> std::io::Result<(Scratch, Arc<JobStore>)> {
    let dir = Scratch::new(&format!("yi-sched-{name}"))?;
    let path = dir.join("scheduled-jobs.json");
    Ok((dir, Arc::new(JobStore::open(path))))
}

fn heartbeat_job(id: &str, interval_ms: u64, next_run_at: u64) -> yi_types::schedule::Job {
    let now = yi_session::now_ms();
    let mut job = new_job(JobSpec {
        id: id.to_owned(),
        session_id: "test".to_owned(),
        cwd: "/tmp".to_owned(),
        source: JobSource::Heartbeat,
        delivery_mode: None,
        label: None,
        prompt: "check the build".to_owned(),
        schedule: yi_types::schedule::CronSchedule {
            kind: yi_types::schedule::ScheduleKind::Interval,
            expression: "every 10s".to_owned(),
            interval_ms: Some(interval_ms),
        },
        next_run_at: now,
        now_ms: now,
    });
    job.next_run_at = Some(next_run_at);
    job
}

fn memory_store(id: &str) -> yi_session::SharedSession {
    Arc::new(Mutex::new(yi_session::SessionStore::in_memory(
        yi_session::SessionMetadata {
            id: id.to_owned(),
            created_at: 0,
            parent_session_id: None,
            name: None,
        },
    )))
}

/// A session the way `attach_runtime` leaves one for the clock: a ledger and a todo list.
fn clock_session(reply: &str) -> Result<(AgentSession, Arc<TodoStore>), Box<dyn Error>> {
    let provider = Arc::new(ProviderStream::new(None, None));
    provider.queue_faux(vec![faux_assistant_message(
        vec![faux_text(reply)],
        StopReason::Stop,
    )]);
    let session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );
    session.attach_store(memory_store("test"))?;
    let todos = TodoStore::new(session.store_handle(), "main");
    session.set_todos(Arc::clone(&todos));
    Ok((session, todos))
}

fn answered(session: &AgentSession, reply: &str) -> bool {
    session.messages().iter().any(|message| {
        matches!(message, AgentMessage::Assistant { content, .. }
            if content.iter().any(|block| matches!(block, Content::Text { text, .. } if text == reply)))
    })
}

fn wake_text(session: &AgentSession) -> Result<String, Box<dyn Error>> {
    Ok(session
        .messages()
        .iter()
        .find_map(|message| match message {
            AgentMessage::Custom {
                custom_type,
                content: UserContent::Text(text),
                ..
            } if custom_type == "heartbeat_prompt" => Some(text.clone()),
            _ => None,
        })
        .ok_or("no heartbeat_prompt message in history")?)
}

#[tokio::test]
async fn a_cron_tick_creates_one_todo_with_its_intent_and_wakes_an_idle_session() -> TestResult {
    let (session, todos) = clock_session("tick handled")?;
    let (_dir, store) = temp_store("wake")?;
    let now = yi_session::now_ms();
    let mut job = heartbeat_job("hb-1", 10_000, now);
    job.schedule = CronSchedule {
        kind: ScheduleKind::Cron,
        expression: "* * * * *".to_owned(),
        interval_ms: None,
    };
    job.intent = vec!["user://1".parse()?];
    store.mutate(|state| state.jobs.push(job));

    let scheduler = start_scheduler(&store, session.heartbeat_deliverer());
    let ran = wait_until(5_000, || run_count(&store, "hb-1") == 1).await;
    session.wait_idle().await;
    scheduler.stop();

    assert!(ran, "the cron tick never ran: {:?}", store.snapshot().jobs);
    let job = store.snapshot().jobs.remove(0);
    assert!(
        job.next_run_at.is_some_and(|at| at > now),
        "the cron schedule must re-arm after a tick"
    );
    let list = todos.list();
    let items: Vec<&Todo> = list.items().collect();
    let [todo] = items.as_slice() else {
        return Err(format!("one tick, one todo: {items:?}").into());
    };
    assert!(
        todo.label.as_str().starts_with("check the build @ "),
        "{}",
        todo.label
    );
    assert_eq!(todo.cites.intent, vec!["user://1".parse()?]);
    assert_eq!(
        todo.note.as_ref().map(|note| note.as_str()),
        Some("check the build"),
        "the instruction rides the todo's note"
    );
    assert_eq!(
        todo.extra
            .get("clock")
            .and_then(|stamp| stamp.get("job"))
            .and_then(serde_json::Value::as_str),
        Some("hb-1")
    );
    let wake = wake_text(&session)?;
    assert!(
        !wake.contains("check the build"),
        "the job's text reached the model as a prompt: {wake}"
    );
    assert!(
        wake.contains("clock://* * * * *") && wake.contains("t1"),
        "the wake names the address and the todo: {wake}"
    );
    assert!(
        answered(&session, "tick handled"),
        "the idle session never woke"
    );
    assert!(
        store.snapshot().dispatches.is_empty(),
        "a delivered dispatch must be resolved, not leak"
    );
    Ok(())
}

/// Incident shape: `compact_now` summarizes while idle, a due steer heartbeat started a run,
/// and the summarized history then overwrote the turn it had added.
#[tokio::test]
async fn a_heartbeat_due_mid_compaction_is_deferred() -> TestResult {
    let provider = Arc::new(ProviderStream::new(None, None));
    provider.queue_faux(vec![
        faux_assistant_message(
            vec![faux_text(&format!("long body {}", "y".repeat(400)))],
            StopReason::Stop,
        ),
        faux_assistant_message(vec![faux_text("## Goal\nIdle summary")], StopReason::Stop),
    ]);
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );
    session.enable_compaction_with(yi_context::Settings {
        enabled: true,
        reserve_tokens: yi_context::Tokens(1_000),
        keep_recent_tokens: yi_context::Tokens(10),
    });
    session.prompt("please do the thing with sufficient text here")?;
    session.wait_idle().await;

    let deliver = session.heartbeat_deliverer();
    let compactor = session.compactor().ok_or("compaction is enabled")?;
    let outcome = Arc::new(Mutex::new(None));
    let seen = Arc::clone(&outcome);
    compactor.set_standing(Arc::new(move || {
        let job = heartbeat_job("hb-mid", 10_000, yi_session::now_ms());
        *seen.lock().ok()? = Some(deliver(&job, &Firing::at(job.next_run_at.unwrap_or(0))));
        None
    }));
    let applied = session.compact_now().await;
    let outcome = outcome.lock().map_err(|error| error.to_string())?.clone();
    assert_eq!(
        outcome,
        Some(Ok(RunOutcome::Skipped)),
        "a heartbeat due while the summarizer runs must wait for the next tick"
    );
    assert!(applied, "the idle compaction still lands");
    assert!(
        !compactor.compacting(),
        "the flag drops once compaction ends"
    );
    Ok(())
}

#[tokio::test]
async fn unresolved_claims_recover_as_interrupted_on_start() -> TestResult {
    let (dir, store) = temp_store("recover")?;
    let now = yi_session::now_ms();
    store.mutate(|state| {
        state.jobs.push(heartbeat_job("hb-crash", 10_000, now));
        state.dispatches.push(DispatchRecord {
            id: "dsp-crash".to_owned(),
            job_id: "hb-crash".to_owned(),
            claimed_at: now,
            scheduled_for: now,
            extra: serde_json::Map::new(),
        });
    });
    drop(store);

    let reopened = Arc::new(JobStore::open(dir.join("scheduled-jobs.json")));
    let deliver: Arc<DeliverFn> = Arc::new(|_, _| Ok(RunOutcome::Skipped));
    let scheduler = Scheduler::start(Arc::clone(&reopened), deliver, || "dsp-x".to_owned());
    scheduler.stop();

    let text = std::fs::read_to_string(dir.join("scheduled-jobs.json"))?;
    let state: ScheduleState = serde_json::from_str(&text)?;
    assert!(
        state.dispatches.is_empty(),
        "recovery must clear the orphaned claim from disk"
    );
    let job = state.jobs.first().ok_or("job missing")?;
    assert_eq!(
        job.last_error.as_deref(),
        Some(INTERRUPTED_ERROR),
        "an interrupted dispatch must be visible on the job"
    );
    Ok(())
}

#[tokio::test]
async fn heartbeat_surface_set_status_pause_clear_round_trip() -> TestResult {
    let (_dir, store) = temp_store("surface")?;
    let service = HeartbeatService::new(Arc::clone(&store), "/tmp");
    service.bind_session("test".to_owned());
    let now = yi_session::now_ms();

    let set = parse_heartbeat_command("/heartbeat every 10m run the tests")?;
    assert_eq!(
        set,
        HeartbeatCommand::Set {
            schedule: "every 10m".to_owned(),
            instruction: "run the tests".to_owned(),
            delivery_mode: None,
        }
    );
    let reply = service.apply(&set, now)?;
    assert!(reply.contains("Heartbeat set"), "{reply}");

    let replacement = parse_heartbeat_command("/heartbeat --every 30m --follow-up watch CI")?;
    service.apply(&replacement, now)?;
    let active: Vec<_> = store
        .snapshot()
        .jobs
        .into_iter()
        .filter(|job| job.status == JobStatus::Active)
        .collect();
    assert_eq!(
        active.len(),
        1,
        "setting a heartbeat must replace the previous one"
    );
    assert_eq!(
        active[0].delivery_mode,
        Some(yi_types::schedule::DeliveryMode::FollowUp)
    );
    assert_eq!(active[0].prompt, "watch CI");

    let status = service.apply(&HeartbeatCommand::Status, now)?;
    assert!(status.contains("watch CI"), "{status}");
    service.apply(&HeartbeatCommand::Pause, now)?;
    assert!(
        store
            .snapshot()
            .jobs
            .iter()
            .filter(|job| job.prompt == "watch CI")
            .all(|job| job.status == JobStatus::Paused),
        "pause must pause the active heartbeat"
    );
    service.apply(&HeartbeatCommand::Clear, now)?;
    let after_clear = service.apply(&HeartbeatCommand::Status, now)?;
    assert_eq!(after_clear, "No heartbeat is set.");

    assert_eq!(
        parse_heartbeat_command("/heartbeat").map_err(|e| e.to_string()),
        Ok(HeartbeatCommand::Status)
    );
    assert!(
        parse_heartbeat_command("/heartbeat --every 10m")
            .err()
            .is_some_and(|error| error.contains("Usage: /heartbeat")),
        "an instruction-less set must show usage"
    );
    let bad = parse_schedule("every 5s", now);
    assert_eq!(
        bad.err().as_deref(),
        Some("Recurring interval must be at least 10 seconds")
    );
    Ok(())
}

type Gate = Arc<(Mutex<bool>, Condvar)>;
type Entered = Arc<std::sync::atomic::AtomicBool>;

/// A deliver that parks every job in `session` until [`open_gate`] releases it;
/// the flag goes true once a lane is actually inside the callback.
fn gated_deliver(session: &'static str) -> (Arc<DeliverFn>, Gate, Entered) {
    let gate: Gate = Arc::new((Mutex::new(false), Condvar::new()));
    let entered: Entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let held = Arc::clone(&gate);
    let mark = Arc::clone(&entered);
    let deliver: Arc<DeliverFn> = Arc::new(move |job, _| {
        if job.session_id == session {
            let (lock, cvar) = &*held;
            let mut go = lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            mark.store(true, std::sync::atomic::Ordering::SeqCst);
            while !*go {
                go = cvar
                    .wait(go)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        }
        Ok(RunOutcome::Ran)
    });
    (deliver, gate, entered)
}

fn open_gate(gate: &Gate) {
    let (lock, cvar) = &**gate;
    *lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
    cvar.notify_all();
}

fn start_scheduler(store: &Arc<JobStore>, deliver: Arc<DeliverFn>) -> Scheduler {
    let mut counter = 0_u64;
    Scheduler::start(Arc::clone(store), deliver, move || {
        counter = counter.saturating_add(1);
        format!("dsp-{counter}")
    })
}

/// A submitted job reaches the timer over a channel, so a fixed sleep is either
/// flaky or slow; every wait here polls the state it actually depends on.
async fn wait_until(within_ms: u64, mut ready: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(within_ms);
    loop {
        if ready() {
            return true;
        }
        if std::time::Instant::now() > deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

fn run_count(store: &Arc<JobStore>, id: &str) -> u64 {
    store
        .snapshot()
        .jobs
        .iter()
        .find(|job| job.id == id)
        .map_or(0, |job| job.run_count)
}

fn lane_job(
    id: &str,
    session: &str,
    interval_ms: u64,
    next_run_at: u64,
) -> yi_types::schedule::Job {
    let mut job = heartbeat_job(id, interval_ms, next_run_at);
    job.session_id = session.to_owned();
    job
}

fn once_job(id: &str, session: &str, next_run_at: u64) -> yi_types::schedule::Job {
    let now = yi_session::now_ms();
    let mut job = new_job(JobSpec {
        id: id.to_owned(),
        session_id: session.to_owned(),
        cwd: "/tmp".to_owned(),
        source: JobSource::Heartbeat,
        delivery_mode: None,
        label: None,
        prompt: "check the build".to_owned(),
        schedule: yi_types::schedule::CronSchedule {
            kind: yi_types::schedule::ScheduleKind::Once,
            expression: "in 1s".to_owned(),
            interval_ms: None,
        },
        next_run_at: now,
        now_ms: now,
    });
    job.next_run_at = Some(next_run_at);
    job
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocked_session_does_not_stall_a_sibling_lane() -> TestResult {
    let (_dir, store) = temp_store("lanes")?;
    let now = yi_session::now_ms();
    store.mutate(|state| {
        state.jobs.push(lane_job("hb-a", "a", 10_000, now));
        state.jobs.push(lane_job("hb-b", "b", 10_000, now));
    });
    let (deliver, gate, _entered) = gated_deliver("a");
    let scheduler = start_scheduler(&store, deliver);

    let b_ran = wait_until(2_000, || run_count(&store, "hb-b") == 1).await;
    let a_runs_while_blocked = run_count(&store, "hb-a");
    open_gate(&gate);
    let a_ran = wait_until(2_000, || run_count(&store, "hb-a") == 1).await;
    scheduler.stop();

    assert!(
        b_ran,
        "session B's heartbeat never ran because session A's deliver slept"
    );
    assert_eq!(
        a_runs_while_blocked, 0,
        "session A must still be blocked when B has already run"
    );
    assert!(a_ran, "session A never ran after the block lifted");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn job_due_during_a_block_still_dispatches() -> TestResult {
    let (_dir, store) = temp_store("due-during")?;
    let now = yi_session::now_ms();
    store.mutate(|state| {
        state.jobs.push(lane_job("hb-a", "a", 10_000, now));
        state
            .jobs
            .push(lane_job("hb-b", "b", 10_000, now.saturating_add(500)));
    });
    let (deliver, gate, _entered) = gated_deliver("a");
    let scheduler = start_scheduler(&store, deliver);

    let b_ran = wait_until(2_000, || run_count(&store, "hb-b") == 1).await;
    let a_runs_while_blocked = run_count(&store, "hb-a");
    open_gate(&gate);
    // Incident: A's completion persists after the gate opens; returning first let it
    // re-create the scratch dir the test had already dropped.
    wait_until(2_000, || run_count(&store, "hb-a") >= 1).await;
    scheduler.stop();

    assert!(
        b_ran,
        "session B became due during A's block and never ran until the timer unblocked"
    );
    assert_eq!(
        a_runs_while_blocked, 0,
        "A must still be blocked when B has run"
    );
    Ok(())
}

/// A blocked session whose own interval re-arms mid-block makes every claim
/// come back empty; the timer must still be watching the sibling's deadline.
/// A 60s heartbeat parked behind a five-minute turn is this case.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rearming_block_does_not_hide_a_sibling_coming_due() -> TestResult {
    let (_dir, store) = temp_store("lanes-rearm")?;
    let now = yi_session::now_ms();
    store.mutate(|state| {
        state.jobs.push(lane_job("hb-a", "a", 200, now));
        state
            .jobs
            .push(lane_job("hb-b", "b", 10_000, now.saturating_add(500)));
    });
    let (deliver, gate, _entered) = gated_deliver("a");
    let scheduler = start_scheduler(&store, deliver);

    let b_ran = wait_until(3_000, || run_count(&store, "hb-b") == 1).await;
    let a_runs_while_blocked = run_count(&store, "hb-a");
    open_gate(&gate);
    wait_until(2_000, || run_count(&store, "hb-a") >= 1).await;
    scheduler.stop();

    assert!(
        b_ran,
        "A's interval re-armed while it blocked, and B never dispatched at its own deadline"
    );
    assert_eq!(
        a_runs_while_blocked, 0,
        "A must still be blocked when B has run"
    );
    Ok(())
}

/// The blocked session's only job is `Once`, so nothing is scheduled at all
/// while its lane runs. A job created mid-block must still dispatch rather
/// than wait the lane out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_job_scheduled_mid_block_dispatches_before_the_lane_ends() -> TestResult {
    let (_dir, store) = temp_store("lanes-midblock")?;
    let now = yi_session::now_ms();
    store.mutate(|state| state.jobs.push(once_job("hb-a", "a", now)));
    let (deliver, gate, entered) = gated_deliver("a");
    let scheduler = start_scheduler(&store, deliver);

    let lane_running =
        wait_until(2_000, || entered.load(std::sync::atomic::Ordering::SeqCst)).await;
    // The park itself has no observable state: the timer reaches it a few
    // instructions after the lane it just spawned enters `deliver`.
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    store.mutate(|state| {
        state
            .jobs
            .push(lane_job("hb-b", "b", 10_000, yi_session::now_ms()));
    });
    let b_ran = wait_until(3_000, || run_count(&store, "hb-b") == 1).await;
    open_gate(&gate);
    wait_until(2_000, || run_count(&store, "hb-a") >= 1).await;
    scheduler.stop();

    assert!(lane_running, "session A's lane never entered deliver");
    assert!(
        b_ran,
        "a job scheduled while A's lane blocked never dispatched"
    );
    Ok(())
}

#[test]
fn setting_a_heartbeat_does_not_cancel_a_sibling_session() -> TestResult {
    let (_dir, store) = temp_store("sibling-hb")?;
    let a = HeartbeatService::new(Arc::clone(&store), "/tmp");
    a.bind_session("sess-a".to_owned());
    let b = HeartbeatService::new(Arc::clone(&store), "/tmp");
    b.bind_session("sess-b".to_owned());
    let now = yi_session::now_ms();
    a.apply(
        &parse_heartbeat_command("/heartbeat every 10m watch A")?,
        now,
    )?;
    b.apply(
        &parse_heartbeat_command("/heartbeat every 10m watch B")?,
        now,
    )?;
    let active: Vec<_> = store
        .snapshot()
        .jobs
        .into_iter()
        .filter(|job| job.status == JobStatus::Active)
        .collect();
    assert_eq!(
        active.len(),
        2,
        "two sessions on one store must keep both heartbeats: {active:?}"
    );
    Ok(())
}

async fn host_call(
    host: &HostRegistry,
    request_type: &str,
    payload: serde_json::Value,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let payload = payload.as_object().cloned().unwrap_or_default();
    let call = HostHandlers::dispatch(host, request_type, payload)
        .ok_or_else(|| format!("{request_type} is not registered"))?;
    call.await
}

fn bound(store: &Arc<JobStore>, session_id: &str) -> Arc<HeartbeatService> {
    let service = Arc::new(HeartbeatService::new(Arc::clone(store), "/tmp"));
    service.bind_session(session_id.to_owned());
    service
}

#[tokio::test]
async fn the_kernel_vocabulary_cannot_reach_a_sibling_session() -> TestResult {
    let (_dir, store) = temp_store("rlm-scope")?;
    let (a, b) = (bound(&store, "sess-a"), bound(&store, "sess-b"));
    let (mut a_host, mut b_host) = (HostRegistry::default(), HostRegistry::default());
    a.register(&mut a_host);
    b.register(&mut b_host);

    let created = host_call(
        &b_host,
        "rlm_heartbeat.create",
        serde_json::json!({"schedule": "10m", "prompt": "watch B"}),
    )
    .await?;
    let b_id = created
        .get("job")
        .and_then(|job| job.get("id"))
        .and_then(serde_json::Value::as_str)
        .ok_or("the created job carries no id")?
        .to_owned();

    let listed = host_call(&a_host, "rlm_heartbeat.list", serde_json::json!({})).await?;
    let jobs = listed
        .get("jobs")
        .and_then(serde_json::Value::as_array)
        .ok_or("the list reply carries no jobs")?;
    assert!(
        jobs.is_empty(),
        "session a listed a sibling session's heartbeat: {jobs:?}"
    );

    let paused = host_call(
        &a_host,
        "rlm_heartbeat.update",
        serde_json::json!({"id": &b_id, "status": "pause"}),
    )
    .await;
    assert!(
        paused.is_err(),
        "session a paused a sibling session's heartbeat: {paused:?}"
    );

    let refused = host_call(
        &a_host,
        "rlm_heartbeat.delete",
        serde_json::json!({"id": &b_id}),
    )
    .await;
    assert!(
        refused.is_err(),
        "session a deleted a sibling session's heartbeat: {refused:?}"
    );
    assert_eq!(
        store
            .snapshot()
            .jobs
            .iter()
            .find(|job| job.id == b_id)
            .map(|job| job.status),
        Some(JobStatus::Active),
        "a sibling session's heartbeat was cancelled from another session"
    );
    Ok(())
}

fn detached_todos() -> Arc<TodoStore> {
    TodoStore::new(Arc::new(|| None), "main")
}

fn created(fired: &Fired) -> usize {
    match fired {
        Fired::Created(ids) => ids.len(),
        Fired::Unblocked(_) | Fired::Held(_) => 0,
    }
}

#[test]
fn overlap_skip_creates_nothing_while_the_last_ticks_todo_is_open() -> TestResult {
    let todos = detached_todos();
    let job = heartbeat_job("hb-o", 10_000, 0);
    let nine = parse_iso_ms("2026-09-28T09:00:00Z").ok_or("iso")?;
    assert_eq!(created(&clock::fire(&todos, &job, &Firing::at(nine))?), 1);

    let second = clock::fire(&todos, &job, &Firing::at(nine + 10_000))?;
    assert_eq!(
        created(&second),
        0,
        "a tick with its predecessor still open created a todo: {second:?}"
    );
    assert_eq!(todos.list().items().count(), 1);

    let first = todos.list().items().next().ok_or("no todo")?.label.clone();
    todos.apply(
        Op::Drop {
            target: Target::Label(first),
            reason: "handled by hand".to_owned(),
        },
        None,
    )?;
    let third = clock::fire(&todos, &job, &Firing::at(nine + 20_000))?;
    assert_eq!(
        created(&third),
        1,
        "a closed todo still held the next tick back"
    );

    let mut buffered = job.clone();
    buffered.overlap = Some(Overlap::BufferOne);
    assert_eq!(
        created(&clock::fire(&todos, &buffered, &Firing::at(nine + 30_000))?),
        1
    );
    assert_eq!(
        created(&clock::fire(&todos, &buffered, &Firing::at(nine + 40_000))?),
        0,
        "buffer_one kept a third todo open"
    );
    Ok(())
}

#[test]
fn three_missed_ticks_make_one_todo_that_names_them() -> TestResult {
    let seven = parse_iso_ms("2026-09-28T07:00:00Z").ok_or("iso")?;
    let hour = 3_600_000;
    let mut state = ScheduleState::default();
    let mut job = heartbeat_job("hb-m", hour, seven);
    job.schedule = parse_schedule("every 1h", seven)?.0;
    state.jobs.push(job);
    let woke = seven + 2 * hour + 5 * 60_000;
    let claimed = claim_due_in_state(
        &mut state,
        woke,
        woke,
        || "dsp-1".to_owned(),
        &HashSet::new(),
    );
    let [dispatch] = claimed.as_slice() else {
        return Err("one due job, one claim".into());
    };
    assert_eq!(
        dispatch.firing.total, 3,
        "07:00, 08:00 and 09:00 all came due asleep"
    );

    let todos = detached_todos();
    assert_eq!(
        created(&clock::fire(&todos, &dispatch.job, &dispatch.firing)?),
        1
    );
    let list = todos.list();
    let todo = list.items().next().ok_or("no todo")?;
    let note = todo
        .note
        .as_ref()
        .map(|note| note.as_str())
        .unwrap_or_default();
    assert!(
        note.contains("3 ticks missed while asleep, 2026-09-28T07:00:00Z–2026-09-28T09:00:00Z"),
        "{note}"
    );
    assert!(
        todo.label.as_str().ends_with("@ 2026-09-28T09:00:00Z"),
        "{}",
        todo.label
    );

    let mut skipping = dispatch.job.clone();
    skipping.id = "hb-skip".to_owned();
    skipping.catch_up = Some(CatchUp::Skip);
    let held = clock::fire(&detached_todos(), &skipping, &dispatch.firing)?;
    assert_eq!(
        created(&held),
        0,
        "catch_up skip made a todo for missed ticks"
    );
    Ok(())
}

fn iso(ms: u64) -> String {
    let seconds = ms / 1_000;
    let (year, month, day) = yi_kernel::client::civil_from_days(seconds / 86_400);
    let (hour, minute, second) = (
        (seconds % 86_400) / 3_600,
        (seconds % 3_600) / 60,
        seconds % 60,
    );
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn blocked(todos: &TodoStore) -> bool {
    todos
        .list()
        .items()
        .any(|item| matches!(item.state, TodoState::Blocked { .. }))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_todo_waiting_on_a_clock_time_unblocks_then_across_a_restart() -> TestResult {
    let (session, todos) = clock_session("woke at the time")?;
    let (dir, store) = temp_store("wait")?;
    let service = HeartbeatService::new(Arc::clone(&store), "/tmp");
    service.bind_session("test".to_owned());
    let todo = Todo::from_text("ship at the time")?;
    let label = todo.label.clone();
    todos.apply(
        Op::Append {
            phase: None,
            under: None,
            items: vec![todo],
        },
        None,
    )?;
    let at = (yi_session::now_ms() / 1_000 + 3) * 1_000;
    todos.apply(
        Op::Block {
            label: label.clone(),
            on: BlockedOn::Channel {
                address: format!("clock://at {}", iso(at)),
                filter: None,
            },
            note: "not before then".to_owned(),
            ask: None,
        },
        None,
    )?;
    service.watch(&todos.list());
    let armed = |store: &JobStore| {
        store
            .snapshot()
            .jobs
            .iter()
            .filter(|job| job.unblocks.as_ref() == Some(&label))
            .map(|job| job.next_run_at)
            .collect::<Vec<_>>()
    };
    assert_eq!(armed(&store), vec![Some(at)], "the wait armed no timer");
    service.watch(&todos.list());
    assert_eq!(armed(&store).len(), 1, "a second look armed a second timer");

    let lost = Scratch::new("yi-sched-wait-lost")?;
    let fresh = Arc::new(JobStore::open(lost.join("scheduled-jobs.json")));
    let rehydrated = HeartbeatService::new(Arc::clone(&fresh), "/tmp");
    rehydrated.bind_session("test".to_owned());
    rehydrated.watch(&todos.list());
    assert_eq!(
        armed(&fresh),
        vec![Some(at)],
        "a host that lost its store did not re-arm the wait from the list"
    );

    let first = start_scheduler(&store, session.heartbeat_deliverer());
    first.stop();
    drop(first);
    assert!(blocked(&todos), "the wait ended before its time");

    let reopened = Arc::new(JobStore::open(dir.join("scheduled-jobs.json")));
    let second = start_scheduler(&reopened, session.heartbeat_deliverer());
    let unblocked = wait_until(8_000, || !blocked(&todos)).await;
    session.wait_idle().await;
    second.stop();

    assert!(unblocked, "the restarted timer never unblocked the todo");
    assert!(
        yi_session::now_ms() >= at,
        "the todo unblocked before its time"
    );
    assert!(
        reopened
            .snapshot()
            .jobs
            .iter()
            .all(|job| job.status == JobStatus::Completed),
        "a spent wait stayed armed"
    );
    assert!(
        answered(&session, "woke at the time"),
        "the unblock never woke the session"
    );
    Ok(())
}

#[tokio::test]
async fn a_scheduled_jobs_file_from_the_prompt_era_fires_as_a_todo() -> TestResult {
    let (session, todos) = clock_session("fixture tick handled")?;
    let dir = Scratch::new("yi-sched-fixture")?;
    let path = dir.join("scheduled-jobs.json");
    std::fs::write(
        &path,
        include_str!("../../types/tests/fixtures/scheduled-jobs-v1.json"),
    )?;
    let store = Arc::new(JobStore::open(path));
    assert_eq!(
        store.snapshot().jobs.len(),
        1,
        "the recorded store did not load"
    );

    let scheduler = start_scheduler(&store, session.heartbeat_deliverer());
    let ran = wait_until(5_000, || run_count(&store, "hb-40f42121") == 2).await;
    session.wait_idle().await;
    scheduler.stop();

    assert!(ran, "the recorded job never fired");
    let list = todos.list();
    let items: Vec<&Todo> = list.items().collect();
    let [todo] = items.as_slice() else {
        return Err(format!("the recorded job made {} todos", items.len()).into());
    };
    assert!(
        todo.label
            .as_str()
            .starts_with("check the build and report red jobs @ "),
        "{}",
        todo.label
    );
    let note = todo
        .note
        .as_ref()
        .map(|note| note.as_str())
        .unwrap_or_default();
    assert!(
        note.starts_with("check the build and report red jobs\n")
            && note.contains("ticks missed while asleep, 2026-09-28T00:45:13Z–"),
        "the ticks since the recording are one todo that counts them: {note}"
    );
    assert!(!wake_text(&session)?.contains("report red jobs"));
    assert!(answered(&session, "fixture tick handled"));
    Ok(())
}

fn turn_of(output: i64) -> yi_types::event::AgentEvent {
    let mut message = faux_assistant_message(Vec::new(), StopReason::Stop);
    if let AgentMessage::Assistant { usage, .. } = &mut message {
        usage.output = output;
    }
    yi_types::event::AgentEvent::MessageEnd { message }
}

fn child_at(tokens: u64) -> yi_types::event::AgentEvent {
    yi_types::event::AgentEvent::ChildUpdate {
        update: yi_types::subagent::ChildUpdate {
            id: yi_types::subagent::ChildId("c1".to_owned()),
            name: "scout".to_owned(),
            status: yi_types::subagent::ChildStatus::Running,
            activity: yi_types::subagent::ChildActivity::Executing,
            tool_use_count: 1,
            token_count: tokens,
            answer_preview: None,
            error: None,
            exit: None,
            flag: None,
        },
    }
}

#[test]
fn a_spend_alert_fires_once_per_crossing() -> TestResult {
    let every = std::num::NonZeroU64::new(1_000).ok_or("zero")?;
    let mut alarm = yi_runtime::spend::SpendAlarm::new(every);
    assert_eq!(alarm.observe(&turn_of(600)), None);
    let crossed = alarm
        .observe(&turn_of(500))
        .ok_or("1100 tokens crossed 1000 and nothing fired")?;
    assert!(
        crossed.contains("used 1100 tokens, past the alert at 1000")
            && crossed.contains("the next alert is at 2000"),
        "{crossed}"
    );
    assert_eq!(
        alarm.observe(&turn_of(300)),
        None,
        "the alert fired again below the next crossing"
    );
    assert!(
        alarm
            .observe(&child_at(700))
            .is_some_and(|text| text.contains("used 2100 tokens")),
        "a child's tokens count toward the crossing"
    );
    assert_eq!(
        alarm.observe(&child_at(1_500)),
        None,
        "a child's newer count replaced its last one, it did not add to it"
    );
    Ok(())
}

async fn until(mut ready: impl FnMut() -> bool) -> bool {
    wait_until(5_000, &mut ready).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_kill_switch_holds_the_clock_and_a_turn_until_resume() -> TestResult {
    let dir = Scratch::new("yi-halt")?;
    let started = dir.join("started");
    let provider = Arc::new(ProviderStream::new(None, None));
    let mut args = serde_json::Map::new();
    args.insert(
        "command".to_owned(),
        serde_json::json!("echo up > started; sleep 5"),
    );
    provider.queue_faux(vec![
        faux_assistant_message(
            vec![yi_ai::faux::faux_tool_call("call-1", "bash", args)],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("resumed")], StopReason::Stop),
    ]);
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );
    let broker = Arc::new(yi_runtime::PermissionBroker::new(
        yi_runtime::PermissionMode::Yolo,
        dir.to_path_buf(),
        Vec::new(),
        None,
        session.events_sender(),
    ));
    session.use_tools(yi_tools::builtin_tools(), dir.to_path_buf(), Some(broker));
    let ledger = memory_store("s");
    session.attach_store(Arc::clone(&ledger))?;
    let store = Arc::new(JobStore::open(dir.join("scheduled-jobs.json")));
    let service = HeartbeatService::new(Arc::clone(&store), "/tmp")
        .with_stop(session.halt_hook())
        .with_words(session.store_handle());
    service.bind_session("s".to_owned());
    service.run("/heartbeat every 10m watch CI")?;
    session.prompt("run it")?;
    assert!(
        until(|| started.exists()).await,
        "the turn never reached its tool"
    );

    let halted = service.run("/heartbeat halt")?;
    assert!(halted.contains("1 running turn(s) interrupted"), "{halted}");
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), session.wait_idle())
            .await
            .is_ok(),
        "the halt left the turn running"
    );
    assert!(
        store.snapshot().jobs.iter().all(|job| job.halted),
        "a clock job stayed live through the halt"
    );
    (session.deliver_hook())(yi_runtime::session::user_input("wake up"), true);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(
        !answered(&session, "resumed"),
        "a machine wake started a turn through the halt"
    );

    let resumed = service.run("/heartbeat resume")?;
    assert!(resumed.contains("The halt is lifted"), "{resumed}");
    assert!(store.snapshot().jobs.iter().all(|job| !job.halted));
    assert!(
        until(|| answered(&session, "resumed")).await,
        "the wake the halt held never ran after resume"
    );
    let records = yi_session::lock_session(&ledger).find_entries(&yi_session::EntryQuery {
        custom_type: Some(yi_types::schedule::HALT_ENTRY_TYPE.to_owned()),
        ..yi_session::EntryQuery::default()
    })?;
    assert_eq!(
        records.len(),
        2,
        "the halt and its undo are both in the ledger"
    );
    Ok(())
}
