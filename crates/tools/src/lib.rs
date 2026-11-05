#![forbid(unsafe_code)]
#![deny(clippy::string_slice)]
mod builtins;
pub mod checkpoint;
pub mod diff;
mod document;
mod exec;
mod grep;
pub mod hashline;
pub mod ignore;
mod ipython;
pub mod jobs;
mod orient;
pub mod plugin;
#[cfg(test)]
#[path = "../../types/tests/support/poll.rs"]
mod poll;
mod process;
mod redact;
pub mod reduce;
pub mod ripwire;
pub mod sandbox;
#[cfg(test)]
#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
mod spawn;
mod spill;
mod steer;
mod syntax;
mod tool;
mod values;

use std::sync::Arc;

pub use builtins::{BashTool, WriteTool, list_files, wall_refusal, walled};
pub use checkpoint::{Change, ChangeKind, CheckpointError, Checkpoints, TreeId};
pub use diff::{GitPatch, patch};
pub use document::{Converter, DEFAULT_TIMEOUT, Documents, document_ceiling};
pub use exec::{SkippedExecTool, discover_exec_tools};
pub use grep::{GrepTool, line_matcher};
pub use ipython::{CellCalls, CellSpill, IpythonTool, KernelBridge, KernelCellOutcome};
pub use ipython::{cell_output, shell_cell};
pub use jobs::{JobId, JobReport, MAX_TIMEOUT_SECS, Run, run_or_background};
pub use orient::{GUIDE as GET_CONTEXT_GUIDE, GetContextTool};
pub use process::{
    CommandCapture, HostRefused, OUTPUT_CAP, command, edit_file, git_root, host_command,
    keyless_command, run_captured, run_captured_until, spill_path,
};
pub use redact::Redactor;
pub use reduce::{Involuntary, Reduced, VIEW_CEILING, reduce, rerun, view};
pub use sandbox::{
    Sandbox, SandboxRefusal, SealBroken, denial_hint, exit_zero_note, kernel_cell_note,
    nested_sandbox_note, reseal, sandbox_refusal, set_wrapper, verify_seal, writes_sealed,
};
pub use spawn::{FirstUse, Spawn, SpawnFn, SpawnRefused, forget_first_use, register_first_use};
/// The spill root's name under `~/.yi`, one dir per session inside it, before spills moved into
/// the family board; walled sessions still may not read it (D340).
pub const SPILLS: &str = "spills";
/// The dir beside it every session shared before spills were per session (D316).
pub const FLAT_SPILLS: &str = "tool-output";
pub use tool::{
    CancelFlag, DETAIL_CAP, FileChange, Fuse, Tool, ToolContext, ToolKind, ToolOutput, cut_row,
    error_output, error_output_kind, glob_head, require_str, text_output,
};

pub fn builtin_tools() -> Vec<Arc<dyn Tool>> {
    builtin_tools_with(false, None)
}

/// `freeform_grammar` opts the edit tool into [`Tool::freeform`]: a grammar
/// the provider rejects fails every request carrying the tool, not just edits.
pub fn builtin_tools_with(
    freeform_grammar: bool,
    documents: Option<Documents>,
) -> Vec<Arc<dyn Tool>> {
    let state = hashline::tool::shared_hashline_state();
    hashline::tool::lock_state(&state).documents = documents;
    vec![
        Arc::new(hashline::tool::HashlineReadTool::new(Arc::clone(&state))),
        Arc::new(hashline::tool::HashlineEditTool {
            state: Arc::clone(&state),
            freeform_grammar,
        }),
        Arc::new(WriteTool {
            hashline: Some(Arc::clone(&state)),
        }),
        Arc::new(GrepTool {
            hashline: Some(Arc::clone(&state)),
        }),
        Arc::new(BashTool {
            hashline: Some(state),
            ..Default::default()
        }),
        Arc::new(GetContextTool),
    ]
}
