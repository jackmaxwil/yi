//! Every `ripwire` spawn. Ripwire indexes the tree it is pointed at, so each runs at the repository's top.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::process::{CommandCapture, OUTPUT_CAP, command, run_captured};
use crate::tool::{CancelFlag, ToolContext};

const CHECK_TIMEOUT_MS: u64 = 3_000;
const CHECK_REGIONS: usize = 4;
const CHECK_LINES: usize = 40;
const CHECK_CAP: usize = 1 << 20;
const CODE_EXTS: [&str; 41] = [
    "rs", "py", "pyi", "ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs", "go", "java", "kt",
    "kts", "swift", "rb", "php", "c", "h", "cc", "cpp", "cxx", "hh", "hpp", "hxx", "m", "mm", "cu",
    "cuh", "metal", "cs", "dart", "lua", "ex", "exs", "gd", "sh", "bash", "scala", "zig",
];
/// Incident: Rust, Go and Ruby answer `incompatible="0"` for broken callers; these flag them.
const ARITY_EXTS: [&str; 11] = [
    "py", "pyi", "ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs", "java",
];

pub(crate) struct CheckLayer {
    rows: Vec<String>,
    missed: Vec<String>,
    asked: usize,
    regions: Vec<(String, u64)>,
}

impl CheckLayer {
    pub(crate) fn failed(reason: &str) -> Self {
        let missed = vec![format!("[ripwire check: unavailable — {reason}]")];
        let (rows, asked, regions) = (Vec::new(), 0, Vec::new());
        Self {
            rows,
            missed,
            asked,
            regions,
        }
    }

    pub(crate) fn name(&self) -> &'static str {
        match (self.rows.is_empty(), self.missed.is_empty()) {
            (false, _) => "findings",
            (true, true) => "clean",
            (true, false) => "unavailable",
        }
    }

    pub(crate) fn render(&self) -> String {
        let clean = self.rows.is_empty() && self.missed.is_empty();
        let mut out = vec![
            if clean {
                "[ripwire check: clean]"
            } else {
                "[ripwire check]"
            }
            .to_owned(),
        ];
        out.extend(self.rows.iter().take(CHECK_LINES).cloned());
        if self.rows.len() > CHECK_LINES {
            out.push(format!(
                "[ripwire check: first {CHECK_LINES} of {} lines — bash: ripwire . --edit-check=SYM for one definition]",
                self.rows.len()
            ));
        }
        out.extend(self.missed.iter().cloned());
        if let Some((file, line)) = self.regions.get(self.asked) {
            out.push(format!(
                "[ripwire check: {} of {} changed regions checked, cap {CHECK_REGIONS} — bash: ripwire . --edit-check=@{file}:{line}]",
                self.asked,
                self.regions.len()
            ));
        }
        out.join("\n")
    }
}

pub(crate) fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

pub fn installed() -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join("ripwire").is_file()))
}

/// The repository's top, where ripwire indexes; a worktree never borrows the main checkout's.
pub(crate) fn root(cwd: &Path) -> &Path {
    cwd.ancestors()
        .find(|dir| dir.join(".git").exists())
        .unwrap_or(cwd)
}

pub(crate) fn regions(
    context: &ToolContext,
    path: &str,
    before: &str,
    after: &str,
) -> Vec<(String, u64)> {
    let path = Path::new(path);
    let code = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| CODE_EXTS.contains(&extension));
    let top = root(&context.cwd);
    let top = top.canonicalize().unwrap_or_else(|_| top.to_path_buf());
    let Some(relative) = path.strip_prefix(&top).ok().filter(|_| code) else {
        return Vec::new();
    };
    let relative = relative.to_string_lossy().into_owned();
    let lines: Vec<&str> = after.split('\n').collect();
    let blank = |line: u64| {
        usize::try_from(line.saturating_sub(1))
            .ok()
            .and_then(|index| lines.get(index))
            .is_none_or(|text| text.trim().is_empty())
    };
    crate::diff::changed_after_lines(before, after)
        .into_iter()
        .filter(|line| !blank(*line))
        .map(|line| (relative.clone(), line))
        .collect()
}

/// Incident: a cold index took 8 to 18 s on Yi, and killing it at the deadline meant none ever built.
pub(crate) fn check(context: &ToolContext, regions: Vec<(String, u64)>) -> CheckLayer {
    let deadline = Instant::now() + Duration::from_millis(CHECK_TIMEOUT_MS);
    let top = root(&context.cwd).to_path_buf();
    let asked = regions.len().min(CHECK_REGIONS);
    let (send, answers) = std::sync::mpsc::channel();
    for (index, (file, line)) in regions.iter().take(asked).enumerate() {
        let (send, top, cancelled) = (send.clone(), top.clone(), Arc::clone(&context.cancelled));
        let selector = format!("--edit-check=@{file}:{line}");
        std::thread::spawn(move || {
            let capture = spawn(&top, &[&selector], &cancelled, CHECK_CAP);
            send.send((index, capture)).ok();
        });
    }
    drop(send);
    let mut outcomes: Vec<Option<Result<CommandCapture, String>>> = vec![None; asked];
    while let Some(wait) = deadline.checked_duration_since(Instant::now()) {
        let Ok((index, capture)) = answers.recv_timeout(wait) else {
            break;
        };
        if let Some(slot) = outcomes.get_mut(index) {
            *slot = Some(capture);
        }
    }
    let (mut rows, mut missed, mut seen) = (Vec::new(), Vec::new(), Vec::new());
    for ((file, line), outcome) in regions.iter().zip(outcomes) {
        let reason = match outcome {
            None => format!(
                "no answer in {CHECK_TIMEOUT_MS} ms; the run finishes in the background, so a later edit is checked"
            ),
            Some(Err(error)) => format!("not runnable: {error}"),
            Some(Ok(capture)) => match capture.exit_code {
                Some(0) => match finding(&uncommented(&capture.stdout), &mut seen) {
                    Ok(found) => {
                        rows.extend(found);
                        continue;
                    }
                    Err(reason) => reason,
                },
                // A changed line inside no definition (a comment, an import) has no contract.
                Some(1) if capture.stderr.contains("symbol not found") => continue,
                Some(code) => format!("exit {code}"),
                None => "killed before it exited".to_owned(),
            },
        };
        missed.push(format!(
            "[ripwire check: {file}:{line} unavailable — {reason}; bash: ripwire . --edit-check=@{file}:{line}]"
        ));
    }
    CheckLayer {
        rows,
        missed,
        asked,
        regions,
    }
}

fn finding(xml: &str, seen: &mut Vec<String>) -> Result<Vec<String>, String> {
    let Some(head) = tags(xml, "edit-check").first().copied() else {
        return Err("the answer was not ripwire's edit-check".to_owned());
    };
    let at = attr(head, "p");
    if seen.contains(&at) {
        return Ok(Vec::new());
    }
    seen.push(at.clone());
    let extension = at
        .split(':')
        .next()
        .and_then(|path| path.rsplit_once('.'))
        .map(|(_, ext)| ext);
    let proven = extension.is_some_and(|ext| ARITY_EXTS.contains(&ext));
    let (status, incompatible) = (attr(head, "status"), attr(head, "incompatible"));
    let callers = attr(head, "callers");
    let fit = match proven {
        true => format!("{callers} callers, {incompatible} incompatible"),
        false => format!(
            "{callers} callers, arity not checked for .{} files — read each call",
            extension.unwrap_or("?")
        ),
    };
    let lead = format!("{} at {at}", attr(head, "sym"));
    let row = match status.as_str() {
        "contract-change" => format!(
            "{lead}: {} changed, params {} -> {}; {fit}",
            attr(head, "change"),
            attr(head, "params_was"),
            attr(head, "params_now")
        ),
        "no-baseline" => {
            format!("{lead}: no git HEAD to compare, so a contract change is unknown; {fit}")
        }
        "unchanged" | "new-symbol" if incompatible.parse::<u64>().is_err() => {
            return Err(format!(
                "the answer named no incompatible count ({incompatible})"
            ));
        }
        "unchanged" | "new-symbol" if proven && incompatible != "0" => {
            format!("{lead}: {incompatible} callers do not fit it now")
        }
        "unchanged" | "new-symbol" => return Ok(Vec::new()),
        _ => return Err(format!("the answer named an unknown status ({status})")),
    };
    let mut out = vec![row];
    for caller in tags(xml, "c") {
        let flag = match proven && attr(caller, "incompatible") == "1" {
            true => format!(" — incompatible at line {}", attr(caller, "sites_l")),
            false => String::new(),
        };
        out.push(format!(
            "  {} {}{flag}",
            attr(caller, "n"),
            attr(caller, "p")
        ));
    }
    Ok(out)
}

/// Incident: ripwire's legend comment spells its own rows (`<c n= p=>`), which read as a caller.
fn uncommented(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some((kept, tail)) = rest.split_once("<!--") {
        out.push_str(kept);
        rest = tail.split_once("-->").map_or("", |(_, after)| after);
    }
    out.push_str(rest);
    out
}

fn tags<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
    let open = format!("<{name} ");
    xml.match_indices(open.as_str())
        .filter_map(|(at, _)| {
            let rest = xml.get(at.saturating_add(open.len())..)?;
            rest.find('>').and_then(|end| rest.get(..end))
        })
        .collect()
}

fn attr(element: &str, key: &str) -> String {
    let needle = format!(" {key}=\"");
    let padded = format!(" {element}");
    padded
        .find(&needle)
        .and_then(|at| padded.get(at.saturating_add(needle.len())..))
        .and_then(|rest| rest.split_once('"'))
        .map_or_else(
            || "?".to_owned(),
            |(value, _)| {
                value
                    .replace("&quot;", "\"")
                    .replace("&apos;", "'")
                    .replace("&lt;", "<")
                    .replace("&gt;", ">")
                    .replace("&amp;", "&")
            },
        )
}

pub(crate) fn json(root: &Path, args: &[&str], cancelled: &CancelFlag) -> Result<Value, String> {
    let mut args = args.to_vec();
    args.push("--json");
    let capture = spawn(root, &args, cancelled, OUTPUT_CAP)
        .map_err(|error| format!("ripwire binary not runnable: {error}"))?;
    if capture.exit_code != Some(0) {
        return Err(format!(
            "ripwire {} exited {:?}: {}",
            args.join(" "),
            capture.exit_code,
            capture.stderr.trim_end()
        ));
    }
    serde_json::from_str(&capture.stdout).map_err(|_| {
        format!(
            "ripwire {} answered something other than JSON",
            args.join(" ")
        )
    })
}

fn spawn(
    root: &Path,
    args: &[&str],
    cancelled: &CancelFlag,
    cap: usize,
) -> Result<CommandCapture, String> {
    let mut ripwire = command("ripwire");
    ripwire.arg(".").args(args).current_dir(root);
    run_captured(ripwire, None, cancelled, cap)
}
