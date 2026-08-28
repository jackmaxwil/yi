#![forbid(unsafe_code)]

pub mod advisor;
pub mod checkpoint;
pub mod compaction;
pub mod goal;
pub mod kernel;
pub mod permission;
pub mod plan;
pub mod provider;
pub mod rewind;
pub mod rules;
pub mod schedule;
pub mod schema;
pub mod session;
pub mod skills;
pub mod subagent;
pub mod tools;
pub mod worktree;

pub use checkpoint::{RecordedCheckpoint, UndoOutcome, recorded, undo, wire_turn_checkpoints};
pub use compaction::{CompactStatus, Compactor};
pub use kernel::{
    HostRegistry, KernelService, KernelServiceOptions, ipython_tool, restore_notice_text,
};
pub use permission::{AskOutcome, Asker, PermissionAsk, PermissionBroker};
pub use provider::{ProviderStream, available_models, resolve_model};
pub use rewind::{Rewound, rewind_to};
pub use session::{AgentSession, SessionConfig, SessionError, Status};
pub use skills::{Skill, skills_catalog};
pub use subagent::{
    ChildBuild, ChildStatus, ChildUpdate, ChildView, RuntimeWiring, SubagentHost,
    SubagentHostOptions, attach_runtime,
};
pub use tools::ToolAdapter;
pub use yi_ai::auth;

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
pub use yi_permission::{ConfigRule, ConfigRuleAction, PermissionMode, mode_fragment};
pub use yi_session as session_store;
pub use yi_tools::{Change, ChangeKind, builtin_tools, discover_exec_tools, edit_file, list_files};
