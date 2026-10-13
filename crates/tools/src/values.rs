use serde_json::Value;
use yi_types::tool_value::{BashValue, WriteValue};

use crate::hashline::format::FileTag;
use crate::jobs::JobId;
use crate::process::CommandCapture;

pub(crate) fn write(path: &str, tag: Option<FileTag>, bytes: usize) -> Value {
    let value = WriteValue {
        path: path.to_owned(),
        tag: tag.map(|tag| tag.to_string()),
        bytes: u64::try_from(bytes).unwrap_or(u64::MAX),
    };
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// The streams before the reducer shortened them, and the row naming any cap that cut them.
pub(crate) fn bash(capture: CommandCapture, timed_out: bool) -> Value {
    let rows: Vec<String> = (capture.cut_row().into_iter())
        .chain(capture.cut_note())
        .collect();
    let value = BashValue {
        exit_code: capture.exit_code.map(i64::from),
        stdout: capture.stdout,
        stderr: capture.stderr,
        timed_out,
        cancelled: capture.cancelled,
        cut: (!rows.is_empty()).then(|| rows.join("\n")),
        job: None,
    };
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// A command that outlived its wait: the job a program polls with `bash()` and no command.
pub(crate) fn job(id: JobId) -> Value {
    let value = BashValue {
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        timed_out: false,
        cancelled: false,
        cut: None,
        job: Some(id.0),
    };
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// A finished job a program polled: its output arrives as one interleaved stream, kept whole.
pub(crate) fn polled(id: JobId, exit_code: Option<i32>, output: &str, killed: bool) -> Value {
    let value = BashValue {
        exit_code: exit_code.map(i64::from),
        stdout: output.to_owned(),
        stderr: String::new(),
        timed_out: false,
        cancelled: killed,
        cut: None,
        job: Some(id.0),
    };
    serde_json::to_value(value).unwrap_or(Value::Null)
}
