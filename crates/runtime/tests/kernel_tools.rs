use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::scratch::Scratch;
use serde_json::{Map, json};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_kernel::client::HostHandlers;
use yi_loop::ExecutionMode;
use yi_runtime::kernel_tools::{Calls, ipython_tool, register};
use yi_runtime::{
    AgentSession, HostRegistry, KernelService, KernelServiceOptions, ProviderStream, SessionConfig,
};
use yi_tools::CellCalls;
use yi_types::event::{AgentEvent, ToolResult};
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
listed = [row["name"] for row in yi.tools.search(limit=50)["tools"]]
signed = all(callable(getattr(yi.tools, name)) for name in listed)
print(walled, nested, "alpha" in view.text, signed and "grep" in listed)
"#;

fn session(cwd: &Path, code: &str) -> AgentSession {
    let provider = Arc::new(ProviderStream::new(None));
    let mut args = Map::new();
    args.insert("code".to_owned(), json!(code));
    provider.queue_faux(vec![
        faux_assistant_message(
            vec![faux_tool_call("call-1", "ipython", args)],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let session = AgentSession::new(
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
    session
}

/// The `ipython` result of `code` run as the model's one cell, the session's tools behind it.
async fn run_cell(cwd: &Path, code: &str) -> Result<ToolResult, Box<dyn Error>> {
    let mut session = session(cwd, code);
    let calls = Arc::new(Calls::default());
    let mut registry = HostRegistry::default();
    registry.register_mcp_stubs();
    let table = session.tools_handle();
    register(&mut registry, table, Arc::clone(&calls), true);
    let service = Arc::new(KernelService::new(KernelServiceOptions {
        cwd: cwd.to_path_buf(),
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
    session.use_tools(tools, cwd.to_path_buf(), None);
    let mut events = session.subscribe();
    session.prompt("run the cell")?;
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
    Ok(cell.ok_or("the cell never ended")?)
}

fn stored(cell: &ToolResult, is_error: bool) -> AgentMessage {
    AgentMessage::ToolResult {
        tool_call_id: "call-1".to_owned(),
        tool_name: "ipython".to_owned(),
        content: cell.content.clone(),
        details: Some(cell.details.clone()),
        usage: None,
        added_tool_names: None,
        is_error,
        timestamp: 0,
    }
}

fn scratch(name: &str) -> Result<(Scratch, PathBuf), Box<dyn Error>> {
    let dir = Scratch::new(name)?;
    let cwd = std::fs::canonicalize(&*dir)?;
    std::fs::create_dir(cwd.join("locked"))?;
    Ok((dir, cwd))
}

#[tokio::test]
async fn a_cell_writes_and_reads_through_the_sessions_own_gated_tools() -> Result<(), Box<dyn Error>>
{
    let (_dir, cwd) = scratch("kernel-tools")?;
    let cell = run_cell(&cwd, CELL).await?;
    let text = yi_types::message::join_text(&cell.content, "\n");
    assert!(text.contains("refused excluded True True"), "{text}");
    assert_eq!(std::fs::read_to_string(cwd.join("notes.txt"))?, "alpha\n");
    let walled = cwd.join("locked/x.txt");
    assert!(!walled.exists(), "the wall let a cell write");

    let rows = yi_context::nested_calls(Some(&cell.details));
    let shape: Vec<(&str, bool, Option<&str>, Vec<String>)> = (rows.iter())
        .map(|row| {
            let kind = row.error_kind.as_deref();
            (row.tool.as_str(), row.ok, kind, row.paths.clone())
        })
        .collect();
    let named = |path: &str| vec![path.to_owned()];
    assert_eq!(
        shape,
        [
            ("write", true, None, named("notes.txt")),
            ("read", true, None, named("notes.txt")),
            ("write", false, Some("refused"), named("locked/x.txt")),
        ],
        "an excluded tool never runs, so it is no row"
    );
    assert!(
        !serde_json::to_string(&rows)?.contains("alpha"),
        "a row names a write's file, never its content"
    );
    let mut ops = yi_context::FileOps::default();
    yi_context::details::extract_file_ops_from_message(&stored(&cell, false), &mut ops);
    assert!(
        ops.written.contains("notes.txt"),
        "compaction's file lists miss the program's write: {ops:?}"
    );
    let message = stored(&cell, false);
    let landed = yi_runtime::todo::coupling::landed(&message, std::slice::from_ref(&message));
    assert_eq!(
        landed.0, 1,
        "the todo nudge counts the write a cell made, not the refusal"
    );
    Ok(())
}

#[tokio::test]
async fn a_cell_that_raises_after_its_edit_still_counts_the_edit() -> Result<(), Box<dyn Error>> {
    let (_dir, cwd) = scratch("kernel-tools-raise")?;
    let code =
        "await yi.tools.write(path=\"kept.txt\", content=\"x\\n\")\nraise RuntimeError(\"after\")";
    let cell = run_cell(&cwd, code).await?;
    assert!(cwd.join("kept.txt").exists());
    let message = stored(&cell, true);
    let landed = yi_runtime::todo::coupling::landed(&message, std::slice::from_ref(&message));
    assert_eq!(landed.0, 1, "the write is on disk though the cell raised");
    Ok(())
}

#[tokio::test]
async fn gathered_writes_from_one_cell_run_one_at_a_time() -> Result<(), Box<dyn Error>> {
    let (_dir, cwd) = scratch("kernel-tools-order")?;
    let code = r#"
import asyncio, time
started = time.monotonic()
await asyncio.gather(*[yi.tools.bash(command="sleep 0.4") for _ in range(2)])
print("ordered", time.monotonic() - started >= 0.8)
"#;
    let cell = run_cell(&cwd, code).await?;
    let text = yi_types::message::join_text(&cell.content, "\n");
    assert!(
        text.contains("ordered True"),
        "two exec calls overlapped: {text}"
    );
    Ok(())
}

fn request(name: &str, args: serde_json::Value) -> Map<String, serde_json::Value> {
    let mut payload = Map::new();
    payload.insert("name".to_owned(), json!(name));
    payload.insert("args".to_owned(), args);
    payload
}

#[tokio::test]
async fn a_call_outside_a_cell_is_refused_and_reads_past_the_cap_are_counted()
-> Result<(), Box<dyn Error>> {
    let (_dir, cwd) = scratch("kernel-tools-cap")?;
    std::fs::write(cwd.join("a.txt"), "a\n")?;
    let mut session = session(&cwd, "");
    session.use_tools(yi_tools::builtin_tools(), cwd.clone(), None);
    let calls = Arc::new(Calls::default());
    let mut registry = HostRegistry::default();
    register(
        &mut registry,
        session.tools_handle(),
        Arc::clone(&calls),
        true,
    );
    let dispatch = |payload| {
        registry
            .dispatch("tool.call", payload)
            .ok_or("no tool.call")
    };
    let read = || request("read", json!({"path": "a.txt"}));

    let closed = dispatch(read())?.await?;
    assert_eq!(closed.get("kind"), Some(&json!("cancelled")), "{closed:?}");
    calls.open();
    let kept = yi_types::kernel::NESTED_READS_KEPT;
    for _ in 0..=kept {
        dispatch(read())?.await?;
    }
    dispatch(request("write", json!({"path": "b.txt", "content": "b\n"})))?.await?;
    let (rows, dropped) = calls.close();
    assert_eq!((rows.len(), dropped), (kept + 1, 1));
    assert_eq!(
        rows.last().map(|row| row.tool.as_str()),
        Some("write"),
        "a write past the cap is kept"
    );
    let after = dispatch(read())?.await?;
    assert_eq!(
        after.get("kind"),
        Some(&json!("cancelled")),
        "closing a cell shuts the gate"
    );
    Ok(())
}

/// A program reads past the model's page: grep's 2,000-match collection cap and read's first
/// look, then edits a line only the value showed it.
#[tokio::test]
async fn a_program_gets_whole_values_and_edits_a_line_the_view_never_showed()
-> Result<(), Box<dyn Error>> {
    let (_dir, cwd) = scratch("kernel-tools-values")?;
    let hits = (1..=2500).map(|n| format!("hit {n}"));
    let rest = (2501..=3000).map(|n| format!("other {n}"));
    let body: Vec<String> = hits.chain(rest).collect();
    std::fs::write(cwd.join("big.txt"), body.join("\n") + "\n")?;
    let code = r#"
g = await yi.tools.grep(pattern="^hit", path="big.txt")
r = await yi.tools.read(path="big.txt")
last = r.value.lines[-1]
await yi.tools.edit(patch=f"[big.txt#{r.value.tag}]\nPUT {last.n}.={last.n}:\n+edited {last.n}\n")
print(len(g.value.matches), g.value.matches[-1].line, len(r.value.lines), last.n, "matches" in g.text)
"#;
    let cell = run_cell(&cwd, code).await?;
    let text = yi_types::message::join_text(&cell.content, "\n");
    assert!(text.contains("2500 2500 3000 3000"), "{text}");
    let written = std::fs::read_to_string(cwd.join("big.txt"))?;
    assert_eq!(
        written.lines().last(),
        Some("edited 3000"),
        "the edit at a line the view never showed"
    );
    assert!(
        !cell.details.to_string().contains("\"matches\""),
        "a value never reaches the transcript"
    );
    Ok(())
}

/// Each tool hands a program its own shape; a field a later change renames breaks this cell.
#[tokio::test]
async fn every_tool_hands_a_program_its_typed_value() -> Result<(), Box<dyn Error>> {
    let (_dir, cwd) = scratch("kernel-tools-shapes")?;
    std::fs::create_dir(cwd.join("src"))?;
    std::fs::write(cwd.join("src/a.rs"), "fn a() {}\n")?;
    let code = r#"
w = (await yi.tools.write(path="src/b.rs", content="fn b() {}\n")).value
d = (await yi.tools.read(path="src")).value
g = (await yi.tools.read(path="src/*.rs")).value
e = (await yi.tools.edit(patch=f"[src/b.rs#{w.tag}]\nPUT 1.=1:\n+fn b2() {{}}\n")).value
b = (await yi.tools.bash(command="echo out; echo err >&2; exit 3")).value
c = (await yi.tools.get_context()).value
rows = [
    (w.path, w.bytes),
    [(entry.name, entry.kind, entry.bytes) for entry in d.entries],
    g.paths,
    [(f.path, f.op, f.firstChangedLine) for f in e.files], "+fn b2" in e.patch,
    (b.exitCode, b.stdout, b.stderr),
    len(c.layers), all(layer.body is not None or layer.absent for layer in c.layers),
]
print(repr(rows))
"#;
    let cell = run_cell(&cwd, code).await?;
    let text = yi_types::message::join_text(&cell.content, "\n");
    let expected = "[('src/b.rs', 10), [('a.rs', 'file', 10), ('b.rs', 'file', 10)], ['src/a.rs', 'src/b.rs'], [('src/b.rs', 'updated', 1)], True, (3, 'out\\n', 'err\\n'), 6, True]";
    assert!(text.contains(expected), "{text}");
    Ok(())
}
