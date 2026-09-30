//! Every `grid` spawn; grid would chart an uncharted working directory, so it is never asked.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::process::{CommandCapture, OUTPUT_CAP, command, run_captured};
use crate::tool::{CancelFlag, ToolContext};

const CHECK_TIMEOUT_MS: u64 = 2_000;
const CHECK_LINES: usize = 40;
const NO_DEFINITIONS: i32 = 2;

pub(crate) enum GridLayer {
    Drift { lines: Vec<String>, outside: usize },
    Unavailable(Unavailable),
}

pub(crate) enum Unavailable {
    NoBinary,
    Timeout { ms: u64 },
    Exit { code: i32 },
    Unparsed,
}

impl GridLayer {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Drift { lines, .. } if lines.is_empty() => "clean",
            Self::Drift { .. } => "findings",
            Self::Unavailable(Unavailable::NoBinary) => "no-binary",
            Self::Unavailable(Unavailable::Timeout { .. }) => "timeout",
            Self::Unavailable(Unavailable::Exit { .. }) => "exit",
            Self::Unavailable(Unavailable::Unparsed) => "unparsed",
        }
    }

    pub(crate) fn render(&self) -> String {
        let reason = match self {
            Self::Drift { lines, outside } => return drift_text(lines, *outside),
            Self::Unavailable(Unavailable::NoBinary) => "no grid binary".to_owned(),
            Self::Unavailable(Unavailable::Timeout { ms }) => format!("no answer in {ms} ms"),
            Self::Unavailable(Unavailable::Exit { code }) => format!("exit {code}"),
            Self::Unavailable(Unavailable::Unparsed) => "the answer was not grid's JSON".to_owned(),
        };
        format!("[grid check: unavailable — {reason}]")
    }
}

fn drift_text(lines: &[String], outside: usize) -> String {
    let head = if lines.is_empty() {
        "[grid check: clean]"
    } else {
        "[grid check]"
    };
    let mut out = vec![head.to_owned()];
    out.extend(lines.iter().take(CHECK_LINES).cloned());
    if lines.len() > CHECK_LINES {
        out.push(format!(
            "[grid check: first {CHECK_LINES} of {} lines — bash: grid check --quick for all]",
            lines.len()
        ));
    }
    if outside > 0 {
        let plural = if outside == 1 { "" } else { "s" };
        out.push(format!(
            "[grid check: {outside} suspect{plural} outside the edited files — bash: grid check --quick]"
        ));
    }
    out.join("\n")
}

pub(crate) fn charted_files<'a>(
    context: &ToolContext,
    paths: impl Iterator<Item = &'a str>,
) -> Vec<String> {
    if !context.cwd.join(".grid").is_dir() {
        return Vec::new();
    }
    let root = context
        .cwd
        .canonicalize()
        .unwrap_or_else(|_| context.cwd.clone());
    paths
        .map(Path::new)
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| matches!(extension, "rs" | "py"))
        })
        .filter_map(|path| path.strip_prefix(&root).ok())
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

/// `grid check` takes no path: a suspect stays when its changed definition is in an edited file.
pub(crate) fn check(context: &ToolContext, files: &[String]) -> GridLayer {
    let deadline = Instant::now() + Duration::from_millis(CHECK_TIMEOUT_MS);
    let parent = Arc::clone(&context.cancelled);
    let cancelled: CancelFlag = Arc::new(move || parent() || Instant::now() > deadline);
    let answer = |args: &[&str]| -> Result<Value, Unavailable> {
        let capture = spawn(context, args, &cancelled).map_err(|_| Unavailable::NoBinary)?;
        if capture.cancelled && !context.cancelled.as_ref()() {
            return Err(Unavailable::Timeout {
                ms: CHECK_TIMEOUT_MS,
            });
        }
        match capture.exit_code {
            Some(0 | 3) => serde_json::from_str(&capture.stdout).map_err(|_| Unavailable::Unparsed),
            Some(code) => Err(Unavailable::Exit { code }),
            None => Err(Unavailable::NoBinary),
        }
    };
    let scope = || -> Result<GridLayer, Unavailable> {
        let report = answer(&["check", "--quick", "--json"])?;
        let rows = |kind, at| {
            report
                .pointer(at)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(move |row| (kind, row))
        };
        let suspects: Vec<(&str, &Value)> = rows("drift", "/drift/suspects")
            .chain(rows("uncompiled", "/compile/uncompiled"))
            .collect();
        let mut defined: Vec<String> = Vec::new();
        for file in files.iter().filter(|_| !suspects.is_empty()) {
            let defs = match answer(&["resolve", file, "--json"]) {
                Err(Unavailable::Exit {
                    code: NO_DEFINITIONS,
                }) => Value::Null,
                other => other?,
            };
            defined.extend(
                defs.as_array()
                    .into_iter()
                    .flatten()
                    .map(|def| field(def, "/callsign")),
            );
        }
        let (mine, outside): (Vec<_>, Vec<_>) = suspects.into_iter().partition(|(_, row)| {
            defined.contains(&field(row, "/changed/callsign"))
                || files.contains(&field(row, "/changed/materialization/file"))
        });
        let line = |(kind, row): &(&str, &Value)| {
            let [dependent, relation, changed, file, line] = [
                "dependent/callsign",
                "relation",
                "changed/callsign",
                "dependent/materialization/file",
                "dependent/materialization/line",
            ]
            .map(|at| field(row, &format!("/{at}")));
            format!("{kind} {dependent} <-{relation}- {changed} at {file}:{line}")
        };
        Ok(GridLayer::Drift {
            lines: mine.iter().map(line).collect(),
            outside: outside.len(),
        })
    };
    scope().unwrap_or_else(GridLayer::Unavailable)
}

fn field(row: &Value, at: &str) -> String {
    match row.pointer(at) {
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
        None => "?".to_owned(),
    }
}

pub(crate) fn layer(context: &ToolContext, args: &[&str]) -> Result<String, String> {
    if !context.cwd.join(".grid").is_dir() {
        return Err(
            "no .grid chart in the working directory — bash: grid survey charts it".to_owned(),
        );
    }
    let capture = spawn(context, args, &context.cancelled)
        .map_err(|error| format!("grid binary not runnable: {error}"))?;
    if capture.exit_code != Some(0) {
        return Err(format!(
            "grid {} exited {:?}: {}",
            args.join(" "),
            capture.exit_code,
            capture.stderr.trim_end()
        ));
    }
    let stdout = capture.stdout.trim_end().to_owned();
    if stdout.is_empty() {
        return Err(format!("grid {} answered nothing", args.join(" ")));
    }
    Ok(stdout)
}

/// grid matches every dot-separated segment, and `**` any number of them.
pub(crate) fn scope_pattern(symbol: &str) -> String {
    if symbol.contains('*') {
        return symbol.to_owned();
    }
    format!("**.{}", symbol.replace("::", "."))
}

fn spawn(
    context: &ToolContext,
    args: &[&str],
    cancelled: &CancelFlag,
) -> Result<CommandCapture, String> {
    let mut grid = command("grid");
    grid.args(args).current_dir(&context.cwd);
    run_captured(grid, None, cancelled, OUTPUT_CAP)
}
