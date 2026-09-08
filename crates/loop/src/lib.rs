#![forbid(unsafe_code)]
#![deny(clippy::string_slice)]

pub mod config;
pub mod interrupt;
pub mod repair;
pub mod run;
pub mod tool;

pub use config::{ExecutionMode, LoopConfig, NextTurn, TurnSnapshot};
pub use repair::repair_tool_name;
pub use run::{
    LENGTH_REDRIVE_CUSTOM_TYPE, LENGTH_REDRIVE_TEXT, LoopContext, REPEAT_BREAK_CUSTOM_TYPE,
    REPEAT_BREAK_TEXT, REPEAT_STEER_AT, REPEAT_STOP_AT, run_loop,
};
pub use tool::{AgentTool, ToolOutcome};
