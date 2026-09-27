//! Parse-only check of an edited file, run by the language's own tool when it is on PATH.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use crate::process::{OUTPUT_CAP, command, run_captured};
use crate::tool::CancelFlag;

const TIMEOUT: Duration = Duration::from_secs(5);
const MESSAGE_CAP: usize = 160;

fn checker(extension: &str) -> Option<(&'static str, &'static [&'static str])> {
    Some(match extension {
        "py" => ("python3", &["-m", "py_compile"]),
        "js" | "mjs" | "cjs" => ("node", &["--check"]),
        "sh" => ("sh", &["-n"]),
        "bash" => ("bash", &["-n"]),
        "rb" => ("ruby", &["-c"]),
        "php" => ("php", &["-l"]),
        "go" => ("gofmt", &["-e"]),
        "rs" => ("rustfmt", &["--check", "--edition", "2024"]),
        _ => return None,
    })
}

fn on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|found| found.is_file())
}

fn before_deadline() -> CancelFlag {
    let deadline = Instant::now() + TIMEOUT;
    Arc::new(move || Instant::now() >= deadline)
}

/// A rustup proxy re-resolves the toolchain on every call (~45 ms), and a pinned toolchain it
/// lacks is downloaded until the timeout kills it; the binary it names is asked once.
fn rustfmt() -> Option<PathBuf> {
    static RESOLVED: OnceLock<Option<PathBuf>> = OnceLock::new();
    RESOLVED
        .get_or_init(|| {
            let found = on_path("rustfmt")?;
            let rustup = found.with_file_name("rustup");
            if !same_file(&found, &rustup) {
                return Some(PathBuf::from("rustfmt"));
            }
            let mut which = command(&rustup);
            which.args(["which", "rustfmt"]);
            let capture = run_captured(which, None, &before_deadline(), OUTPUT_CAP).ok()?;
            let named = PathBuf::from(capture.stdout.trim());
            (capture.exit_code == Some(0) && named.is_file()).then_some(named)
        })
        .clone()
}

/// A proxy is a symlink or a hard link to `rustup`, so it shares the inode either way.
#[cfg(unix)]
fn same_file(left: &Path, right: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(left), std::fs::metadata(right)) {
        (Ok(left), Ok(right)) => left.dev() == right.dev() && left.ino() == right.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn same_file(_left: &Path, _right: &Path) -> bool {
    false
}

/// `syntax: ok`, `syntax: error line N: <message>`, or `syntax: check timed out`; None when
/// no checker applies or its program is absent.
pub fn verdict(path: &Path) -> Option<String> {
    let extension = path.extension()?.to_str()?;
    if extension == "json" {
        let text = std::fs::read_to_string(path).ok()?;
        return Some(match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(_) => "syntax: ok".to_owned(),
            Err(error) => error_line(&error.to_string()),
        });
    }
    let (program, flags) = checker(extension)?;
    let _span = yi_types::trace::span("syntax.check").arg("program", program);
    let resolved = match program {
        "rustfmt" => rustfmt()?,
        _ => on_path(program).map(|_| PathBuf::from(program))?,
    };
    let mut check = command(resolved);
    check.args(flags).arg(path).env(
        "PYTHONPYCACHEPREFIX",
        std::env::temp_dir().join("yi-pycache"),
    );
    let capture = run_captured(check, None, &before_deadline(), OUTPUT_CAP).ok()?;
    if capture.cancelled {
        return Some("syntax: check timed out".to_owned());
    }
    let output = format!("{}\n{}", capture.stderr, capture.stdout);
    // rustfmt exits 1 for a formatting diff too; only a diagnostic line is a parse failure.
    let failed = capture.exit_code != Some(0)
        && (extension != "rs" || output.lines().any(|line| line.starts_with("error")));
    Some(if failed {
        error_line(&output)
    } else {
        "syntax: ok".to_owned()
    })
}

fn error_line(output: &str) -> String {
    let lines = || {
        output
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
    };
    let message: String = lines()
        .find(|line| line.contains("rror"))
        .or_else(|| lines().next())
        .unwrap_or_default()
        .chars()
        .take(MESSAGE_CAP)
        .collect();
    let number = regex::Regex::new(r"(?:line |:)([0-9]+)")
        .ok()
        .and_then(|pattern| pattern.captures(output))
        .and_then(|found| found.get(1))
        .map(|found| found.as_str());
    match number {
        Some(number) => format!("syntax: error line {number}: {message}"),
        None => format!("syntax: error: {message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::error_line;

    #[test]
    fn line_numbers_come_from_python_node_and_sh_output() {
        assert_eq!(
            error_line("Sorry: IndentationError: expected an indented block (a.py, line 2)\n"),
            "syntax: error line 2: Sorry: IndentationError: expected an indented block (a.py, line 2)"
        );
        assert_eq!(
            error_line("/tmp/a.js:3\n\n\nSyntaxError: Unexpected end of input\n    at wrapSafe\n"),
            "syntax: error line 3: SyntaxError: Unexpected end of input"
        );
        assert_eq!(
            error_line("a.sh: line 3: syntax error: unexpected end of file\n"),
            "syntax: error line 3: a.sh: line 3: syntax error: unexpected end of file"
        );
        assert_eq!(error_line("garbage\n"), "syntax: error: garbage");
    }
}
