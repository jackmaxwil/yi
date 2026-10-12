use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::scratch::Scratch;
use serde_json::{Map, json};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_kernel::client::HostHandlers;
use yi_loop::ExecutionMode;
use yi_runtime::kernel_tools::{Calls, ipython_tool, register};
use yi_runtime::permission::PermissionBroker;
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

fn session(cwd: &Path, codes: &[&str]) -> AgentSession {
    let provider = Arc::new(ProviderStream::new(None));
    let mut turns: Vec<_> = (codes.iter().enumerate())
        .map(|(at, code)| {
            let mut args = Map::new();
            args.insert("code".to_owned(), json!(code));
            let call = faux_tool_call(&format!("call-{at}"), "ipython", args);
            faux_assistant_message(vec![call], StopReason::ToolUse)
        })
        .collect();
    turns.push(faux_assistant_message(
        vec![faux_text("done")],
        StopReason::Stop,
    ));
    provider.queue_faux(turns);
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
    let (cells, _) = run_cells(cwd, &[code], None, None).await?;
    Ok(cells.into_iter().next().ok_or("the cell never ended")?)
}

/// Each code run as the model's next cell in one session, and their results in order; a
/// `console` cell is typed into the booted kernel first and its output returned beside them.
async fn run_cells(
    cwd: &Path,
    codes: &[&str],
    broker: Option<Arc<PermissionBroker>>,
    console: Option<&'static str>,
) -> Result<(Vec<ToolResult>, String), Box<dyn Error>> {
    let mut session = session(cwd, codes);
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
    session.use_tools(tools, cwd.to_path_buf(), broker);
    let never: yi_tools::CancelFlag = Arc::new(|| false);
    let typed = match console {
        Some(code) => {
            service.execute_user_cell("1", &never).await;
            let (service, never) = (Arc::clone(&service), Arc::clone(&never));
            let typed = tokio::spawn(async move { service.execute_user_cell(code, &never).await });
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            Some(typed)
        }
        None => None,
    };
    let mut events = session.subscribe();
    session.prompt("run the cell")?;
    session.wait_idle().await;
    let console = match typed {
        Some(typed) => yi_types::message::join_text(&typed.await?.result.content, "\n"),
        None => String::new(),
    };
    service.dispose().await;
    let mut cells = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::ToolExecutionEnd {
            tool_name, result, ..
        } = event
            && tool_name == "ipython"
        {
            cells.push(result);
        }
    }
    Ok((cells, console))
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
async def read_later():
    await asyncio.sleep(0.1)
    await yi.tools.grep(pattern="x", path=".")
    return time.monotonic() - started
started = time.monotonic()
_, read_at = await asyncio.gather(yi.tools.bash(command="sleep 0.4; touch t"), read_later())
print("waited", read_at >= 0.4)
"#;
    let cell = run_cell(&cwd, code).await?;
    let text = yi_types::message::join_text(&cell.content, "\n");
    assert!(
        text.contains("ordered True"),
        "two exec calls overlapped: {text}"
    );
    assert!(
        text.contains("waited True"),
        "a read overlapped an exec: {text}"
    );
    Ok(())
}

/// Incident: a task cell 1 left running called a tool during cell 2, and cell 2 was credited.
#[tokio::test]
async fn a_task_that_outlived_its_cell_is_refused_while_the_next_cell_runs()
-> Result<(), Box<dyn Error>> {
    let (_dir, cwd) = scratch("kernel-tools-late")?;
    let first = r#"
import asyncio
async def late():
    await asyncio.sleep(0.5)
    try:
        await yi.tools.write(path="late.txt", content="x\n")
        return "late wrote"
    except yi.tools.ToolError as error:
        return f"late refused {error.kind}"
task = asyncio.ensure_future(late())
"#;
    let second = "import asyncio\nawait asyncio.sleep(1.5)";
    let (cells, _) = run_cells(&cwd, &[first, second, "print(task.result())"], None, None).await?;
    let [_, during, after] = cells.as_slice() else {
        return Err(format!("{} cells ended, not 3", cells.len()).into());
    };
    let text = yi_types::message::join_text(&after.content, "\n");
    assert!(text.contains("late refused cancelled"), "{text}");
    assert!(
        !cwd.join("late.txt").exists(),
        "a task from an ended cell wrote"
    );
    let rows = yi_context::nested_calls(Some(&during.details));
    assert!(rows.is_empty(), "the next cell was credited with {rows:?}");
    Ok(())
}

/// A cell typed into the console while the agent's cell waits for the kernel is still the
/// user's: its calls are refused and never land on the agent's cell.
#[tokio::test]
async fn a_console_cell_running_while_the_agents_cell_waits_is_refused()
-> Result<(), Box<dyn Error>> {
    let (_dir, cwd) = scratch("kernel-tools-console")?;
    let typed = r#"
import asyncio
await asyncio.sleep(1.0)
try:
    await yi.tools.write(path="console.txt", content="c\n")
    print("console wrote")
except yi.tools.ToolError as error:
    print("console refused", error.kind)
"#;
    let (cells, console) = run_cells(&cwd, &["print('agent')"], None, Some(typed)).await?;
    assert!(console.contains("console refused cancelled"), "{console}");
    assert!(!cwd.join("console.txt").exists(), "a console cell wrote");
    let cell = cells.first().ok_or("the agent's cell never ended")?;
    let rows = yi_context::nested_calls(Some(&cell.details));
    assert!(
        rows.is_empty(),
        "the agent's cell was credited with {rows:?}"
    );
    Ok(())
}

/// The cell's own approval never covers its calls: an Auto broker with nobody to ask judges
/// the nested `rm` on its own arguments.
#[tokio::test]
async fn an_auto_broker_with_no_asker_refuses_a_cells_nested_rm() -> Result<(), Box<dyn Error>> {
    let (_dir, cwd) = scratch("kernel-tools-broker")?;
    std::fs::write(cwd.join("victim.txt"), "v\n")?;
    let broker = Arc::new(PermissionBroker::new(
        yi_runtime::PermissionMode::Auto,
        cwd.clone(),
        Vec::new(),
        None,
        tokio::sync::broadcast::channel(8).0,
    ));
    let code = r#"
try:
    await yi.tools.bash(command="rm victim.txt")
    print("removed")
except yi.Refused:
    print("refused")
"#;
    let (cells, _) = run_cells(&cwd, &[code], Some(broker), None).await?;
    let cell = cells.first().ok_or("the cell never ended")?;
    let text = yi_types::message::join_text(&cell.content, "\n");
    assert!(text.contains("refused"), "{text}");
    assert!(cwd.join("victim.txt").exists(), "a cell's nested rm ran");
    let rows = yi_context::nested_calls(Some(&cell.details));
    let kinds: Vec<_> = rows.iter().map(|row| row.error_kind.as_deref()).collect();
    assert_eq!(kinds, [Some("refused")]);
    Ok(())
}

/// The execute request the agent's cell holds the kernel as, which `yi.tools` sends as `cell`.
const REQUEST: &str = "request-1";

fn request(name: &str, args: serde_json::Value) -> Map<String, serde_json::Value> {
    let mut payload = Map::new();
    payload.insert("name".to_owned(), json!(name));
    payload.insert("args".to_owned(), args);
    payload.insert("cell".to_owned(), json!(REQUEST));
    payload
}

#[tokio::test]
async fn a_call_outside_a_cell_is_refused_and_reads_past_the_cap_are_counted()
-> Result<(), Box<dyn Error>> {
    let (_dir, cwd) = scratch("kernel-tools-cap")?;
    std::fs::write(cwd.join("a.txt"), "a\n")?;
    let mut session = session(&cwd, &[]);
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
    let mut untagged = read();
    untagged.remove("cell");
    let shut = dispatch(untagged)?.await?;
    assert_eq!(
        shut.get("kind"),
        Some(&json!("cancelled")),
        "a call naming no cell is refused while none is open"
    );
    calls.open(REQUEST);
    let mut untagged = read();
    untagged.remove("cell");
    let refused = dispatch(untagged)?.await?;
    assert_eq!(
        refused.get("kind"),
        Some(&json!("cancelled")),
        "a call naming no cell is refused while one is open"
    );
    let kept = yi_types::kernel::NESTED_READS_KEPT;
    for _ in 0..3 {
        dispatch(read())?.await?;
    }
    for n in 0..kept {
        dispatch(request("read", json!({"path": format!("r{n}.txt")})))?.await?;
    }
    dispatch(request("write", json!({"path": "b.txt", "content": "b\n"})))?.await?;
    let (rows, dropped) = calls.close();
    assert_eq!(
        (rows.len(), dropped),
        (kept + 1, 1),
        "three reads of one file are one row, and the last distinct read is past the cap"
    );
    assert_eq!(
        rows.last().map(|row| row.tool.as_str()),
        Some("write"),
        "a write past the cap is kept"
    );
    let details = json!({"calls": rows, "callsDropped": dropped});
    let cell = serde_json::from_value(json!({"content": [], "details": details}))?;
    let mut ops = yi_context::FileOps::default();
    yi_context::details::extract_file_ops_from_message(&stored(&cell, false), &mut ops);
    let view = yi_context::CompiledView::default();
    let (summary, details) = yi_context::compose_summary("## Goal", &ops, &view);
    let notice = format!(
        "[… {kept} read files listed; 1 more read calls from cells are not, and their paths were not kept: a cell keeps {kept} distinct read rows (NESTED_READS_KEPT); the cells that made them: grep(pattern=\"yi.tools\", path=\"history://self\")]"
    );
    let row = summary.lines().find(|line| line.starts_with("[…"));
    assert_eq!(row, Some(notice.as_str()), "{summary}");
    let again = yi_context::details::extract_file_ops(&[], Some(&details));
    let (resummary, _) = yi_context::compose_summary("## Goal", &again, &view);
    let row = resummary.lines().find(|line| line.starts_with("[…"));
    assert_eq!(
        row,
        Some(notice.as_str()),
        "the next compaction lost the row"
    );
    let after = dispatch(read())?.await?;
    assert_eq!(
        after.get("kind"),
        Some(&json!("cancelled")),
        "closing a cell shuts the gate"
    );

    calls.open(REQUEST);
    let writes = 1024;
    for n in 0..writes {
        let path = format!("w{n}.txt");
        dispatch(request("write", json!({"path": path, "content": "w\n"})))?.await?;
    }
    std::fs::create_dir(cwd.join("taken"))?;
    dispatch(request("write", json!({"path": "taken", "content": "w\n"})))?.await?;
    let (rows, dropped) = calls.close();
    let last = rows.last().ok_or("no rows")?;
    assert_eq!(
        (rows.len(), dropped),
        (writes + 1, 0),
        "a write past a thousand others is kept, not folded or counted as a dropped read"
    );
    assert_eq!(
        (last.tool.as_str(), last.ok, last.paths.clone()),
        ("write", false, vec!["taken".to_owned()]),
        "a failed write past a thousand others keeps its own verdict"
    );

    calls.open(REQUEST);
    let slow = dispatch(request(
        "bash",
        json!({"command": "sleep 0.3; touch late.txt"}),
    ))?;
    let running = tokio::spawn(slow);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let (during, _) = calls.close();
    calls.open("request-2");
    running.await??;
    let (next, _) = calls.close();
    let outlived: Vec<bool> = next.iter().map(|row| row.outlived).collect();
    assert_eq!(
        (during.len(), outlived),
        (0, vec![true]),
        "a call that outlives its cell says so on the next cell already open"
    );
    Ok(())
}

/// The wiring a real session gets: `attach_runtime` registers the verb and hands the ipython
/// tool the same rows, and the stored result is what compaction later reads.
#[tokio::test]
async fn an_attached_session_answers_tool_calls_and_stores_their_rows() -> Result<(), Box<dyn Error>>
{
    let (_dir, cwd) = scratch("kernel-tools-attached")?;
    let provider = Arc::new(ProviderStream::new(None));
    let mut code = Map::new();
    let cell = "await yi.tools.write(path=\"wired.txt\", content=\"w\\n\")";
    code.insert("code".to_owned(), json!(cell));
    provider.queue_faux(vec![
        faux_assistant_message(
            vec![faux_tool_call("call-1", "ipython", code)],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: String::new(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        Arc::clone(&provider),
    );
    let sessions = cwd.join("sessions");
    let _host = yi_runtime::attach_runtime(
        &mut session,
        yi_runtime::RuntimeWiring {
            provider,
            system_prompt: String::new(),
            tool_execution: ExecutionMode::Sequential,
            cwd: cwd.clone(),
            home: home(),
            lane_slots: 1,
            broker: None,
            tools: Arc::new(yi_tools::builtin_tools),
            depth: 0,
            max_depth: 1,
            rlm_dir: sessions.join("rlm-1"),
            family_dir: None,
            summarizer: None,
            advisor: None,
            auto_review: None,
            plan_stale_turns: None,
            plans_dir: Some(cwd.join("plans")),
            parent_link: None,
            wall: yi_runtime::wall::Wall::default(),
            auto_background: None,
            deadline: None,
            kernel_prewarm: false,
            mcp_read: None,
            sessions_dir: Some(sessions),
            kernels: yi_runtime::fetch::KernelServiceMap::new(),
        },
    );
    let store = Arc::new(std::sync::Mutex::new(yi_session::SessionStore::in_memory(
        yi_session::SessionMetadata {
            id: "wired".to_owned(),
            created_at: 0,
            parent_session_id: None,
            name: None,
        },
    )));
    session.attach_store(Arc::clone(&store))?;
    session.prompt("write through the kernel")?;
    session.wait_idle().await;
    if let Some(kernel) = session.kernel_service() {
        kernel.dispose().await;
    }
    assert_eq!(std::fs::read_to_string(cwd.join("wired.txt"))?, "w\n");
    let entries =
        yi_session::lock_session(&store).find_entries(&yi_session::EntryQuery::default())?;
    let stored = (entries.iter()).find_map(|entry| match entry {
        yi_types::entry::Entry::Message {
            message: message @ AgentMessage::ToolResult { tool_name, .. },
            ..
        } if tool_name == "ipython" => Some(message.clone()),
        _ => None,
    });
    let mut ops = yi_context::FileOps::default();
    yi_context::details::extract_file_ops_from_message(&stored.ok_or("no stored cell")?, &mut ops);
    assert!(
        ops.written.contains("wired.txt"),
        "the stored result lists the cell's write: {ops:?}"
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
    assert!(text.contains("2500 2500 3000 3000 True"), "{text}");
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
