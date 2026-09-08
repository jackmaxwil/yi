//! `kernel://<child>/var` is a parent reading a namespace that is not its own,
//! so the proof is one map shared by two sessions the wiring built.
use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_loop::ExecutionMode;
use yi_runtime::fetch::{KernelServiceMap, Resolver};
use yi_runtime::{
    AgentSession, ProviderStream, RuntimeWiring, SessionConfig, SubagentHost, Wall, attach_runtime,
};
use yi_types::message::StopReason;
use yi_types::model::{Model, ModelCost};

type TestResult = Result<(), Box<dyn Error>>;

const POLL_ATTEMPTS: usize = 400;
const POLL_INTERVAL_MS: u64 = 50;

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

async fn wait_for_status(host: &Arc<SubagentHost>, name: &str, status: &str) -> bool {
    for _ in 0..POLL_ATTEMPTS {
        let listed = host.list();
        if let Some(Value::Array(children)) = listed.get("subagents")
            && children.iter().any(|child| {
                child.get("session_name").and_then(Value::as_str) == Some(name)
                    && child.get("status").and_then(Value::as_str) == Some(status)
            })
        {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    false
}

#[tokio::test]
async fn a_parent_reads_a_variable_out_of_its_childs_kernel() -> TestResult {
    let root = std::env::temp_dir().join(format!("yi-kernel-across-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root)?;
    let provider = Arc::new(ProviderStream::new(None, None));
    let mut code = Map::new();
    code.insert("code".to_owned(), Value::String("answer = 42".to_owned()));
    provider.queue_faux(vec![
        faux_assistant_message(
            vec![faux_tool_call("call-1", "ipython", code)],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("bound it")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("bound it")], StopReason::Stop),
    ]);
    let mut parent = AgentSession::new(
        SessionConfig {
            system_prompt: String::new(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        Arc::clone(&provider),
    );
    let kernels = KernelServiceMap::new();
    let host = attach_runtime(
        &mut parent,
        RuntimeWiring {
            provider: Arc::clone(&provider),
            system_prompt: String::new(),
            tool_execution: ExecutionMode::Sequential,
            cwd: root.clone(),
            lane_slots: 1,
            deadline: None,
            home: std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default(),
            broker: None,
            tools: Arc::new(yi_tools::builtin_tools),
            depth: 0,
            max_depth: 1,
            rlm_dir: root.join("rlm"),
            summarizer: None,
            advisor: None,
            auto_review: None,
            plan_stale_turns: None,
            plans_dir: Some(root.join("plans")),
            parent_link: None,
            wall: Wall::default(),
            auto_background: None,
            kernel_prewarm: false,
            mcp_read: None,
            sessions_dir: None,
            kernels: Arc::clone(&kernels),
        },
    );

    let mut kwargs = Map::new();
    kwargs.insert("name".to_owned(), Value::String("helper".to_owned()));
    host.spawn("bind the answer".to_owned(), kwargs)?;
    assert!(
        wait_for_status(&host, "helper", "completed").await,
        "the child never finished its cell: {:?}",
        host.list()
    );

    let resolver = Resolver::new(root.clone(), Wall::default()).with_kernel_variables(kernels);
    let url: yi_types::url::Url = "kernel://helper/answer".parse()?;
    let fetched = tokio::task::spawn_blocking(move || resolver.fetch(&url)).await??;
    assert_eq!(fetched.text, "42");
    assert_eq!(fetched.served_by, "kernel-namespace helper");
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}
