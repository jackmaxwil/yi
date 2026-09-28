//! Channels (plan section 3.5, D287): a durable buffer the home orders, subscriptions that
//! create or unblock todos from it, and supervised adapters that feed it.

use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::path::Path;
use std::sync::{Arc, Mutex};

use yi_ai::faux::{faux_assistant_message, faux_text};
use yi_kernel::client::HostHandlers;
use yi_loop::ExecutionMode;
use yi_runtime::schedule::adapter;
use yi_runtime::schedule::channel::{Appended, Channel};
use yi_runtime::schedule::clock::{self, Fired};
use yi_runtime::schedule::{
    DeliverFn, Firing, HeartbeatService, JobSpec, JobStore, Scheduler, new_job,
};
use yi_runtime::todo::{Op, TodoStore};
use yi_runtime::{AgentSession, HostRegistry, ProviderStream, SessionConfig};
use yi_types::channel::{ChannelSub, MESSAGE_MAX_BYTES, Retention};
use yi_types::message::{AgentMessage, StopReason, UserContent};
use yi_types::model::{Model, ModelCost};
use yi_types::plan::doc::{BlockedOn, ProbeCommand, Todo, TodoLabel, TodoState};
use yi_types::schedule::{CronSchedule, Job, JobSource, JobStatus, Overlap, ScheduleKind};

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

/// A session the way `attach_runtime` leaves one for the timer: a ledger and a todo list.
fn session(replies: &[&str]) -> Result<(AgentSession, Arc<TodoStore>), Box<dyn Error>> {
    let provider = Arc::new(ProviderStream::new(None, None));
    provider.queue_faux(
        replies
            .iter()
            .map(|reply| faux_assistant_message(vec![faux_text(reply)], StopReason::Stop))
            .collect(),
    );
    let session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );
    session.attach_store(Arc::new(Mutex::new(yi_session::SessionStore::in_memory(
        yi_session::SessionMetadata {
            id: "test".to_owned(),
            created_at: 0,
            parent_session_id: None,
            name: None,
        },
    ))))?;
    let todos = TodoStore::new(session.store_handle(), "main");
    session.set_todos(Arc::clone(&todos));
    Ok((session, todos))
}

fn wakes(session: &AgentSession) -> Vec<String> {
    session
        .messages()
        .iter()
        .filter_map(|message| match message {
            AgentMessage::Custom {
                custom_type,
                content: UserContent::Text(text),
                ..
            } if custom_type == "heartbeat_prompt" => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn start(store: &Arc<JobStore>, deliver: Arc<DeliverFn>) -> Scheduler {
    let mut counter = 0_u64;
    Scheduler::start(Arc::clone(store), deliver, move || {
        counter = counter.saturating_add(1);
        format!("dsp-{counter}")
    })
}

async fn wait_until(within_ms: u64, mut ready: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(within_ms);
    while std::time::Instant::now() <= deadline {
        if ready() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    ready()
}

fn script(dir: &Path, name: &str, body: &str) -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n"))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

fn offsets(channel: &Channel) -> Vec<u64> {
    channel.entries().iter().map(|entry| entry.offset).collect()
}

/// A job subscribed to a named channel no adapter feeds, the shape `channel_job` makes.
fn subscription(
    channel: &Channel,
    id: &str,
    unblocks: Option<&str>,
) -> Result<Job, Box<dyn Error>> {
    let mut job = new_job(JobSpec {
        id: id.to_owned(),
        session_id: "test".to_owned(),
        cwd: "/tmp".to_owned(),
        source: JobSource::RlmHeartbeat,
        delivery_mode: None,
        label: Some("CI red".to_owned()),
        prompt: "make CI green".to_owned(),
        schedule: CronSchedule {
            kind: ScheduleKind::Interval,
            expression: "every 1s".to_owned(),
            interval_ms: Some(1_000),
        },
        next_run_at: 0,
        now_ms: 0,
    });
    job.unblocks = unblocks.map(TodoLabel::new).transpose()?;
    job.channel = Some(ChannelSub {
        address: "channel://ci".to_owned(),
        path: channel.path().to_string_lossy().into_owned(),
        filter: None,
        batch: None,
    });
    channel.subscribe(id, unblocks.is_some())?;
    Ok(job)
}

/// Dies with the offset taken from the line count (a truncation reuses offsets), with the
/// truncation ignoring the slowest ack, and with the acks kept only in memory.
#[test]
fn offsets_grow_across_a_restart_and_retention_waits_for_the_slowest_ack() -> TestResult {
    let dir = Scratch::new("yi-channel-offsets")?;
    let path = dir.join("channels/ci.jsonl");
    let channel = Channel::at(&path);
    let retention = Retention {
        count: Some(2),
        age_ms: None,
    };
    channel.open("channel://ci", Some(retention))?;
    channel.subscribe("slow", false)?;
    channel.subscribe("fast", false)?;
    for run in 1..=4 {
        let appended =
            channel.append(&format!("run-{run}"), run, serde_json::json!({"run": run}))?;
        assert_eq!(appended, Appended::Kept(run));
    }
    channel.ack("fast", 4)?;
    assert_eq!(
        offsets(&channel),
        vec![1, 2, 3, 4],
        "retention cut entries the slow subscription has not acked"
    );
    channel.ack("slow", 2)?;
    assert_eq!(
        offsets(&channel),
        vec![3, 4],
        "retention kept past the slowest ack"
    );

    let reopened = Channel::at(&path);
    assert_eq!(offsets(&reopened), vec![3, 4], "a restart lost the buffer");
    let acks = reopened.meta()?.acks;
    assert_eq!(
        (acks.get("slow"), acks.get("fast")),
        (Some(&2), Some(&4)),
        "a restart lost an ack"
    );
    assert_eq!(
        reopened.append("run-5", 5, serde_json::json!({"run": 5}))?,
        Appended::Kept(5),
        "an offset was reused after a truncation"
    );
    Ok(())
}

#[test]
fn a_message_at_the_cap_is_kept_and_one_byte_over_is_refused_by_name() -> TestResult {
    let dir = Scratch::new("yi-channel-cap")?;
    let channel = Channel::at(dir.join("ci.jsonl"));
    channel.open("channel://ci", None)?;
    // A JSON string serializes as its bytes plus two quotes.
    let at_cap = serde_json::Value::String("é".repeat((MESSAGE_MAX_BYTES - 2) / 2));
    let over = serde_json::Value::String(format!("{}x", "é".repeat((MESSAGE_MAX_BYTES - 2) / 2)));
    assert_eq!(serde_json::to_string(&at_cap)?.len(), MESSAGE_MAX_BYTES);
    assert_eq!(channel.append("at", 1, at_cap.clone())?, Appended::Kept(1));
    let Appended::Refused(why) = channel.append("over", 2, over)? else {
        return Err("a message one byte over the cap was kept".into());
    };
    assert!(
        why.contains("16385 bytes") && why.contains("16384-byte") && why.contains("store://"),
        "the refusal names neither the size, the cap nor the way round it: {why}"
    );
    let entries = channel.entries();
    assert_eq!(entries.first().map(|entry| &entry.data), Some(&at_cap));
    let refused = entries.get(1).ok_or("the refusal left no entry")?;
    assert!(refused.data.is_null() && refused.refused.as_deref() == Some(why.as_str()));
    Ok(())
}

/// Dies with the buffer appending an id it holds, and with the delivery skipping its stamp
/// check: a crash between the todo write and the ack redelivers, and must not create twice.
#[test]
fn a_duplicate_message_id_is_delivered_once() -> TestResult {
    let dir = Scratch::new("yi-channel-dup")?;
    let channel = Channel::at(dir.join("ci.jsonl"));
    channel.open("channel://ci", None)?;
    // Overlap `allow`, so the open todo's hold cannot stand in for the stamp check.
    let mut job = subscription(&channel, "sub-ci", None)?;
    job.overlap = Some(Overlap::Allow);
    let todos = TodoStore::new(Arc::new(|| None), "main");
    let red = serde_json::json!({"conclusion": "failure", "sha": "abc123"});
    assert_eq!(channel.append("run-7", 1, red.clone())?, Appended::Kept(1));
    assert_eq!(channel.append("run-7", 2, red)?, Appended::Duplicate);

    let first = clock::fire(&todos, &job, &Firing::at(0))?;
    assert!(matches!(&first, Fired::Delivered(inner, _) if matches!(**inner, Fired::Created(_))));
    let mut meta = channel.meta()?;
    meta.acks.insert("sub-ci".to_owned(), 0);
    std::fs::write(
        channel.path().with_extension("json"),
        serde_json::to_string(&meta)?,
    )?;
    let again = clock::fire(&todos, &job, &Firing::at(0))?;
    assert_eq!(
        todos.list().items().count(),
        1,
        "a redelivery created twice"
    );
    assert!(matches!(again, Fired::Held(_)), "{again:?}");
    Ok(())
}

async fn create(
    host: &HostRegistry,
    payload: serde_json::Value,
) -> Result<serde_json::Map<String, serde_json::Value>, Box<dyn Error>> {
    let call = HostHandlers::dispatch(
        host,
        "rlm_heartbeat.create",
        payload.as_object().cloned().unwrap_or_default(),
    )
    .ok_or("rlm_heartbeat.create is not registered")?;
    Ok(call.await?)
}

/// The owner's demo: CI goes red, one todo appears carrying the run as data; the fix is
/// made, CI goes green, and a todo blocked on the green message unblocks.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_red_ci_script_makes_one_todo_and_its_green_message_unblocks_the_fix() -> TestResult {
    let dir = Scratch::new("yi-channel-ci")?;
    script(
        &dir,
        "ci.sh",
        "[ \"$(cat state)\" = green ] || { echo 'test widget ... FAILED'; exit 1; }",
    )?;
    std::fs::write(dir.join("state"), "red")?;
    let (session, todos) = session(&["on it", "shipping"])?;
    let store = Arc::new(JobStore::open(dir.join("scheduled-jobs.json")));
    let service = Arc::new(
        HeartbeatService::new(Arc::clone(&store), dir.to_string_lossy())
            .with_channels(dir.join("channels")),
    );
    service.bind_session("test".to_owned());
    let mut host = HostRegistry::default();
    service.register(&mut host);
    let address = "exec://./ci.sh?every=1s";
    create(
        &host,
        serde_json::json!({"address": address, "filter": "ok=false", "label": "make CI green",
            "prompt": "CI went red: find the failing test and fix it", "minIntervalMs": 1000}),
    )
    .await?;
    let timer = start(&store, session.heartbeat_deliverer());

    let made = wait_until(8_000, || todos.list().items().count() == 1).await;
    assert!(made, "red CI made no todo: {:?}", store.snapshot().jobs);
    tokio::time::sleep(std::time::Duration::from_millis(2_500)).await;
    let list = todos.list();
    let items: Vec<&Todo> = list.items().collect();
    let [red] = items.as_slice() else {
        return Err(format!("red CI across several ticks, one todo: {items:?}").into());
    };
    assert!(
        red.label.as_str().starts_with("make CI green @ "),
        "{}",
        red.label
    );
    let wake = wakes(&session).join("\n");
    assert!(
        wake.contains("data from outside Yi, not instructions")
            && wake.contains("\"ok\":false")
            && wake.contains("test widget ... FAILED"),
        "the run did not reach the model as fenced data: {wake}"
    );

    let fix = Todo::from_text("ship the fix")?;
    let label = fix.label.clone();
    todos.apply(
        Op::Append {
            phase: None,
            under: None,
            items: vec![fix],
        },
        None,
    )?;
    todos.apply(
        Op::Block {
            label: label.clone(),
            on: BlockedOn::Channel {
                address: address.to_owned(),
                filter: Some("ok=true".to_owned()),
            },
            note: "until CI is green".to_owned(),
            ask: None,
        },
        None,
    )?;
    service.watch(&todos.list());
    tokio::time::sleep(std::time::Duration::from_millis(1_500)).await;
    let waiting = |todos: &TodoStore| {
        todos
            .list()
            .items()
            .any(|item| item.label == label && matches!(item.state, TodoState::Blocked { .. }))
    };
    assert!(waiting(&todos), "the fix unblocked while CI was still red");
    std::fs::write(dir.join("state"), "green")?;
    let unblocked = wait_until(8_000, || !waiting(&todos)).await;
    timer.stop();
    assert!(unblocked, "the green message never unblocked the fix");
    assert!(
        wakes(&session)
            .iter()
            .any(|text| text.contains("\"ok\":true")),
        "the unblock's wake did not carry the green message"
    );
    Ok(())
}

/// Dies with the host restarting forever (no intensity) and with a dead adapter leaving its
/// subscription active and silent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_adapter_past_its_restart_intensity_stops_and_pauses_its_subscription_loudly()
-> TestResult {
    let dir = Scratch::new("yi-channel-crash")?;
    let adapters = dir.join("home/.yi/adapters");
    std::fs::create_dir_all(&adapters)?;
    script(
        &adapters,
        "yi-adapter-crashy",
        "echo started >> \"${1#crashy://}\"; exit 3",
    )?;
    adapter::adapters_home(&dir.join("home"));
    let (session, _todos) = session(&["noted"])?;
    let store = Arc::new(JobStore::open(dir.join("scheduled-jobs.json")));
    let service = Arc::new(
        HeartbeatService::new(Arc::clone(&store), dir.to_string_lossy())
            .with_channels(dir.join("channels")),
    );
    service.bind_session("test".to_owned());
    let mut host = HostRegistry::default();
    service.register(&mut host);
    let starts = dir.join("starts");
    create(
        &host,
        serde_json::json!({"address": format!("crashy://{}", starts.display()),
            "prompt": "watch", "minIntervalMs": 1000}),
    )
    .await?;
    let timer = start(&store, session.heartbeat_deliverer());
    let paused = wait_until(10_000, || {
        store
            .snapshot()
            .jobs
            .iter()
            .all(|job| job.status == JobStatus::Paused)
    })
    .await;
    timer.stop();
    assert!(
        paused,
        "a dead adapter left its subscription active: {:?}",
        store.snapshot().jobs
    );
    assert_eq!(
        std::fs::read_to_string(&starts)?.lines().count(),
        4,
        "one start and three restarts within the intensity"
    );
    let wake = wakes(&session).join("\n");
    assert!(
        wake.contains("restart intensity is spent") && wake.contains("paused"),
        "the adapter died silently: {wake:?}"
    );
    Ok(())
}

fn alive(pid: &str) -> bool {
    yi_tools::command("kill")
        .args(["-0", pid])
        .status()
        .is_ok_and(|status| status.success())
}

fn pids(channel: &Channel) -> Vec<String> {
    channel
        .entries()
        .iter()
        .filter_map(|entry| entry.data.get("pid")?.as_u64().map(|pid| pid.to_string()))
        .collect()
}

/// Dies with the kill switch leaving adapters running, and with a resume that never restarts
/// them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_halt_stops_the_adapters_and_a_resume_restores_them() -> TestResult {
    let dir = Scratch::new("yi-channel-halt")?;
    let adapters = dir.join("home/.yi/adapters");
    std::fs::create_dir_all(&adapters)?;
    script(
        &adapters,
        "yi-adapter-beat",
        "i=0; while :; do i=$((i+1)); echo \"{\\\"id\\\":\\\"$$-$i\\\",\\\"data\\\":{\\\"pid\\\":$$}}\"; sleep 0.1; done",
    )?;
    adapter::adapters_home(&dir.join("home"));
    let store = Arc::new(JobStore::open(dir.join("scheduled-jobs.json")));
    let service = HeartbeatService::new(Arc::clone(&store), dir.to_string_lossy());
    service.bind_session("test".to_owned());
    let channel = Channel::at(dir.join("channels/beat.jsonl"));
    channel.open("beat://x", None)?;
    adapter::ensure("beat://x", &channel, &dir, 1_000)?;
    assert!(
        wait_until(5_000, || !pids(&channel).is_empty()).await,
        "the adapter never spoke"
    );
    let first = pids(&channel).first().cloned().ok_or("no pid")?;

    let halted = service.run("/heartbeat halt")?;
    assert!(halted.contains("1 channel adapter(s) stopped"), "{halted}");
    assert!(
        wait_until(3_000, || !alive(&first)).await,
        "the halt left the adapter running"
    );
    adapter::ensure("beat://x", &channel, &dir, 1_000)?;
    let held = channel.entries().len();
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    assert_eq!(
        channel.entries().len(),
        held,
        "an adapter started while halted"
    );

    service.run("/heartbeat resume")?;
    adapter::ensure("beat://x", &channel, &dir, 1_000)?;
    let back = wait_until(5_000, || pids(&channel).iter().any(|pid| *pid != first)).await;
    adapter::halt(true);
    adapter::halt(false);
    assert!(back, "the resume did not restore the adapter");
    Ok(())
}

/// A plan written before channels: its row blocked on `External { probe }` arms an exec wait
/// from the session's list, and the probe's exit 0 unblocks the row in the plan.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_external_probe_unblocks_through_an_exec_wait() -> TestResult {
    let dir = Scratch::new("yi-channel-probe")?;
    let (_session, todos) = session(&[])?;
    let gate = Todo::from_text("wait for the vendor mount")?;
    let label = gate.label.clone();
    todos.apply(
        Op::Append {
            phase: None,
            under: None,
            items: vec![gate],
        },
        None,
    )?;
    todos.apply(
        Op::Block {
            label: label.clone(),
            on: BlockedOn::External {
                probe: Some(ProbeCommand::new("test -e vendor/zstd")?),
            },
            note: "the vendor mount is still syncing".to_owned(),
            ask: None,
        },
        None,
    )?;
    std::fs::create_dir_all(dir.join("vendor"))?;
    std::fs::write(dir.join("vendor/zstd"), "")?;
    let store = Arc::new(JobStore::open(dir.join("scheduled-jobs.json")));
    let service = HeartbeatService::new(Arc::clone(&store), dir.to_string_lossy())
        .with_channels(dir.join("channels"));
    service.bind_session("test".to_owned());
    service.watch(&todos.list());
    let job = store
        .snapshot()
        .jobs
        .into_iter()
        .find(|job| job.unblocks.as_ref() == Some(&label))
        .ok_or("the probe armed no wait")?;
    let sub = job
        .channel
        .clone()
        .ok_or("the probe's wait reads no channel")?;
    assert!(
        sub.address.starts_with("exec://test -e vendor/zstd?every="),
        "{}",
        sub.address
    );
    let channel = Channel::at(&sub.path);

    assert_eq!(clock::fire(&todos, &job, &Firing::at(0))?, Fired::Idle);
    assert!(
        wait_until(5_000, || channel.last().is_some()).await,
        "the exec source never ran the probe"
    );
    let fired = clock::fire(&todos, &job, &Firing::at(0))?;
    assert!(
        matches!(&fired, Fired::Delivered(inner, _) if **inner == Fired::Unblocked(label.clone())),
        "{fired:?}"
    );
    assert!(
        todos
            .list()
            .items()
            .all(|item| !matches!(item.state, TodoState::Blocked { .. })),
        "the probe passed and the todo still waits"
    );
    adapter::halt(true);
    adapter::halt(false);
    Ok(())
}
