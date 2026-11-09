#![forbid(unsafe_code)]
#![deny(clippy::string_slice)]

mod answer;
mod batch;
pub mod config;
mod end;
pub mod interrupt;
pub mod reasoning;
pub mod repair;
pub mod run;
mod stream;
mod stream_retry;
pub mod tool;

pub use config::{ExecutionMode, LoopConfig, LoopGuards, NextTurn, TurnSnapshot};
pub use reasoning::REASONING_CHAR_CAP;
pub use run::{
    CUT_REDRIVE_TEXT, CUT_STOP_AT, LENGTH_REDRIVE_CUSTOM_TYPE, LENGTH_REDRIVE_TEXT, LENGTH_STOP_AT,
    LoopContext, REPEAT_BREAK_CUSTOM_TYPE, REPEAT_STEER_AT, REPEAT_STOP_AT, run_loop,
    synthesized_error_message,
};
pub use stream_retry::STREAM_RETRY_CUSTOM_TYPE;
pub use tool::{AgentTool, ProgramFuture, ToolOutcome};
