//! Parse-only check of an edited file, run by the language's own tool when it is on PATH.

use std::path::Path;
use std::sync::Arc;
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

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
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
    if !on_path(program) {
        return None;
    }
    let mut check = command(program);
    check.args(flags).arg(path).env(
        "PYTHONPYCACHEPREFIX",
        std::env::temp_dir().join("yi-pycache"),
    );
    let deadline = Instant::now() + TIMEOUT;
    let cancelled: CancelFlag = Arc::new(move || Instant::now() >= deadline);
    let capture = run_captured(check, None, &cancelled, OUTPUT_CAP).ok()?;
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
