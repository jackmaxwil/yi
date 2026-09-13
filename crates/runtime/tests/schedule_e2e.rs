#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

use std::error::Error;
use std::sync::{Arc, Condvar, Mutex};

use yi_ai::faux::{faux_assistant_message, faux_text};
use yi_kernel::client::HostHandlers;
use yi_loop::ExecutionMode;
use yi_runtime::schedule::{
    DEFAULT_HEARTBEAT_DELIVERY_MODE, DeliverFn, HeartbeatCommand, HeartbeatService,
    INTERRUPTED_ERROR, JobSpec, JobStore, RunOutcome, Scheduler, SessionActivity,
    heartbeat_message, new_job, parse_heartbeat_command, parse_schedule, should_defer,
};
use yi_runtime::{AgentSession, HostRegistry, ProviderStream, SessionConfig};
use yi_types::message::{AgentMessage, StopReason, UserContent};
use yi_types::model::{Model, ModelCost};
use yi_types::schedule::{DispatchRecord, JobSource, JobStatus, ScheduleState};

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

#[tokio::test]
async fn due_heartbeat_wakes_an_idle_session_and_advances_the_job() -> TestResult {
    let provider = Arc::new(ProviderStream::new(None, None));
    provider.queue_faux(vec![faux_assistant_message(
        vec![faux_text("heartbeat handled")],
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

    let (_dir, store) = temp_store("wake")?;
    let now = yi_session::now_ms();
    store.mutate(|state| state.jobs.push(heartbeat_job("hb-1", 10_000, now)));

    let hook = session.heartbeat_hook();
    let busy = session.activity_handle();
    let deliver: Arc<DeliverFn> = Arc::new(move |job| {
        let activity = SessionActivity {
            is_streaming: busy(),
            ..SessionActivity::default()
        };
        if should_defer(job, &activity) {
            return RunOutcome::Skipped;
        }
        hook(
            heartbeat_message(job, yi_session::now_ms()),
            job.delivery_mode.unwrap_or(DEFAULT_HEARTBEAT_DELIVERY_MODE),
        );
        RunOutcome::Ran
    });
    let mut counter = 0_u64;
    let scheduler = Scheduler::start(Arc::clone(&store), deliver, move || {
        counter += 1;
        format!("dsp-{counter}")
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let job = store
            .snapshot()
            .jobs
            .first()
            .cloned()
            .ok_or("job missing")?;
        if job.run_count == 1 {
            assert!(
                job.next_run_at.is_some_and(|at| at > now),
                "the interval schedule must re-arm after a run"
            );
            break;
        }
        if std::time::Instant::now() > deadline {
            return Err(format!("heartbeat never ran: {job:?}").into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    session.wait_idle().await;
    let messages = session.messages();
    let heartbeat = messages
        .iter()
        .find_map(|message| match message {
            AgentMessage::Custom {
                custom_type,
                content: UserContent::Text(text),
                ..
            } if custom_type == "heartbeat_prompt" => Some(text.clone()),
            _ => None,
        })
        .ok_or("no heartbeat_prompt message in history")?;
    assert!(
        heartbeat.contains("<heartbeat job=") && heartbeat.contains("check the build"),
        "the model must see the framed heartbeat: {heartbeat}"
    );
    assert!(
        store.snapshot().dispatches.is_empty(),
        "a delivered dispatch must be resolved, not leak"
    );
    scheduler.stop();
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
    let deliver: Arc<DeliverFn> = Arc::new(|_| RunOutcome::Skipped);
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
    let deliver: Arc<DeliverFn> = Arc::new(move |job| {
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
        RunOutcome::Ran
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
