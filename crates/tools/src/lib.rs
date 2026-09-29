#![forbid(unsafe_code)]

mod builtins;
pub mod checkpoint;
pub mod diff;
mod document;
mod exec;
mod grep;
pub mod hashline;
mod ignore;
mod ipython;
pub mod jobs;
mod orient;
mod process;
pub mod reduce;
pub mod sandbox;
#[cfg(test)]
#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
pub mod spill;
mod syntax;
mod tool;

use std::sync::Arc;

pub use builtins::{BashTool, WriteTool, list_files, wall_refusal};
pub use checkpoint::{Change, ChangeKind, CheckpointError, Checkpoints, TreeId};
pub use diff::{GitPatch, patch};
pub use document::{Converter, DEFAULT_TIMEOUT, Documents, document_ceiling};
pub use exec::{ExecTool, discover_exec_tools};
pub use grep::GrepTool;
pub use ipython::cell_output;
pub use ipython::{CellSpill, IpythonTool, KernelBridge, KernelCellOutcome};
pub use jobs::{JobId, JobReport, MAX_TIMEOUT_SECS, Run, clamp_timeout, run_or_background};
pub use orient::GetContextTool;
pub use process::{CommandCapture, OUTPUT_CAP, command, edit_file, run_captured};
pub use reduce::{Reduced, reduce};
pub use sandbox::{Sandbox, SandboxRefusal, denial_hint, sandbox_refusal};
pub use tool::{
    CancelFlag, DETAIL_CAP, Tool, ToolContext, ToolKind, ToolOutput, error_output,
    error_output_kind, text_output,
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
