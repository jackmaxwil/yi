use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use yi_runtime::AgentSession;

use crate::{INTERNAL_ERROR, INVALID_PARAMS};

pub(crate) fn root(session: &AgentSession, cwd: &Path) -> PathBuf {
    session
        .lane()
        .and_then(|lane| lane.path())
        .unwrap_or_else(|| cwd.to_path_buf())
}

pub(crate) fn why(session: &AgentSession, cwd: &Path, params: &Value) -> Value {
    let lines: Vec<u32> = params
        .get("lines")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|line| u32::try_from(line.as_u64()?).ok())
        .collect();
    let path = params.get("path").and_then(Value::as_str).unwrap_or("");
    why_answers(&root(session, cwd), path, &lines)
}

/// A call answers at most [`WHY_LINES`]; the lines past it come back as `unasked`, so the
/// client can name the cut and `yi why <path>:<line>` for the rest.
pub fn why_answers(root: &Path, path: &str, lines: &[u32]) -> Value {
    let plans = root.join(yi_runtime::plan::PLANS_DIR);
    let (asked, unasked) = lines.split_at(lines.len().min(WHY_LINES));
    let answers: Vec<Value> = asked
        .iter()
        .map(
            |&line| match yi_runtime::plan::why::answer(root, &plans, path, line) {
                Ok(found) => json!({
                    "line": line,
                    "commit": found.commit.get(..12).unwrap_or(&found.commit),
                    "subject": found.subject,
                    "todo": found.todo,
                    "goal": found.goal,
                }),
                Err(yi_runtime::plan::why::WhyError::Uncommitted { .. }) => {
                    json!({"line": line, "uncommitted": true})
                }
                Err(error) => json!({"line": line, "error": error.to_string()}),
            },
        )
        .collect();
    json!({"path": path, "answers": answers, "cap": WHY_LINES, "unasked": unasked})
}

pub const WHY_LINES: usize = 8;

pub(crate) fn restore_before(
    session: &AgentSession,
    entry_id: &str,
    cwd: &Path,
) -> Result<(yi_runtime::Rewound, String), (i64, String)> {
    if session.status() == yi_runtime::Status::Running {
        return Err((
            INVALID_PARAMS,
            "the current turn is still running".to_owned(),
        ));
    }
    let store = session.store().ok_or((
        INVALID_PARAMS,
        "no store to read checkpoints from".to_owned(),
    ))?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let mut rewound = None;
    let rewind = || {
        rewound = Some(yi_runtime::rewind_to(session, entry_id)?);
        Ok(())
    };
    match yi_runtime::undo_to(&store, entry_id, &root(session, cwd), &home, rewind) {
        yi_runtime::UndoOutcome::Restored { changes, scoped } => rewound
            .map(|rewound| (rewound, yi_runtime::describe_undo(&changes, scoped)))
            .ok_or((
                INTERNAL_ERROR,
                "the restore ran without its rewind".to_owned(),
            )),
        yi_runtime::UndoOutcome::NoCheckpoint => Err((
            INVALID_PARAMS,
            "no checkpoint holds the files from before that message".to_owned(),
        )),
        yi_runtime::UndoOutcome::Failed(error) => Err((INTERNAL_ERROR, error)),
    }
}

pub(crate) fn undo_text(session: &AgentSession, cwd: &std::path::Path) -> String {
    if session.status() == yi_runtime::Status::Running {
        return "/undo: the current turn is still running (esc stops it)".to_owned();
    }
    let Some(store) = session.store() else {
        return "/undo: this session has no store to read checkpoints from".to_owned();
    };
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    match yi_runtime::undo(&store, cwd, &home) {
        yi_runtime::UndoOutcome::Restored { changes, scoped } => {
            format!("/undo: {}", yi_runtime::describe_undo(&changes, scoped))
        }
        yi_runtime::UndoOutcome::NoCheckpoint => "/undo: no checkpoint to restore".to_owned(),
        yi_runtime::UndoOutcome::Failed(error) => format!("/undo: {error}"),
    }
}
