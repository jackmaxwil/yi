use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::process::{OUTPUT_CAP, run_captured, run_captured_until};
use crate::tool::{Tool, ToolContext, ToolKind, ToolOutput, error_output, text_output};

pub struct ExecTool {
    path: PathBuf,
    name: String,
    description: String,
    schema: Value,
    kind: ToolKind,
}

fn parse_kind(value: Option<&str>) -> ToolKind {
    match value {
        Some("read") => ToolKind::Read,
        Some("write") => ToolKind::Write,
        _ => ToolKind::Exec,
    }
}

impl ExecTool {
    fn from_schema_output(path: PathBuf, output: &str) -> Option<Self> {
        let value: Value = serde_json::from_str(output).ok()?;
        let name = value.get("name")?.as_str()?.to_owned();
        let description = value
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let schema = value
            .get("input_schema")
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        let kind = parse_kind(value.get("kind").and_then(Value::as_str));
        Some(Self {
            path,
            name,
            description,
            schema,
            kind,
        })
    }
}

impl Tool for ExecTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn schema(&self) -> Value {
        self.schema.clone()
    }

    fn kind(&self) -> ToolKind {
        self.kind
    }

    fn execute(&self, input: Map<String, Value>, context: &ToolContext) -> ToolOutput {
        let args = match serde_json::to_vec(&Value::Object(input)) {
            Ok(args) => args,
            Err(error) => return error_output(format!("failed to encode arguments: {error}")),
        };
        let mut process = crate::process::command(&self.path);
        process.current_dir(&context.cwd);
        let capture = match run_captured(process, Some(args), &context.cancelled, OUTPUT_CAP) {
            Ok(capture) => capture,
            Err(message) => return error_output(message),
        };
        if capture.cancelled {
            return error_output("[tool aborted]");
        }
        if capture.exit_code != Some(0) {
            let message = if capture.stderr.is_empty() {
                format!("tool exited with code {:?}", capture.exit_code)
            } else {
                capture.stderr
            };
            return error_output(message);
        }
        // An exec tool declares only its input; its value is whatever JSON it prints.
        let value = (context.program).then(|| serde_json::from_str::<Value>(&capture.stdout).ok());
        let mut output = text_output(capture.stdout);
        output.value = value.flatten();
        output
    }
}

/// User-level only. Project `.yi/tools` waits on hash-pinned trust (D30): without a grant
/// store, a cloned repo's tools are arbitrary code behind one prompt.
#[derive(Debug)]
pub struct SkippedExecTool {
    pub path: PathBuf,
    pub reason: String,
}

pub fn discover_exec_tools(dir: &Path) -> (Vec<ExecTool>, Vec<SkippedExecTool>) {
    const PROBE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);
    discover_within(dir, PROBE_DEADLINE)
}

/// Invariant: probes run side by side under a pool of four, each with its own deadline measured
/// from its own start, so a slow sibling enumerated first cannot skip a later tool.
fn discover_within(
    dir: &Path,
    deadline: std::time::Duration,
) -> (Vec<ExecTool>, Vec<SkippedExecTool>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return (Vec::new(), Vec::new());
    };
    let paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| is_executable_file(path))
        .collect();
    let outcomes = probe_all(&paths, deadline);
    collect(paths, outcomes)
}

fn collect(
    paths: Vec<PathBuf>,
    outcomes: Vec<ProbeOutcome>,
) -> (Vec<ExecTool>, Vec<SkippedExecTool>) {
    let (mut tools, mut skipped) = (Vec::new(), Vec::new());
    for (path, outcome) in paths.into_iter().zip(outcomes) {
        match outcome {
            Some(Ok(tool)) => tools.push(tool),
            Some(Err(skip)) => skipped.push(skip),
            None => skipped.push(SkippedExecTool {
                path,
                reason: "schema probe panicked".to_owned(),
            }),
        }
    }
    tools.sort_by(|left, right| left.name.cmp(&right.name));
    skipped.sort_by(|left, right| left.path.cmp(&right.path));
    (tools, skipped)
}

const PROBE_WORKERS: usize = 4;

/// A slot stays `None` only when its probe thread panicked; `collect` names that path as skipped.
type ProbeOutcome = Option<Result<ExecTool, SkippedExecTool>>;

fn probe_all(paths: &[PathBuf], deadline: std::time::Duration) -> Vec<ProbeOutcome> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let next = AtomicUsize::new(0);
    let outcomes = std::sync::Mutex::new(paths.iter().map(|_| None).collect::<Vec<_>>());
    let drain = || loop {
        let index = next.fetch_add(1, Ordering::SeqCst);
        let Some(path) = paths.get(index) else {
            return;
        };
        let outcome = probe(path.clone(), deadline);
        let mut slots = match outcomes.lock() {
            Ok(slots) => slots,
            Err(poison) => poison.into_inner(),
        };
        if let Some(slot) = slots.get_mut(index) {
            *slot = Some(outcome);
        }
    };
    std::thread::scope(|scope| {
        let mut workers = Vec::new();
        for _ in 0..PROBE_WORKERS {
            match std::thread::Builder::new()
                .name("yi-exec-probe".to_owned())
                .spawn_scoped(scope, drain)
            {
                Ok(worker) => workers.push(worker),
                Err(_) => break,
            }
        }
        if workers.is_empty() {
            drain();
        }
        for worker in workers {
            let _ = worker.join();
        }
    });
    match outcomes.into_inner() {
        Ok(slots) => slots,
        Err(poison) => poison.into_inner(),
    }
}

fn probe(path: PathBuf, deadline: std::time::Duration) -> Result<ExecTool, SkippedExecTool> {
    let mut command = crate::process::command(&path);
    command.arg("--schema");
    let capture = match run_captured_until(command, None, None, deadline, OUTPUT_CAP) {
        Ok(capture) => capture,
        Err(reason) => return Err(SkippedExecTool { path, reason }),
    };
    let reason = if capture.cancelled {
        format!("schema probe timed out after {}s", deadline.as_secs_f64())
    } else if capture.exit_code != Some(0) {
        match capture.exit_code {
            Some(code) => format!("--schema exited with code {code}"),
            None => "--schema terminated without an exit code".to_owned(),
        }
    } else {
        return ExecTool::from_schema_output(path.clone(), &capture.stdout).ok_or(
            SkippedExecTool {
                path,
                reason: "no valid schema on stdout".to_owned(),
            },
        );
    };
    Err(SkippedExecTool { path, reason })
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path)
        .map(|meta| meta.is_file())
        .unwrap_or(false)
}

#[cfg(all(test, unix))]
mod tests {
    use super::{PROBE_WORKERS, collect, discover_within};
    use crate::Tool;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn scripts(dir: &std::path::Path, bodies: &[(&str, String)]) -> TestResult {
        for (name, body) in bodies {
            let script = dir.join(name);
            fs::write(&script, body)?;
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755))?;
        }
        Ok(())
    }

    /// A hung probe spends its own deadline and nothing more: with eight hung tools and a pool of
    /// four, discovery costs two rounds, which no single shared budget can produce.
    #[test]
    fn hung_probes_spend_one_deadline_per_pool_round() -> TestResult {
        let dir = crate::scratch::Scratch::new("yi-tools-exec-hung")?;
        let names = ["a", "b", "c", "d", "e", "f", "g", "h"];
        let bodies: Vec<(&str, String)> =
            ["hang-a", "hang-b", "hang-c", "hang-d", "hang-e", "hang-f"]
                .into_iter()
                .chain(["hang-g", "hang-h"])
                .map(|name| (name, "#!/bin/sh\nsleep 30\n".to_owned()))
                .collect();
        scripts(&dir, &bodies)?;
        let deadline = std::time::Duration::from_secs(1);
        let started = std::time::Instant::now();
        let (_tools, skipped) = discover_within(&dir, deadline);
        let elapsed = started.elapsed();
        assert_eq!(skipped.len(), names.len(), "{skipped:?}");
        assert!(
            skipped
                .iter()
                .all(|skip| skip.reason == "schema probe timed out after 1s"),
            "{skipped:?}"
        );
        assert!(
            elapsed >= deadline.saturating_mul(2),
            "eight hung probes at {deadline:?} finished in {elapsed:?}, so one budget covered them"
        );
        assert!(elapsed < deadline.saturating_mul(5), "{elapsed:?}");
        Ok(())
    }

    /// The slow probe prints a valid schema once it wakes, so only the deadline can skip it.
    #[test]
    fn a_probe_inside_the_deadline_loads_and_one_past_it_is_skipped() -> TestResult {
        let dir = crate::scratch::Scratch::new("yi-tools-exec-limit")?;
        scripts(
            &dir,
            &[
                (
                    "fast",
                    "#!/bin/sh\nprintf '{\"name\":\"fast\",\"input_schema\":{\"type\":\"object\"}}'\n"
                        .to_owned(),
                ),
                (
                    "slow",
                    "#!/bin/sh\nsleep 30\nprintf '{\"name\":\"slow\",\"input_schema\":{\"type\":\"object\"}}'\n"
                        .to_owned(),
                ),
            ],
        )?;
        let (tools, skipped) = discover_within(&dir, std::time::Duration::from_secs(3));
        assert_eq!(tools.len(), 1, "{skipped:?}");
        assert_eq!(tools[0].name(), "fast");
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].path, dir.join("slow"));
        assert!(
            skipped[0].reason.contains("timed out after 3s"),
            "{skipped:?}"
        );
        Ok(())
    }

    #[test]
    fn a_slow_sibling_cannot_skip_a_later_tool() -> TestResult {
        let dir = crate::scratch::Scratch::new("yi-tools-exec-slow-siblings")?;
        let names = [
            "slow-a", "slow-b", "slow-c", "slow-d", "slow-e", "slow-f", "slow-g", "slow-h",
        ];
        let bodies: Vec<(&str, String)> = names
            .into_iter()
            .map(|name| {
                (
                    name,
                    format!("#!/bin/sh\nsleep 0.5\nprintf '{{\"name\":\"{name}\"}}'\n"),
                )
            })
            .collect();
        scripts(&dir, &bodies)?;
        let (tools, skipped) = discover_within(&dir, std::time::Duration::from_secs(4));
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(tools.len(), names.len());
        Ok(())
    }

    /// The bound the pool holds is what keeps a large tools directory from forking it whole; each
    /// probe counts the siblings alive beside it in this scratch dir, so the peak is observed.
    #[test]
    fn exec_tool_probes_run_under_a_fixed_pool() -> TestResult {
        let dir = crate::scratch::Scratch::new("yi-tools-exec-pool")?;
        let total = 8;
        for index in 0..total {
            let name = format!("tool-{index:02}");
            scripts(
                &dir,
                &[(
                    &name,
                    format!(
                        "#!/bin/sh\nif [ \"$1\" = \"--schema\" ]; then\n  \
                         pgrep -f '{dir}/tool-' | wc -l > '{dir}/seen-{name}'\n  \
                         sleep 30\n  \
                         printf '{{\"name\":\"{name}\",\"input_schema\":{{\"type\":\"object\"}}}}'\nfi\n",
                        dir = dir.display(),
                    ),
                )],
            )?;
        }
        let (_tools, skipped) = discover_within(&dir, std::time::Duration::from_secs(1));
        assert_eq!(skipped.len(), total, "{skipped:?}");
        let mut seen: Vec<usize> = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            if path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("seen-"))
                && let Ok(text) = fs::read_to_string(&path)
                && let Ok(count) = text.trim().parse()
            {
                seen.push(count);
            }
        }
        let peak = seen.iter().copied().max().unwrap_or(0);
        assert!(
            (2..=PROBE_WORKERS).contains(&peak),
            "peak {peak} probes ran at once: {seen:?}"
        );
        Ok(())
    }

    #[test]
    fn a_probe_that_panics_is_named_as_skipped() {
        let path = PathBuf::from("/virtual/panicked-tool");
        let (tools, skipped) = collect(vec![path.clone()], vec![None]);
        assert!(tools.is_empty());
        assert_eq!(skipped.len(), 1, "{skipped:?}");
        assert_eq!(skipped[0].path, path);
        assert!(skipped[0].reason.contains("panicked"), "{skipped:?}");
    }
}
