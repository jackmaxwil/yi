#![forbid(unsafe_code)]

mod builtins;
mod exec;
pub mod hashline;
mod process;
mod tool;

use std::sync::Arc;

pub use builtins::{BashTool, GlobTool, GrepTool, ReadTool, WriteTool};
pub use exec::{ExecTool, discover_exec_tools};
pub use process::{CommandCapture, OUTPUT_CAP, run_captured};
pub use tool::{CancelFlag, Tool, ToolContext, ToolKind, ToolOutput, error_output, text_output};

pub fn builtin_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(ReadTool),
        Arc::new(WriteTool),
        Arc::new(GlobTool),
        Arc::new(GrepTool),
        Arc::new(BashTool),
    ]
}
