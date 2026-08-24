use std::error::Error;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};
use yi_ai::faux::{faux_assistant_message, faux_text};
use yi_loop::ExecutionMode;
use yi_runtime::{
    AgentSession, HostRegistry, KernelService, KernelServiceOptions, ProviderStream, SessionConfig,
    SubagentHost, SubagentHostOptions,
};
use yi_types::message::{AgentMessage, StopReason};
use yi_types::model::{Model, ModelCost};

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

fn child_reply(text: &str) -> AgentMessage {
    let mut message = faux_assistant_message(vec![faux_text(text)], StopReason::Stop);
    if let AgentMessage::Assistant { usage, .. } = &mut message {
        usage.input = 100;
        usage.output = 20;
        usage.total_tokens = 120;
    }
    message
}

struct Harness {
    host: Arc<SubagentHost>,
    notices: Arc<Mutex<Vec<String>>>,
    attributed: Arc<AtomicU32>,
    root: PathBuf,
}

fn harness(depth: u8, max_depth: u8, child_answer: &'static str) -> Harness {
    let root = std::env::temp_dir().join(format!(
        "yi-recursion-{}-{depth}-{}",
        std::process::id(),
        child_answer.len()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let notices = Arc::new(Mutex::new(Vec::new()));
    let attributed = Arc::new(AtomicU32::new(0));
    let notice_sink = Arc::clone(&notices);
    let attribute_sink = Arc::clone(&attributed);
    let host = Arc::new(SubagentHost::new(SubagentHostOptions {
        depth,
        max_depth,
        max_children: 8,
        parent_session_dir: root.clone(),
        default_model: faux_model(),
        factory: Arc::new(move |model, thinking, _child_dir| {
            let provider = Arc::new(ProviderStream::new(None, None));
            provider.queue_faux(vec![child_reply(child_answer)]);
            Ok(AgentSession::new(
                SessionConfig {
                    system_prompt: "child sys".to_owned(),
                    model,
                    thinking_level: thinking,
                    tool_execution: ExecutionMode::Sequential,
                },
                provider,
            ))
        }),
        notice: Arc::new(move |text: &str| {
            if let Ok(mut sink) = notice_sink.lock() {
                sink.push(text.to_owned());
            }
        }),
        attribute: Arc::new(move |usage| {
            assert_eq!(usage.total_tokens, 120);
            attribute_sink.fetch_add(1, Ordering::SeqCst);
        }),
    }));
    Harness {
        host,
        notices,
        attributed,
        root,
    }
}

fn kwargs(pairs: &[(&str, &str)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), Value::String((*value).to_owned())))
        .collect()
}

async fn wait_for_status(host: &Arc<SubagentHost>, child_id: &str, status: &str) -> bool {
    for _ in 0..100 {
        let list = host.list();
        let found = list["subagents"].as_array().and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry["rlm_child_id"] == child_id)
        });
        if found.is_some_and(|entry| entry["status"] == status) {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
    false
}

#[tokio::test]
async fn spawn_runs_child_to_completion_with_notice_and_attribution() -> TestResult {
    let harness = harness(0, 1, "the answer is forty-two");
    let reply = harness
        .host
        .spawn(
            "compute the answer".to_owned(),
            kwargs(&[("name", "helper")]),
        )
        .map_err(|error| error.to_string())?;
    let child_id = reply["rlm_child_id"]
        .as_str()
        .ok_or("missing child id")?
        .to_owned();
    assert!(child_id.starts_with("sub-"));
    assert_eq!(reply["name"], "helper");
    assert_eq!(reply["model"], "faux/faux-1");
    assert!(
        harness.root.join(&child_id).is_dir(),
        "child session dir must exist"
    );

    assert!(
        wait_for_status(&harness.host, &child_id, "completed").await,
        "child must reach completed"
    );
    assert_eq!(
        harness.attributed.load(Ordering::SeqCst),
        1,
        "child usage must attribute once"
    );
    let notices = harness.notices.lock().map_err(|_| "poisoned")?.clone();
    assert_eq!(notices.len(), 1);
    assert!(
        notices[0].contains("completed without replying")
            && notices[0].contains("the answer is forty-two"),
        "terminal notice must carry the answer preview: {}",
        notices[0]
    );
    Ok(())
}

#[tokio::test]
async fn depth_limit_name_collision_slots_and_delete() -> TestResult {
    let at_limit = harness(1, 1, "unused");
    let refused = at_limit.host.spawn("nested".to_owned(), Map::new());
    assert_eq!(
        refused.err().as_deref(),
        Some("RLM recursion depth limit reached (RLM_DEPTH=1, RLM_MAX_DEPTH=1)"),
        "a depth-1 child must not spawn grandchildren"
    );

    let harness = harness(0, 1, "ok");
    harness
        .host
        .spawn("first".to_owned(), kwargs(&[("name", "twin")]))
        .map_err(|error| error.to_string())?;
    let collision = harness
        .host
        .spawn("second".to_owned(), kwargs(&[("name", "twin")]));
    assert!(
        collision
            .err()
            .is_some_and(|error| error.contains("\"twin\" is already taken at depth 1")),
        "duplicate names must be rejected"
    );

    for index in 0..7 {
        harness
            .host
            .spawn(
                format!("filler {index}"),
                kwargs(&[("name", &format!("filler-{index}"))]),
            )
            .map_err(|error| error.to_string())?;
    }
    let overflow = harness.host.spawn("ninth".to_owned(), Map::new());
    assert!(
        overflow
            .err()
            .is_some_and(|error| error.contains("child limit reached")),
        "a completed child must hold its slot until closed"
    );

    let deleted = harness
        .host
        .delete("twin")
        .map_err(|error| error.to_string())?;
    assert_eq!(deleted["subagent"]["session_name"], "twin");
    let after = harness
        .host
        .spawn("tenth".to_owned(), kwargs(&[("name", "tenth")]));
    assert!(
        after.is_ok(),
        "deleting a child must release its slot: {after:?}"
    );

    assert!(harness.host.delete("no-such-child").is_err());

    let unknown_kwarg = harness.host.spawn(
        "bad".to_owned(),
        kwargs(&[("name", "bad"), ("fork", "all")]),
    );
    assert_eq!(
        unknown_kwarg.err().as_deref(),
        Some("Unsupported rlm.run kwargs: fork"),
        "unknown kwargs are an error, never silently dropped"
    );
    Ok(())
}

#[tokio::test]
async fn rlm_run_round_trips_through_a_real_kernel() -> TestResult {
    let harness = harness(0, 1, "kernel child says hi");
    let mut registry = HostRegistry::default();
    registry.register_mcp_stubs();
    harness.host.register(&mut registry);
    let service = Arc::new(KernelService::new(KernelServiceOptions {
        cwd: std::env::temp_dir(),
        home: std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default(),
        session_dir: Some(harness.root.clone()),
        host: Arc::new(registry),
        on_restore: None,
    }));

    let cancelled: yi_tools::CancelFlag = Arc::new(|| false);
    let spawn_cell = tokio::task::spawn_blocking({
        let service = Arc::clone(&service);
        let cancelled = Arc::clone(&cancelled);
        move || {
            yi_tools::KernelBridge::execute_cell(
                service.as_ref(),
                "h = await rlm.run('greet the parent', name='helper')\nprint(h.rlm_child_id.startswith('sub-'), h.name, h.model)",
                &cancelled,
            )
        }
    })
    .await??;
    assert!(
        spawn_cell.result.stdout.contains("True helper faux/faux-1"),
        "rlm.run must return a validated spawn handle: {} {}",
        spawn_cell.result.stdout,
        spawn_cell.result.stderr
    );

    let list_cell = tokio::task::spawn_blocking({
        let service = Arc::clone(&service);
        let cancelled = Arc::clone(&cancelled);
        move || {
            yi_tools::KernelBridge::execute_cell(
                service.as_ref(),
                "import asyncio\nfor _ in range(100):\n    subs = await rlm.list_subagents()\n    if subs and subs[0].status == 'completed':\n        break\n    await asyncio.sleep(0.05)\nprint(subs[0].session_name, subs[0].status)",
                &cancelled,
            )
        }
    })
    .await??;
    assert!(
        list_cell.result.stdout.contains("helper completed"),
        "rlm.list_subagents must expose the completed child: {} {}",
        list_cell.result.stdout,
        list_cell.result.stderr
    );

    let delete_cell = tokio::task::spawn_blocking({
        let service = Arc::clone(&service);
        let cancelled = Arc::clone(&cancelled);
        move || {
            yi_tools::KernelBridge::execute_cell(
                service.as_ref(),
                "gone = await rlm.delete_subagent('helper')\nprint(gone.rlm_child_id.startswith('sub-'), len(await rlm.list_subagents()))",
                &cancelled,
            )
        }
    })
    .await??;
    assert!(
        delete_cell.result.stdout.contains("True 0"),
        "rlm.delete_subagent must reap the child: {} {}",
        delete_cell.result.stdout,
        delete_cell.result.stderr
    );

    let refused_cell = tokio::task::spawn_blocking({
        let service = Arc::clone(&service);
        let cancelled = Arc::clone(&cancelled);
        move || {
            yi_tools::KernelBridge::execute_cell(
                service.as_ref(),
                "try:\n    await rlm.run('too deep', junk=1)\nexcept RuntimeError as e:\n    print(f'refused: {e}')",
                &cancelled,
            )
        }
    })
    .await??;
    assert!(
        refused_cell
            .result
            .stdout
            .contains("refused: Unsupported rlm.run kwargs: junk"),
        "handler errors must surface as Python RuntimeError: {}",
        refused_cell.result.stdout
    );

    service.dispose().await;
    Ok(())
}
