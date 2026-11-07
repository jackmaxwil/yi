//! The root-session wiring `yi` builds, so a kernel test reaches its cell through the same
//! `attach_runtime` a real session gets rather than a hand-rolled service.

use std::path::PathBuf;
use std::sync::Arc;

/// A session wired the way `yi` wires one: the corpus it reads, the wall it runs under, and,
/// for a child, the board its spawn was beside.
pub(crate) struct Wired<'a> {
    pub(crate) project: &'a std::path::Path,
    pub(crate) home: &'a std::path::Path,
    pub(crate) rlm: &'a std::path::Path,
    pub(crate) corpus: Option<PathBuf>,
    pub(crate) broker: Option<Arc<yi_runtime::permission::PermissionBroker>>,
    pub(crate) mcp_read: Option<Arc<dyn yi_runtime::fetch::McpResourceRead>>,
    pub(crate) wall: yi_runtime::Wall,
    pub(crate) board: Option<PathBuf>,
    pub(crate) link: Option<yi_runtime::ParentLink>,
    pub(crate) kernels: Option<Arc<yi_runtime::fetch::KernelServiceMap>>,
}

impl Default for Wired<'_> {
    fn default() -> Self {
        Self {
            project: std::path::Path::new(""),
            home: std::path::Path::new(""),
            rlm: std::path::Path::new(""),
            corpus: None,
            broker: None,
            mcp_read: None,
            wall: yi_runtime::Wall::default(),
            board: None,
            link: None,
            kernels: None,
        }
    }
}

/// A root session wired the way `yi` wires one, with `sessions_dir` as its session corpus.
pub(crate) fn root_session(
    project: &std::path::Path,
    home: &std::path::Path,
    rlm_dir: &std::path::Path,
    sessions_dir: Option<PathBuf>,
    broker: Option<Arc<yi_runtime::permission::PermissionBroker>>,
    mcp_read: Option<Arc<dyn yi_runtime::fetch::McpResourceRead>>,
) -> yi_runtime::AgentSession {
    walled_session(Wired {
        project,
        home,
        rlm: rlm_dir,
        corpus: sessions_dir,
        broker,
        mcp_read,
        ..Wired::default()
    })
}

/// The same wiring under another wall or beside a family board, for a juror, reader or child.
pub(crate) fn walled_session(w: Wired<'_>) -> yi_runtime::AgentSession {
    let depth = if w.link.is_some() { 1 } else { 0 };
    let provider = Arc::new(yi_runtime::ProviderStream::new(None));
    let mut session = yi_runtime::AgentSession::new(
        yi_runtime::SessionConfig {
            system_prompt: String::new(),
            model: crate::support::faux_model(128_000),
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
            cwd: w.project.to_path_buf(),
            home: w.home.to_path_buf(),
            lane_slots: 1,
            broker: w.broker,
            tools: Arc::new(yi_tools::builtin_tools),
            depth,
            max_depth: 2,
            rlm_dir: w.rlm.to_path_buf(),
            family_dir: w.board,
            summarizer: None,
            advisor: None,
            auto_review: None,
            plan_stale_turns: None,
            plans_dir: Some(w.rlm.join("plans")),
            parent_link: w.link,
            wall: w.wall,
            auto_background: None,
            deadline: None,
            kernel_prewarm: false,
            mcp_read: w.mcp_read,
            sessions_dir: w.corpus,
            kernels: w.kernels.unwrap_or_default(),
        },
    );
    session
}
