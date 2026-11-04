//! The root-session wiring `yi` builds, so a kernel test reaches its cell through the same
//! `attach_runtime` a real session gets rather than a hand-rolled service.

use std::path::PathBuf;
use std::sync::Arc;

/// A root session wired the way `yi` wires one, with `sessions_dir` as its session corpus.
pub(crate) fn root_session(
    project: &std::path::Path,
    home: &std::path::Path,
    rlm_dir: &std::path::Path,
    sessions_dir: Option<PathBuf>,
    broker: Option<Arc<yi_runtime::permission::PermissionBroker>>,
    mcp_read: Option<Arc<dyn yi_runtime::fetch::McpResourceRead>>,
) -> yi_runtime::AgentSession {
    let wall = yi_runtime::Wall::default();
    walled_session(project, home, rlm_dir, sessions_dir, broker, mcp_read, wall)
}

/// The same wiring under another wall, for a juror or a reader.
pub(crate) fn walled_session(
    project: &std::path::Path,
    home: &std::path::Path,
    rlm_dir: &std::path::Path,
    sessions_dir: Option<PathBuf>,
    broker: Option<Arc<yi_runtime::permission::PermissionBroker>>,
    mcp_read: Option<Arc<dyn yi_runtime::fetch::McpResourceRead>>,
    wall: yi_runtime::Wall,
) -> yi_runtime::AgentSession {
    let provider = Arc::new(yi_runtime::ProviderStream::new(None));
    let mut session = yi_runtime::AgentSession::new(
        yi_runtime::SessionConfig {
            system_prompt: String::new(),
            model: crate::support::faux_model(),
            thinking_level: None,
            tool_execution: yi_loop::ExecutionMode::Sequential,
        },
        Arc::clone(&provider),
    );
    yi_runtime::attach_runtime(
        &mut session,
        yi_runtime::RuntimeWiring {
            provider,
            system_prompt: String::new(),
            tool_execution: yi_loop::ExecutionMode::Sequential,
            cwd: project.to_path_buf(),
            home: home.to_path_buf(),
            lane_slots: 1,
            broker,
            tools: Arc::new(yi_tools::builtin_tools),
            depth: 0,
            max_depth: 1,
            rlm_dir: rlm_dir.to_path_buf(),
            family_dir: None,
            summarizer: None,
            advisor: None,
            auto_review: None,
            plan_stale_turns: None,
            plans_dir: Some(rlm_dir.join("plans")),
            parent_link: None,
            wall,
            auto_background: None,
            deadline: None,
            kernel_prewarm: false,
            mcp_read,
            sessions_dir,
            kernels: yi_runtime::fetch::KernelServiceMap::new(),
        },
    );
    session
}
