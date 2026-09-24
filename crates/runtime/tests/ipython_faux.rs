use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_loop::ExecutionMode;
use yi_runtime::{
    AgentSession, HostRegistry, KernelService, KernelServiceOptions, ProviderStream, SessionConfig,
    ipython_tool,
};
use yi_types::event::AgentEvent;
use yi_types::message::StopReason;
use yi_types::model::{Model, ModelCost};

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

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

#[tokio::test]
async fn ipython_tool_runs_a_cell_through_the_full_agent_loop() -> Result<(), Box<dyn Error>> {
    let provider = Arc::new(ProviderStream::new(None, None));
    let mut call_args = serde_json::Map::new();
    call_args.insert(
        "code".to_owned(),
        serde_json::json!("state = 21\nprint('kernel says', state * 2)"),
    );
    let mut second_args = serde_json::Map::new();
    second_args.insert("code".to_owned(), serde_json::json!("state + 1"));
    provider.queue_faux(vec![
        faux_assistant_message(
            vec![faux_tool_call("call-1", "ipython", call_args)],
            StopReason::ToolUse,
        ),
        faux_assistant_message(
            vec![faux_tool_call("call-2", "ipython", second_args)],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
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
    let mut registry = HostRegistry::default();
    registry.register_mcp_stubs();
    let service = Arc::new(KernelService::new(KernelServiceOptions {
        cwd: std::env::temp_dir(),
        home: home(),
        session_dir: None,
        family_dir: None,
        host: Arc::new(registry),
        on_restore: None,
        sandbox: None,
        snapshot_key: None,
        cell_ceiling: None,
    }));
    let mut tools = yi_tools::builtin_tools();
    tools.push(ipython_tool(Arc::clone(&service)));
    session.use_tools(tools, std::env::temp_dir(), None);

    let mut events = session.subscribe();
    session.prompt("run some python")?;
    session.wait_idle().await;
    service.dispose().await;

    let mut results = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::ToolExecutionEnd {
            result, is_error, ..
        } = event
        {
            assert!(!is_error, "ipython cell failed: {result:?}");
            for content in result.content {
                if let yi_types::message::Content::Text { text, .. } = content {
                    results.push(text);
                }
            }
        }
    }
    assert_eq!(results.len(), 2, "both ipython calls must produce results");
    assert!(
        results[0].contains("kernel says 42"),
        "first cell stdout missing: {}",
        results[0]
    );
    assert!(
        results[1].contains("22"),
        "namespace must persist between tool calls: {}",
        results[1]
    );
    Ok(())
}

/// Six of eight dogfood trials called `rlm` without `await`: such a spawn runs once, after the cell.
#[tokio::test]
async fn an_unawaited_spawn_runs_once_after_the_cell_and_says_so() -> Result<(), Box<dyn Error>> {
    let provider = Arc::new(ProviderStream::new(None, None));
    let mut call_args = serde_json::Map::new();
    call_args.insert("code".to_owned(), serde_json::json!("print(rlm.run('x'))"));
    provider.queue_faux(vec![
        faux_assistant_message(
            vec![faux_tool_call("call-1", "ipython", call_args)],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
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
    let mut registry = HostRegistry::default();
    registry.register_mcp_stubs();
    let spawns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = Arc::clone(&spawns);
    registry.register("rlm.run", move |_payload| {
        counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async {
            let handle = serde_json::json!({"rlm_child_id": "sub-1", "name": "x", "session_dir": "/tmp/sub-1", "model": "faux/faux-1"});
            handle.as_object().cloned().ok_or_else(|| "handle".to_owned())
        })
    });
    let service = Arc::new(KernelService::new(KernelServiceOptions {
        cwd: std::env::temp_dir(),
        home: home(),
        session_dir: None,
        family_dir: None,
        host: Arc::new(registry),
        on_restore: None,
        sandbox: None,
        snapshot_key: None,
        cell_ceiling: None,
    }));
    let mut tools = yi_tools::builtin_tools();
    tools.push(ipython_tool(Arc::clone(&service)));
    session.use_tools(tools, std::env::temp_dir(), None);

    let mut events = session.subscribe();
    session.prompt("spawn a child")?;
    session.wait_idle().await;
    service.dispose().await;

    let mut texts = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::ToolExecutionEnd { result, .. } = event {
            for content in result.content {
                if let yi_types::message::Content::Text { text, .. } = content {
                    texts.push(text);
                }
            }
        }
    }
    let cell = texts.first().ok_or("the ipython call produced no result")?;
    let note = cell.find("rlm.run() was not awaited, so the cell got a coroutine object");
    assert!(
        cell.find("<coroutine object ") < note && note < cell.find("RLMSpawnHandle(name='x'"),
        "the cell prints the coroutine, then the note with the spawned handle: {cell}"
    );
    assert_eq!(
        spawns.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "{cell}"
    );
    assert!(!cell.contains("never awaited"), "{cell}");
    Ok(())
}

/// Incident: a `while True:` cell had no clock but the interrupt, so it held the turn until
/// harbor killed the container (the 2026-09-10 harness audit, B6).
#[tokio::test]
async fn a_cell_that_never_returns_is_aborted_at_the_ceiling() -> Result<(), Box<dyn Error>> {
    let provider = Arc::new(ProviderStream::new(None, None));
    let mut call_args = serde_json::Map::new();
    call_args.insert(
        "code".to_owned(),
        serde_json::json!(
            "import time\nprint('started', flush=True)\nwhile True:\n    time.sleep(0.1)"
        ),
    );
    provider.queue_faux(vec![
        faux_assistant_message(
            vec![faux_tool_call("call-1", "ipython", call_args)],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
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
    let mut registry = HostRegistry::default();
    registry.register_mcp_stubs();
    let service = Arc::new(KernelService::new(KernelServiceOptions {
        cwd: std::env::temp_dir(),
        home: home(),
        session_dir: None,
        family_dir: None,
        host: Arc::new(registry),
        on_restore: None,
        sandbox: None,
        snapshot_key: None,
        cell_ceiling: Some(std::time::Duration::from_secs(1)),
    }));
    // The boot (and a cold venv build) stays off the clock under test.
    service.prewarm().await;
    let mut tools = yi_tools::builtin_tools();
    tools.push(ipython_tool(Arc::clone(&service)));
    session.use_tools(tools, std::env::temp_dir(), None);

    let mut events = session.subscribe();
    session.prompt("loop forever")?;
    let settled =
        tokio::time::timeout(std::time::Duration::from_secs(30), session.wait_idle()).await;
    service.dispose().await;
    settled.map_err(|_| "a cell with a 1 s ceiling was still running after 30 s")?;

    let mut ends = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::ToolExecutionEnd {
            result, is_error, ..
        } = event
        {
            for content in result.content {
                if let yi_types::message::Content::Text { text, .. } = content {
                    ends.push((is_error, text));
                }
            }
        }
    }
    let (is_error, cell) = ends.first().ok_or("the ipython call produced no result")?;
    assert!(
        *is_error && cell.contains("started") && cell.contains("[cell aborted]"),
        "a cell past its ceiling must come back aborted with its output so far: {cell}"
    );
    Ok(())
}
