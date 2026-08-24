use std::error::Error;
use std::sync::Arc;

use yi_ai::faux::{faux_assistant_message, faux_text};
use yi_loop::ExecutionMode;
use yi_runtime::schedule::{
    DEFAULT_HEARTBEAT_DELIVERY_MODE, DeliverFn, HeartbeatCommand, HeartbeatService,
    INTERRUPTED_ERROR, JobSpec, JobStore, RunOutcome, Scheduler, SessionActivity,
    heartbeat_message, new_job, parse_heartbeat_command, parse_schedule, should_defer,
};
use yi_runtime::{AgentSession, ProviderStream, SessionConfig};
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

fn temp_store(name: &str) -> (std::path::PathBuf, Arc<JobStore>) {
    let dir = std::env::temp_dir().join(format!("yi-sched-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("scheduled-jobs.json");
    (dir, Arc::new(JobStore::open(path)))
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

    let (dir, store) = temp_store("wake");
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
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[tokio::test]
async fn unresolved_claims_recover_as_interrupted_on_start() -> TestResult {
    let (dir, store) = temp_store("recover");
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
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[tokio::test]
async fn heartbeat_surface_set_status_pause_clear_round_trip() -> TestResult {
    let (dir, store) = temp_store("surface");
    let service = HeartbeatService {
        store: Arc::clone(&store),
        session_id: "test".to_owned(),
        cwd: "/tmp".to_owned(),
    };
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
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
