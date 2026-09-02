use std::error::Error;
use std::sync::Arc;

use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_loop::ExecutionMode;
use yi_runtime::{AgentSession, ProviderStream, SessionConfig, Wall};
use yi_types::event::AgentEvent;
use yi_types::message::{Content, StopReason};
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

async fn run_denied_command(wall: Wall, command: String) -> Result<(String, bool), Box<dyn Error>> {
    let mut args = serde_json::Map::new();
    args.insert("command".to_owned(), serde_json::json!(command));
    run_walled_tool(wall, "bash", args, std::env::temp_dir()).await
}

async fn run_walled_tool(
    wall: Wall,
    tool: &str,
    args: serde_json::Map<String, serde_json::Value>,
    cwd: std::path::PathBuf,
) -> Result<(String, bool), Box<dyn Error>> {
    let provider = Arc::new(ProviderStream::new(None, None));
    provider.queue_faux(vec![
        faux_assistant_message(
            vec![faux_tool_call("call-1", tool, args)],
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
    session.set_wall(wall);
    session.use_tools(yi_tools::builtin_tools(), cwd, None);
    let mut events = session.subscribe();
    session.prompt("go")?;
    session.wait_idle().await;
    let mut seen = None;
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::ToolExecutionEnd {
            result, is_error, ..
        } = event
        {
            let text = result
                .content
                .iter()
                .map(|content| match content {
                    Content::Text { text, .. } => text.clone(),
                    _ => String::new(),
                })
                .collect::<String>();
            seen = Some((text, is_error));
        }
    }
    seen.ok_or_else(|| "no tool result observed".into())
}

#[tokio::test]
async fn the_wall_denies_a_write_to_the_instrument_before_it_runs() -> TestResult {
    let instrument = std::env::temp_dir().join(format!("yi-wall-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&instrument);
    std::fs::create_dir_all(&instrument)?;
    let marker = instrument.join("cases.json");
    let wall = Wall {
        deny_write: vec![instrument.clone()],
        deny_read: Vec::new(),
        deny_url: Vec::new(),
    };

    let (denial, is_error) =
        run_denied_command(wall.clone(), format!("echo relaxed > {}", marker.display())).await?;
    assert!(is_error, "a walled path must be refused");
    assert!(
        denial.contains("Denied by the reviewer wall")
            && denial.contains(&instrument.display().to_string()),
        "the denial names the path that is off limits: {denial}"
    );
    assert!(
        !marker.exists(),
        "the command must never have run: the wall sits before execution"
    );

    let (allowed, is_error) = run_denied_command(wall, "echo untouched".to_owned()).await?;
    assert!(
        !is_error && allowed.contains("untouched"),
        "work outside the wall is untouched by it: {allowed}"
    );
    let _ = std::fs::remove_dir_all(&instrument);
    Ok(())
}

/// The orientation packet names no path in its arguments, so the wall cannot
/// refuse it from the call: the tool has to consult the deny set itself.
#[tokio::test]
async fn a_read_deny_keeps_the_orientation_packet_out_of_the_denied_tree() -> TestResult {
    let root = std::env::temp_dir().join(format!("yi-wall-orient-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("secret"))?;
    std::fs::write(root.join("open.rs"), "pub fn open_declaration() {}\n")?;
    std::fs::write(
        root.join("secret/hidden.rs"),
        "pub fn hidden_declaration() {}\n",
    )?;
    let wall = Wall {
        deny_write: Vec::new(),
        deny_read: vec![root.join("secret")],
        deny_url: Vec::new(),
    };

    let (packet, is_error) =
        run_walled_tool(wall, "get_context", serde_json::Map::new(), root.clone()).await?;
    assert!(
        !is_error,
        "the packet still answers outside the deny: {packet}"
    );
    assert!(
        packet.contains("open_declaration"),
        "a deny narrows the packet, it does not empty it: {packet}"
    );
    assert!(
        !packet.contains("hidden_declaration"),
        "a deny_read child must not read declarations out of the denied tree: {packet}"
    );
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

#[test]
fn a_read_deny_binds_reads_and_a_write_deny_does_not() -> TestResult {
    let root = std::env::temp_dir().join("yi-wall-scope");
    let mut args = serde_json::Map::new();
    args.insert(
        "path".to_owned(),
        serde_json::json!(root.join("cases.json").display().to_string()),
    );
    let write_only = Wall {
        deny_write: vec![root.clone()],
        deny_read: Vec::new(),
        deny_url: Vec::new(),
    };
    assert!(
        write_only
            .check("read", yi_tools::ToolKind::Read, &args, &root)
            .is_none(),
        "a write deny leaves reading the standard open — the reviewer still needs it"
    );
    assert!(
        write_only
            .check("write", yi_tools::ToolKind::Write, &args, &root)
            .is_some()
    );
    let read_too = Wall {
        deny_write: Vec::new(),
        deny_read: vec![root.clone()],
        deny_url: Vec::new(),
    };
    assert!(
        read_too
            .check("read", yi_tools::ToolKind::Read, &args, &root)
            .is_some(),
        "the sampled-instrument case opts reads in"
    );
    assert!(
        read_too
            .check("write", yi_tools::ToolKind::Write, &args, &root)
            .is_some(),
        "a path hidden from a child is not writable by it either"
    );
    Ok(())
}

#[test]
fn a_spawn_declares_url_denies_and_the_child_wall_carries_them() -> TestResult {
    let root = std::env::temp_dir().join("yi-wall-url");
    let kwargs: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(r#"{"deny_url": ["kernel://", "plan://secret-cut"]}"#)?;
    let wall = Wall::from_kwargs(&kwargs, &root)?;
    assert_eq!(wall.deny_url, vec!["kernel://", "plan://secret-cut"]);
    let walled: yi_types::url::Url = "kernel://main/answers".parse()?;
    assert!(
        wall.check_url(&walled, &root).is_some(),
        "a bare scheme prefix walls the whole scheme"
    );
    let open: yi_types::url::Url = "plan://another-cut/step".parse()?;
    assert!(wall.check_url(&open, &root).is_none());
    // Incident: only `local://` mapped onto deny_read, so a walled path stayed
    // readable as of any checkpoint tree.
    let read_walled: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(r#"{"deny_read": ["secret"]}"#)?;
    let wall = Wall::from_kwargs(&read_walled, &root)?;
    let tree = "a".repeat(40);
    let as_of: yi_types::url::Url = format!("checkpoint://{tree}/secret/key.txt").parse()?;
    assert!(
        wall.check_url(&as_of, &root).is_some(),
        "a read-walled path is walled as of every checkpoint tree too"
    );
    let elsewhere: yi_types::url::Url = format!("checkpoint://{tree}/src/lib.rs").parse()?;
    assert!(wall.check_url(&elsewhere, &root).is_none());
    let not_a_list: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(r#"{"deny_url": "kernel://"}"#)?;
    assert!(
        Wall::from_kwargs(&not_a_list, &root).is_err(),
        "a deny that is not a list is refused, not silently ignored"
    );
    Ok(())
}
