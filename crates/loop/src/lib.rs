#![forbid(unsafe_code)]
#![deny(clippy::string_slice)]

mod answer;
pub mod config;
pub mod interrupt;
pub mod reasoning;
pub mod repair;
pub mod run;
pub mod tool;

pub use config::{ExecutionMode, LoopConfig, LoopGuards, NextTurn, TurnSnapshot};
pub use reasoning::REASONING_CHAR_CAP;
pub use run::{
    CUT_REDRIVE_TEXT, CUT_STOP_AT, LENGTH_REDRIVE_CUSTOM_TYPE, LENGTH_REDRIVE_TEXT, LENGTH_STOP_AT,
    LoopContext, REPEAT_BREAK_CUSTOM_TYPE, REPEAT_STEER_AT, REPEAT_STOP_AT,
    STREAM_RETRY_CUSTOM_TYPE, run_loop, synthesized_error_message,
};
pub use tool::{AgentTool, ProgramFuture, ToolOutcome};
