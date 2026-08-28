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
    let provider = Arc::new(ProviderStream::new(None, None));
    let mut args = serde_json::Map::new();
    args.insert("command".to_owned(), serde_json::json!(command));
    provider.queue_faux(vec![
        faux_assistant_message(
            vec![faux_tool_call("call-1", "bash", args)],
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
    session.use_tools(yi_tools::builtin_tools(), std::env::temp_dir(), None);
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
