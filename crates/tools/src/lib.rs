#![forbid(unsafe_code)]

mod builtins;
pub mod checkpoint;
pub mod diff;
mod exec;
pub mod hashline;
mod ignore;
mod ipython;
pub mod jobs;
mod process;
pub mod reduce;
mod tool;

use std::sync::Arc;

pub use builtins::{BashTool, GlobTool, GrepTool, WriteTool, list_files};
pub use checkpoint::{Change, ChangeKind, CheckpointError, Checkpoints, TreeId};
pub use diff::{GitPatch, patch};
pub use exec::{ExecTool, discover_exec_tools};
pub use ipython::{IpythonTool, KernelBridge, KernelCellOutcome};
pub use jobs::{JobId, JobReport, Run, run_or_background};
pub use process::{CommandCapture, OUTPUT_CAP, command, edit_file, run_captured};
pub use reduce::{Reduced, reduce};
pub use tool::{CancelFlag, Tool, ToolContext, ToolKind, ToolOutput, error_output, text_output};

pub fn builtin_tools() -> Vec<Arc<dyn Tool>> {
    let state = hashline::tool::shared_hashline_state();
    vec![
        Arc::new(hashline::tool::HashlineReadTool {
            state: Arc::clone(&state),
        }),
        Arc::new(hashline::tool::HashlineEditTool {
            state: Arc::clone(&state),
        }),
        Arc::new(WriteTool {
            hashline: Some(state),
        }),
        Arc::new(GlobTool),
        Arc::new(GrepTool),
        Arc::new(BashTool),
    ]
}
