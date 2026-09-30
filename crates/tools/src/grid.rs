//! Every `grid` spawn. grid's root is its working directory, so each runs at the chart's root.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::process::{CommandCapture, OUTPUT_CAP, command, run_captured};
use crate::tool::{CancelFlag, ToolContext};

const CHECK_TIMEOUT_MS: u64 = 2_000;
const CHECK_LINES: usize = 40;
const CHECK_CAP: usize = 4 << 20;
const BAD_INPUT: i32 = 2;

pub(crate) enum GridLayer {
    Suspects { rows: Vec<String>, outside: usize },
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
            Self::Suspects { rows, .. } if rows.is_empty() => "clean",
            Self::Suspects { .. } => "findings",
            Self::Unavailable(Unavailable::NoBinary) => "no-binary",
            Self::Unavailable(Unavailable::Timeout { .. }) => "timeout",
            Self::Unavailable(Unavailable::Exit { .. }) => "exit",
            Self::Unavailable(Unavailable::Unparsed) => "unparsed",
        }
    }

    pub(crate) fn render(&self) -> String {
        let reason = match self {
            Self::Suspects { rows, outside } => return suspects_text(rows, *outside),
            Self::Unavailable(Unavailable::NoBinary) => "no grid binary".to_owned(),
            Self::Unavailable(Unavailable::Timeout { ms }) => format!("no answer in {ms} ms"),
            Self::Unavailable(Unavailable::Exit { code }) => format!("exit {code}"),
            Self::Unavailable(Unavailable::Unparsed) => "the answer was not grid's JSON".to_owned(),
        };
        format!("[grid check: unavailable — {reason}]")
    }
}

fn suspects_text(lines: &[String], outside: usize) -> String {
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
    let Some(root) = chart_root(&context.cwd) else {
        return Vec::new();
    };
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
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

/// `grid check` takes no path: a suspect stays when its changed definition is edited or gone.
pub(crate) fn check(context: &ToolContext, files: &[String]) -> GridLayer {
    let deadline = Instant::now() + Duration::from_millis(CHECK_TIMEOUT_MS);
    let parent = Arc::clone(&context.cancelled);
    let cancelled: CancelFlag = Arc::new(move || parent() || Instant::now() > deadline);
    let root = chart_root(&context.cwd).unwrap_or(&context.cwd);
    let answer = |args: &[&str]| -> Result<Value, Unavailable> {
        let capture =
            spawn(root, args, &cancelled, CHECK_CAP).map_err(|_| Unavailable::NoBinary)?;
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
        let rows = |at| report.pointer(at).and_then(Value::as_array);
        let drift = rows("/drift/suspects").ok_or(Unavailable::Unparsed)?;
        let uncompiled = rows("/compile/uncompiled").into_iter().flatten();
        let suspects: Vec<(&str, &Value)> = (drift.iter().map(|row| ("drift", row)))
            .chain(uncompiled.map(|row| ("uncompiled", row)))
            .collect();
        let mut defined: Vec<String> = Vec::new();
        for file in files.iter().filter(|_| !suspects.is_empty()) {
            let defs = match answer(&["resolve", file, "--json"]) {
                Err(Unavailable::Exit { code: BAD_INPUT }) => Value::Null,
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
            row.pointer("/sig_after").is_none()
                || defined.contains(&field(row, "/changed/callsign"))
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
        Ok(GridLayer::Suspects {
            rows: mine.iter().map(line).collect(),
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

/// The nearest `.grid` at or below the repository's top: a worktree never borrows the main one's.
pub(crate) fn chart_root(cwd: &Path) -> Option<&Path> {
    let top = cwd
        .ancestors()
        .find(|dir| dir.join(".grid").is_dir() || dir.join(".git").exists())?;
    top.join(".grid").is_dir().then_some(top)
}

pub(crate) fn layer(root: &Path, args: &[&str], cancelled: &CancelFlag) -> Result<String, String> {
    let capture = spawn(root, args, cancelled, OUTPUT_CAP)
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

pub(crate) fn scope_pattern(symbol: &str) -> String {
    if symbol.contains('*') {
        return symbol.to_owned();
    }
    let dotted = symbol.replace("::", ".");
    let mut name = dotted.trim_start_matches('.');
    while let Some(rest) = ["crate.", "self.", "super."]
        .iter()
        .find_map(|head| name.strip_prefix(head))
    {
        name = rest;
    }
    format!("**.{name}")
}

fn spawn(
    root: &Path,
    args: &[&str],
    cancelled: &CancelFlag,
    cap: usize,
) -> Result<CommandCapture, String> {
    let mut grid = command("grid");
    grid.args(args).current_dir(root);
    run_captured(grid, None, cancelled, cap)
}
