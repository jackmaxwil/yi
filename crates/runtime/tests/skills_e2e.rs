use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use yi_loop::ExecutionMode;
use yi_runtime::{
    AgentSession, HostRegistry, KernelService, KernelServiceOptions, ProviderStream, SessionConfig,
};
use yi_tools::{CancelFlag, KernelBridge};
use yi_types::model::{Model, ModelCost};

type TestResult = Result<(), Box<dyn Error>>;

fn vision_faux_model() -> Model {
    let zero = || serde_json::Number::from(0u64);
    Model {
        id: "faux-1".to_owned(),
        name: "Faux".to_owned(),
        api: "faux".to_owned(),
        provider: "faux".to_owned(),
        base_url: "http://localhost:0".to_owned(),
        reasoning: false,
        input: vec!["text".to_owned(), "image".to_owned()],
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

async fn cell(
    service: &Arc<KernelService>,
    code: &'static str,
) -> Result<yi_tools::KernelCellOutcome, String> {
    let service = Arc::clone(service);
    tokio::task::spawn_blocking(move || {
        let cancelled: CancelFlag = Arc::new(|| false);
        KernelBridge::execute_cell(service.as_ref(), code, &cancelled)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tokio::test]
async fn bundled_python_skills_work_through_the_kernel() -> TestResult {
    let provider = Arc::new(ProviderStream::new(None, None));
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: vision_faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );
    session.enable_compaction();
    let compactor = session.compactor().ok_or("no compactor")?;

    let mut registry = HostRegistry::default();
    registry.register_mcp_stubs();
    {
        let compactor = Arc::clone(&compactor);
        registry.register("compact.run", move |payload| {
            let instructions = payload
                .get("instructions")
                .and_then(Value::as_str)
                .map(str::to_owned);
            compactor.schedule_with_instructions(instructions);
            Box::pin(async {
                let mut reply = serde_json::Map::new();
                reply.insert("scheduled".to_owned(), Value::Bool(true));
                Ok(reply)
            })
        });
    }
    {
        let status = session.compact_status_handle().ok_or("no status handle")?;
        registry.register("compact.status", move |_payload| {
            let status = status();
            Box::pin(async move {
                let mut reply = serde_json::Map::new();
                reply.insert("tokens".to_owned(), Value::from(status.tokens));
                reply.insert(
                    "context_window".to_owned(),
                    Value::from(status.context_window),
                );
                reply.insert("percent".to_owned(), Value::from(status.percent));
                reply.insert("scheduled".to_owned(), Value::Bool(status.scheduled));
                Ok(reply)
            })
        });
    }
    let model = session.model();
    registry.register("model.info", move |_payload| {
        let reply = json!({
            "provider": model.provider,
            "id": model.id,
            "name": model.name,
            "selector": format!("{}/{}", model.provider, model.id),
            "input": model.input,
        })
        .as_object()
        .cloned()
        .unwrap_or_default();
        Box::pin(async move { Ok(reply) })
    });

    let service = Arc::new(KernelService::new(KernelServiceOptions {
        cwd: std::env::temp_dir(),
        home: std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default(),
        session_dir: None,
        host: Arc::new(registry),
    }));

    let status_cell = cell(
        &service,
        "s = await compact.status()\nprint(s['context_window'], s['scheduled'])",
    )
    .await
    .map_err(|error| error.to_string())?;
    assert!(
        status_cell.result.stdout.contains("128000 False"),
        "compact.status must surface real usage: {} {}",
        status_cell.result.stdout,
        status_cell.result.stderr
    );

    let run_cell = cell(
        &service,
        "r = await compact.run('keep the failing test names')\nprint(r)",
    )
    .await
    .map_err(|error| error.to_string())?;
    assert!(
        run_cell.result.stdout.contains("'scheduled': True"),
        "compact.run must schedule: {} {}",
        run_cell.result.stdout,
        run_cell.result.stderr
    );
    assert!(
        compactor.scheduled(),
        "a kernel compact.run must set the host compactor pending"
    );

    let attach_cell = cell(
        &service,
        "from PIL import Image\nimport tempfile, os\np = os.path.join(tempfile.gettempdir(), 'yi-skill-test.png')\nImage.new('RGB', (4, 4), 'red').save(p)\nprint(await attach_image(p))",
    )
    .await
    .map_err(|error| error.to_string())?;
    assert!(
        attach_cell
            .result
            .stdout
            .contains("Loaded 1 image(s) into context"),
        "attach_image must confirm the load: {} {}",
        attach_cell.result.stdout,
        attach_cell.result.stderr
    );
    assert_eq!(
        attach_cell.result.attachments.len(),
        1,
        "the display_data attachment must reach the host reducer"
    );
    assert_eq!(attach_cell.result.attachments[0].mime_type, "image/png");

    service.dispose().await;
    Ok(())
}
