use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use serde_json::Value;
use yi_tools::CancelFlag;

use super::config::Handler;

/// Per stream: a cut stdout carries the drain's marker mid-text, so its JSON never parses.
const HOOK_OUTPUT_CAP: usize = 1024 * 1024;

/// What one handler's run said, by the doc's table of stdout against exit code.
pub(super) enum Said {
    Quiet,
    Json(Value),
    Blocked {
        reason: String,
        json: Option<Value>,
        cut: Option<String>,
    },
    Notice(String),
}

/// The hook inherits Yi's environment as Claude's inherits Claude's: provider keys go only under
/// `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1`, and the `OTEL_*` exporter variables always go.
fn spawned(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let mut process = match std::env::var("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB").as_deref() {
        Ok("1") => yi_tools::keyless_command(program),
        _ => yi_tools::command(program),
    };
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("OTEL_") {
            process.env_remove(name);
        }
    }
    process
}

fn command(handler: &Handler, project: &Path) -> std::process::Command {
    let root = handler.plugin_root.as_deref();
    let placed = |text: &str| {
        let text = text.replace("${CLAUDE_PROJECT_DIR}", &project.to_string_lossy());
        match root {
            Some(root) => text.replace("${CLAUDE_PLUGIN_ROOT}", &root.to_string_lossy()),
            None => text,
        }
    };
    let mut process = match &handler.args {
        Some(args) => {
            let mut process = spawned(placed(&handler.command));
            process.args(args.iter().map(|arg| placed(arg)));
            process
        }
        None => {
            let mut process = spawned("/bin/sh");
            process.arg("-c").arg(&handler.command);
            process
        }
    };
    process.env("CLAUDE_PROJECT_DIR", project);
    if let Some(root) = root {
        process.env("CLAUDE_PLUGIN_ROOT", root);
    }
    process
}

/// A background handler: it never decides, and what it prints goes nowhere. Only an
/// `asyncRewake` one is held to its timeout; Claude Code enforces none on `async`.
pub(super) fn detach(handler: &Handler, stdin: Vec<u8>, cwd: &Path, project: &Path) {
    let mut process = command(handler, project);
    process.current_dir(cwd);
    let deadline = Instant::now().checked_add(handler.timeout);
    let timed = handler.rewake;
    let due: CancelFlag =
        Arc::new(move || timed && deadline.is_none_or(|due| Instant::now() >= due));
    std::thread::spawn(move || yi_tools::run_captured(process, Some(stdin), &due, 0));
}

pub(super) fn run(
    handler: &Handler,
    stdin: &[u8],
    (cwd, project): (&Path, &Path),
    cancel: &CancelFlag,
) -> Said {
    let mut process = command(handler, project);
    process.current_dir(cwd);
    let deadline = Instant::now().checked_add(handler.timeout);
    let session = Arc::clone(cancel);
    let due = move || deadline.is_none_or(|due| Instant::now() >= due);
    let flag: CancelFlag = Arc::new(move || session() || due());
    let failure = |what: String| match handler.block_on_failure {
        true => Said::Blocked {
            reason: format!("{what}; blocking because onFailure is \"block\""),
            json: None,
            cut: None,
        },
        false => Said::Notice(what),
    };
    let label = handler.command.as_str();
    let capture =
        match yi_tools::run_captured(process, Some(stdin.to_vec()), &flag, HOOK_OUTPUT_CAP) {
            Ok(capture) => capture,
            Err(error) => return failure(format!("[{label}]: failed to start: {error}")),
        };
    if capture.cancelled && !cancel() {
        let secs = handler.timeout.as_secs();
        return failure(format!("[{label}]: timed out after {secs}s"));
    }
    let json = match stdout_json(&capture.stdout) {
        Some(Err(_)) if capture.truncated => Some(Err(format!(
            "its stdout passed HOOK_OUTPUT_CAP ({HOOK_OUTPUT_CAP} bytes per stream), so it was not read"
        ))),
        json => json,
    };
    let cut = capture.cut.filter(|_| capture.truncated).map(|(kept, total)| {
        format!("[… kept {kept} of {total} bytes of the hook's output (HOOK_OUTPUT_CAP {HOOK_OUTPUT_CAP} per stream); no file kept the rest: run `{label}` by hand to read it all]")
    });
    let stderr = capture.stderr.trim();
    match (capture.exit_code, json) {
        (Some(2), json) => Said::Blocked {
            reason: match stderr.is_empty() {
                true => format!("[{label}]: blocked by the hook"),
                false => stderr.to_owned(),
            },
            json: json.and_then(Result::ok),
            cut,
        },
        (Some(0), Some(Ok(value))) => Said::Json(value),
        (Some(0), None) => Said::Quiet,
        (Some(0), Some(Err(error))) => failure(format!("[{label}]: invalid JSON output: {error}")),
        (code, _) => {
            let first = stderr.lines().next().unwrap_or_default();
            let code = code.map_or_else(|| "a signal".to_owned(), |code| format!("exit {code}"));
            failure(format!(
                "[{label}]: Failed with non-blocking status code ({code}): {first}"
            ))
        }
    }
}

/// Stdout is JSON output only as one object with nothing around it but whitespace; anything
/// else is plain text, which the tool events ignore.
fn stdout_json(stdout: &str) -> Option<Result<Value, String>> {
    let text = stdout.trim();
    if !(text.starts_with('{') && text.ends_with('}')) {
        return None;
    }
    Some(match serde_json::from_str::<Value>(text) {
        Ok(value @ Value::Object(_)) => Ok(value),
        Ok(_) => Err("not an object".to_owned()),
        Err(error) => Err(error.to_string()),
    })
}
