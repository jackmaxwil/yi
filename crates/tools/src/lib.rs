#![forbid(unsafe_code)]

mod builtins;
mod exec;
pub mod hashline;
mod ignore;
mod ipython;
mod process;
mod tool;

use std::sync::Arc;

pub use builtins::{BashTool, GlobTool, GrepTool, WriteTool, list_files};
pub use exec::{ExecTool, discover_exec_tools};
pub use ipython::{IpythonTool, KernelBridge, KernelCellOutcome};
pub use process::{CommandCapture, OUTPUT_CAP, edit_file, run_captured};
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
