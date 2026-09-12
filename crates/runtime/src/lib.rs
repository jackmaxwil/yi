#![forbid(unsafe_code)]

pub mod advisor;
pub mod affordance;
pub mod auto_review;
pub mod checkpoint;
pub mod compaction;
pub mod environment;
pub mod ext;
pub mod family;
pub mod fetch;
pub mod gate;
pub mod goal;
pub mod kernel;
mod kernel_doctor;
mod kernel_variables;
pub mod lane;
pub mod mailbox;
pub mod memory;
pub mod permission;
pub mod plan;
pub mod provider;
pub mod rewind;
pub mod rules;
pub mod schedule;
pub mod schema;
pub mod session;
pub mod skills;
pub mod slash;
pub mod subagent;
pub mod telemetry;
pub mod todo;
pub mod tools;
pub mod wall;
pub mod wiring;

pub use checkpoint::{RecordedCheckpoint, UndoOutcome, recorded, undo, wire_turn_checkpoints};
pub use compaction::{CompactStatus, Compactor};
pub use ext::{ExtOptions, Host as ExtensionHost, Trust, TrustGate};
pub use kernel::{
    HostRegistry, KernelService, KernelServiceOptions, ipython_tool, restore_notice_text,
};
pub use kernel_doctor::{doctor_boot, doctor_toolchain};
pub use mailbox::ParentLink;
pub use permission::{AskOutcome, Asker, PermissionAsk, PermissionBroker};
pub use provider::{
    CATALOG_PROVIDERS, DEFAULT_REFRESH_HOURS, MODELS_DEV, ProviderStream, available_models,
    catalog_age, catalog_cache_dir, catalog_is_stale, catalog_list_url, refresh_catalog,
    resolve_model, set_catalog_cache_dir,
};
pub use rewind::{BranchStub, Rewound, rewind_to, summarize_branch};
pub use session::{AgentSession, SessionConfig, SessionError, Status};
pub use skills::{Skill, skills_catalog};
pub use subagent::{
    ChildBuild, ChildStatus, ChildUpdate, ChildView, SubagentHost, SubagentHostOptions,
};
pub use telemetry::Telemetry;
pub use tools::ToolAdapter;
pub use wall::Wall;
pub use wiring::{RuntimeWiring, attach_runtime};
pub use yi_ai::auth;
pub use yi_ai::request::ProxyConfig;
pub use yi_kernel::bootstrap::{python_root, unpack_embedded_python};

/// None where the platform has no sandbox: a contained decision then degrades
/// to a question rather than to an unenforced allowance.
pub fn workspace_sandbox(
    cwd: &std::path::Path,
    home: &std::path::Path,
    session_dir: &std::path::Path,
) -> Option<yi_tools::Sandbox> {
    yi_tools::Sandbox::available()
        .then(|| yi_tools::Sandbox::for_workspace(cwd, home, Some(session_dir)))
}

pub fn identity_fragment() -> &'static str {
    include_str!("prompts/identity.md")
}

/// Stable text: it rides the cached prefix, so every byte is paid per turn.
pub fn doctrine_fragment() -> &'static str {
    include_str!("prompts/doctrine.md")
}
pub use yi_ai::faux;
pub use yi_context::{Bytes, SourceBudgets, Truncated};
pub use yi_loop::ExecutionMode;
pub use yi_permission::{
    Class, ConfigRule, ConfigRuleAction, Decision, PermissionMode, Verdict, mode_fragment,
};
pub use yi_session as session_store;
pub use yi_tools::{
    Change, ChangeKind, Converter, Documents, builtin_tools, builtin_tools_with,
    discover_exec_tools, edit_file, list_files,
};

pub fn documents(home: &std::path::Path) -> Documents {
    let venv_home = home.to_path_buf();
    Documents {
        home: home.to_path_buf(),
        converter: std::sync::Arc::new(move || {
            let (python, formats) = yi_kernel::bootstrap::document_converter(&venv_home);
            Converter { python, formats }
        }),
        timeout: yi_tools::DEFAULT_TIMEOUT,
    }
}
