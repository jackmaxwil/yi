use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Map, Value};
use yi_ai::faux::{faux_assistant_message, faux_text};
use yi_loop::ExecutionMode;
use yi_runtime::{
    AgentSession, HostRegistry, KernelService, KernelServiceOptions, ProviderStream, SessionConfig,
    SubagentHost, SubagentHostOptions,
};
use yi_types::message::{AgentMessage, StopReason};
use yi_types::model::{Effort, Model, ModelCost};

type TestResult = Result<(), Box<dyn Error>>;

const POLL_ATTEMPTS: usize = 400;
const POLL_INTERVAL_MS: u64 = 30;

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

/// `answer: None` queues nothing, so the child's turn ends on the faux
/// provider's own empty-queue error rather than an answer.
fn host(answer: Option<&'static str>) -> (Arc<SubagentHost>, PathBuf) {
    static SCENARIO: AtomicUsize = AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "yi-child-transcript-{}-{}",
        std::process::id(),
        SCENARIO.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    let (events, _keep) = tokio::sync::broadcast::channel(64);
    let host = Arc::new(SubagentHost::new(SubagentHostOptions {
        depth: 0,
        max_depth: 1,
        max_children: 4,
        parent_session_dir: root.clone(),
        cwd: std::env::temp_dir(),
        home: std::env::temp_dir(),
        lane_slots: 1,
        defaults: Arc::new(|| (faux_model(), Effort::Medium)),
        factory: Arc::new(move |build: yi_runtime::ChildBuild<'_>| {
            let provider = Arc::new(ProviderStream::new(None, None));
            if let Some(text) = answer {
                provider.queue_faux(vec![faux_assistant_message(
                    vec![faux_text(text)],
                    StopReason::Stop,
                )]);
            }
            Ok(AgentSession::new(
                SessionConfig {
                    system_prompt: "child sys".to_owned(),
                    model: build.model,
                    thinking_level: build.thinking,
                    tool_execution: ExecutionMode::Sequential,
                },
                provider,
            ))
        }),
        notice: Arc::new(|_| {}),
        events,
        report: Arc::new(|_| {}),
        parent_messages: Arc::new(Vec::new),
        attribute: Arc::new(|_| {}),
        store: Arc::new(|| None),
        plans_dir: std::env::temp_dir().join(".yi/plans"),
    }));
    (host, root)
}

fn kwargs(name: &str) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("name".to_owned(), Value::String(name.to_owned()));
    map
}

async fn wait_for_status(host: &Arc<SubagentHost>, child_id: &str, status: &str) -> bool {
    for _ in 0..POLL_ATTEMPTS {
        let list = host.list();
        let found = list["subagents"].as_array().and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry["rlm_child_id"] == child_id)
        });
        if found.is_some_and(|entry| entry["status"] == status) {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    false
}

/// The affordance promises `<session_dir>/*.jsonl`: exactly one file, named by
/// the session repo, sitting directly in the child's own directory.
fn transcript_in(dir: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "jsonl") {
            found.push(path);
        }
    }
    match found.as_slice() {
        [only] => Ok(only.clone()),
        other => Err(format!(
            "expected one transcript directly in {}, found {other:?}",
            dir.display()
        )
        .into()),
    }
}

fn messages_of(path: &Path) -> Result<Vec<AgentMessage>, Box<dyn Error>> {
    let store = yi_session::load_session(path)?;
    let entries = store.find_entries_on_branch(
        "main",
        &yi_session::EntryQuery {
            order: yi_session::EntryOrder::OldestFirst,
            ..yi_session::EntryQuery::default()
        },
        &yi_session::BranchBounds::default(),
    )?;
    Ok(yi_context::project(&entries))
}

async fn spawn_and_settle(
    answer: Option<&'static str>,
    status: &str,
) -> Result<(Arc<SubagentHost>, String, PathBuf), Box<dyn Error>> {
    let (host, _root) = host(answer);
    let reply = host
        .spawn("compute the answer".to_owned(), kwargs("helper"))
        .map_err(|error| error.to_string())?;
    let child_id = reply["rlm_child_id"]
        .as_str()
        .ok_or("missing child id")?
        .to_owned();
    let session_dir = PathBuf::from(reply["session_dir"].as_str().ok_or("missing session dir")?);
    assert!(
        wait_for_status(&host, &child_id, status).await,
        "child must reach {status}"
    );
    Ok((host, child_id, session_dir))
}

#[tokio::test]
async fn a_faux_childs_transcript_reads_back_through_the_parents_reader() -> TestResult {
    let (_host, _child_id, dir) = spawn_and_settle(Some("the answer is forty-two"), "completed")
        .await
        .map_err(|error| error.to_string())?;
    let messages = messages_of(&transcript_in(&dir)?)?;
    let rendered = format!("{messages:?}");
    assert!(
        rendered.contains("[task from parent]") && rendered.contains("compute the answer"),
        "the child's own prompt must be on file: {rendered}"
    );
    assert!(
        rendered.contains("the answer is forty-two"),
        "the child's answer must be on file: {rendered}"
    );
    Ok(())
}

/// A child that dies is the run whose evidence a reader most needs; the error
/// text has to be in the file, not only in the parent's terminal notice.
#[tokio::test]
async fn an_errored_child_leaves_a_transcript_naming_the_error() -> TestResult {
    let (_host, _child_id, dir) = spawn_and_settle(None, "error")
        .await
        .map_err(|error| error.to_string())?;
    let messages = messages_of(&transcript_in(&dir)?)?;
    let named = messages.iter().any(|message| {
        matches!(
            message,
            AgentMessage::Assistant {
                stop_reason: StopReason::Error,
                error_message: Some(text),
                ..
            } if text.contains("No more faux responses queued")
        )
    });
    assert!(named, "the transcript must name the error: {messages:?}");
    Ok(())
}

/// Invariant: a real run reaches a child only through a kernel cell, so the
/// spawn, the typed answer and the file are one journey or none of them is
/// proven. `just journeys` runs it; the ordinary suite skips the kernel boot.
#[tokio::test]
#[ignore = "tier-2 journey: `just journeys`"]
async fn a_kernel_cell_spawns_a_child_whose_typed_answer_and_transcript_land() -> TestResult {
    let (host, root) = host(Some(r#"{"answer": 42}"#));
    let mut registry = HostRegistry::default();
    registry.register_mcp_stubs();
    host.register(&mut registry);
    let service = KernelService::new(KernelServiceOptions {
        cwd: std::env::temp_dir(),
        home: std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default(),
        session_dir: Some(root.clone()),
        family_dir: None,
        host: Arc::new(registry),
        on_restore: None,
        sandbox: None,
        snapshot_key: None,
    });
    let cancelled: yi_tools::CancelFlag = Arc::new(|| false);
    let cell = tokio::task::spawn_blocking(move || {
        yi_tools::KernelBridge::execute_cell(
            &service,
            "h = await rlm.run('report the answer', name='helper')\nr = await h.result()\nprint('typed', r['json']['answer'], r['name'])\nprint(h.session_dir)",
            &cancelled,
        )
    })
    .await??;

    let stdout = cell.result.stdout.clone();
    assert!(
        stdout.contains("typed 42 helper"),
        "the cell must get the child's answer back as data: {stdout} {}",
        cell.result.stderr
    );
    let dir = stdout
        .lines()
        .last()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .ok_or("the cell printed no child session dir")?;
    let messages = messages_of(&transcript_in(Path::new(dir))?)?;
    let rendered = format!("{messages:?}");
    assert!(
        rendered.contains("report the answer") && rendered.contains("42"),
        "the child spawned from the cell must leave its own transcript: {rendered}"
    );
    Ok(())
}

/// Reaping drops the record and the live session; the evidence outlives both.
#[tokio::test]
async fn a_reaped_childs_transcript_survives_delete() -> TestResult {
    let (host, child_id, dir) = spawn_and_settle(Some("done"), "completed")
        .await
        .map_err(|error| error.to_string())?;
    let path = transcript_in(&dir)?;
    host.delete(&child_id).map_err(|error| error.to_string())?;
    assert!(
        path.is_file(),
        "the transcript must survive the reap: {}",
        path.display()
    );
    Ok(())
}
