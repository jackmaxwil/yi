#![forbid(unsafe_code)]

pub mod advisor;
pub mod compaction;
pub mod goal;
pub mod kernel;
pub mod permission;
pub mod provider;
pub mod schedule;
pub mod session;
pub mod subagent;
pub mod tools;

pub use compaction::{CompactStatus, Compactor};
pub use kernel::{
    HostRegistry, KernelService, KernelServiceOptions, ipython_tool, restore_notice_text,
};
pub use permission::{AskOutcome, Asker, PermissionBroker};
pub use provider::{ProviderStream, available_models, resolve_model};
pub use session::{AgentSession, SessionConfig, SessionError, Status};
pub use subagent::{
    ChildStatus, ChildView, RuntimeWiring, SubagentHost, SubagentHostOptions, attach_runtime,
};
pub use tools::ToolAdapter;
pub use yi_ai::auth;

/// The base identity fragment every Yi session leads with (operating
/// doctrine is a later, separate fragment).
pub fn identity_fragment() -> &'static str {
    include_str!("prompts/identity.md")
}
pub use yi_ai::faux;
pub use yi_loop::ExecutionMode;
pub use yi_permission::{ConfigRule, ConfigRuleAction, PermissionMode, mode_fragment};
pub use yi_session as session_store;
pub use yi_tools::{builtin_tools, discover_exec_tools};
