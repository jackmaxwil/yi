use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

use crate::scratch::Scratch;
use serde_json::{Map, Value, json};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_loop::ExecutionMode;
use yi_runtime::kernel_tools::{Calls, ipython_tool, register};
use yi_runtime::{
    AgentSession, HostRegistry, KernelService, KernelServiceOptions, ProviderStream, SessionConfig,
};
use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, StopReason};
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

/// One cell, the model's own tools behind it: what a program wrote, the wall refused and the
/// kernel left out, read from the cell's own result as the transcript stores it.
const CELL: &str = r#"
await yi.tools.write(path="notes.txt", content="alpha\n")
view = await yi.tools.read(path="notes.txt")
try:
    await yi.tools.write(path="locked/x.txt", content="no")
    walled = "wrote"
except yi.Refused:
    walled = "refused"
try:
    await yi.tools.call("ipython", code="1")
    nested = "ran"
except yi.tools.UnknownTool:
    nested = "excluded"
print(walled, nested, "alpha" in view.text)
"#;

#[tokio::test]
async fn a_cell_writes_and_reads_through_the_sessions_own_gated_tools() -> Result<(), Box<dyn Error>>
{
    let dir = Scratch::new("kernel-tools")?;
    let cwd = std::fs::canonicalize(&*dir)?;
    std::fs::create_dir(cwd.join("locked"))?;
    let provider = Arc::new(ProviderStream::new(None));
    let mut code = Map::new();
    code.insert("code".to_owned(), json!(CELL));
    provider.queue_faux(vec![
        faux_assistant_message(
            vec![faux_tool_call("call-1", "ipython", code)],
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
    session.set_wall(yi_runtime::wall::Wall {
        deny_write: vec![cwd.join("locked")],
        ..yi_runtime::wall::Wall::default()
    });
    let calls = Arc::new(Calls::default());
    let mut registry = HostRegistry::default();
    registry.register_mcp_stubs();
    register(
        &mut registry,
        session.tools_handle(),
        Arc::clone(&calls),
        true,
    );
    let service = Arc::new(KernelService::new(KernelServiceOptions {
        cwd: cwd.clone(),
        home: home(),
        session_dir: None,
        family_dir: None,
        host: Arc::new(registry),
        on_restore: None,
        on_boot: None,
        sandbox: None,
        snapshot_key: None,
        per_session_state: false,
        cell_ceiling: None,
    }));
    let mut tools = yi_tools::builtin_tools();
    tools.push(ipython_tool(Arc::clone(&service), Some(calls)));
    session.use_tools(tools, cwd.clone(), None);

    let mut events = session.subscribe();
    session.prompt("edit through the kernel")?;
    session.wait_idle().await;
    service.dispose().await;

    let mut cell = None;
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::ToolExecutionEnd {
            tool_name, result, ..
        } = event
            && tool_name == "ipython"
        {
            cell = Some(result);
        }
    }
    let cell = cell.ok_or("the cell never ended")?;
    let text = yi_types::message::join_text(&cell.content, "\n");
    assert!(text.contains("refused excluded True"), "{text}");
    assert_eq!(std::fs::read_to_string(cwd.join("notes.txt"))?, "alpha\n");
    assert!(
        !cwd.join("locked/x.txt").exists(),
        "the wall let a cell write"
    );

    let rows = yi_context::nested_calls(Some(&cell.details));
    let shape: Vec<(&str, bool, Option<&str>, bool)> = (rows.iter())
        .map(|row| {
            let kind = row.error_kind.as_deref();
            (row.tool.as_str(), row.ok, kind, row.args.is_some())
        })
        .collect();
    assert_eq!(
        shape,
        [
            ("write", true, None, true),
            ("read", true, None, false),
            ("write", false, Some("refused"), true),
        ],
        "an excluded tool never runs, so it is no row"
    );

    let stored = AgentMessage::ToolResult {
        tool_call_id: "call-1".to_owned(),
        tool_name: "ipython".to_owned(),
        content: cell.content.clone(),
        details: Some(cell.details.clone()),
        usage: None,
        added_tool_names: None,
        is_error: false,
        timestamp: 0,
    };
    let mut ops = yi_context::FileOps::default();
    yi_context::details::extract_file_ops_from_message(&stored, &mut ops);
    assert!(
        ops.written.contains("notes.txt"),
        "compaction's file lists miss the program's write: {ops:?}"
    );
    let landed = yi_runtime::todo::coupling::landed(&stored, std::slice::from_ref(&stored));
    assert_eq!(
        landed.0, 1,
        "the todo nudge counts the write a cell made, not the refusal"
    );
    assert!(matches!(cell.details.get("calls"), Some(Value::Array(_))));
    Ok(())
}
