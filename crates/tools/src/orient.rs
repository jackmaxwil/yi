use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::process::{OUTPUT_CAP, command, run_captured};
use crate::tool::{Tool, ToolContext, ToolKind, ToolOutput, text_output};

const LAYER_CAP: usize = 4_000;
const SKELETON_FILES: usize = 40;
const SKELETON_LINES: usize = 12;
const SKELETON_SCAN: usize = 2_000;
const HEAT_ROWS: usize = 15;
const HEAT_COMMITS: usize = 200;
const ISSUE_ROWS: usize = 10;
const ISSUES_PATH: &str = ".yi/mining/issues.jsonl";

const SKELETON_EXTS: [&str; 6] = ["rs", "py", "ts", "tsx", "js", "go"];

const DECL_HEADS: [&str; 15] = [
    "pub ",
    "fn ",
    "async ",
    "struct ",
    "enum ",
    "trait ",
    "impl ",
    "type ",
    "const ",
    "static ",
    "mod ",
    "macro_rules!",
    "class ",
    "def ",
    "export ",
];

const GATES: [(&str, &str); 6] = [
    ("justfile", "just check"),
    ("Justfile", "just check"),
    ("Makefile", "make"),
    ("Cargo.toml", "cargo test"),
    ("package.json", "npm test"),
    ("pyproject.toml", "pytest"),
];

/// Invariant: an absent layer is named, never dropped — Err carries the reason.
type LayerBody = Result<String, String>;

pub struct GetContextTool;

impl Tool for GetContextTool {
    fn name(&self) -> &str {
        "get_context"
    }

    fn description(&self) -> &str {
        "One orientation packet for the working directory: grid roots, symbol neighborhood, file skeletons, git change heat, gate commands, prior mining issues. Layers are clamped and name what they cut; the header says how many were available."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "symbol": {"type": "string", "description": "Optional callsign or type name for the symbol-neighborhood layer"}
            }
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Read
    }

    fn execute(&self, input: Map<String, Value>, context: &ToolContext) -> ToolOutput {
        let symbol = input.get("symbol").and_then(Value::as_str);
        text_output(packet(symbol, context))
    }
}

fn packet(symbol: Option<&str>, context: &ToolContext) -> String {
    let root = context.cwd.as_path();
    let layers: [(&str, LayerBody); 6] = [
        // Invariant: this tool only reads. `grid survey` charts the worktree,
        // so the packet takes a slice of an existing chart instead.
        ("grid roots", grid(context, &["roots"])),
        ("symbol neighborhood", neighborhood(symbol, context)),
        ("file skeletons", skeletons(root)),
        ("git change heat", git_heat(context)),
        ("gate commands", gates(root)),
        ("prior issues", issues(root)),
    ];
    let present = layers.iter().filter(|(_, body)| body.is_ok()).count();
    let total = layers.len();
    let mut out = String::from("# orientation packet\n");
    if present == total {
        out.push_str("COMPLETE\n");
    } else {
        out.push_str(&format!("PARTIAL - {present} of {total} layers\n"));
    }
    for (name, body) in layers {
        out.push_str(&format!("\n## {name}\n"));
        match body {
            Ok(text) => out.push_str(&clamp(name, text)),
            Err(reason) => out.push_str(&format!("absent: {reason}")),
        }
        out.push('\n');
    }
    out
}

fn clamp(name: &str, mut body: String) -> String {
    if body.len() <= LAYER_CAP {
        return body;
    }
    let mut end = LAYER_CAP;
    while end > 0 && !body.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    body.truncate(end);
    body.push_str(&format!("\n[{name} truncated at {LAYER_CAP} bytes]"));
    body
}

fn grid(context: &ToolContext, args: &[&str]) -> LayerBody {
    let mut spawn = command("grid");
    spawn.args(args).current_dir(&context.cwd);
    let capture = run_captured(spawn, None, &context.cancelled, LAYER_CAP)
        .map_err(|error| format!("grid binary not runnable: {error}"))?;
    if capture.exit_code != Some(0) {
        return Err(format!(
            "grid {} exited {:?}: {}",
            args.join(" "),
            capture.exit_code,
            capture.stderr.lines().next().unwrap_or_default()
        ));
    }
    let stdout = capture.stdout.trim_end().to_owned();
    if stdout.is_empty() {
        return Err(format!("grid {} answered nothing", args.join(" ")));
    }
    Ok(stdout)
}

fn neighborhood(symbol: Option<&str>, context: &ToolContext) -> LayerBody {
    let symbol = symbol.ok_or_else(|| "no symbol argument was given".to_owned())?;
    grid(context, &["scope", symbol, "--depth", "1"])
}

fn skeletons(root: &Path) -> LayerBody {
    let mut files: Vec<PathBuf> = Vec::new();
    crate::builtins::walk_files(root, &mut |path| {
        let interesting = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| SKELETON_EXTS.contains(&ext));
        if interesting {
            files.push(path.to_path_buf());
        }
        files.len() < SKELETON_SCAN
    });
    if files.is_empty() {
        return Err("no source files under the working directory".to_owned());
    }
    let mut named: Vec<String> = files
        .iter()
        .map(|path| {
            path.strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    named.sort();
    let total = named.len();
    let mut out = String::new();
    for name in named.iter().take(SKELETON_FILES) {
        out.push_str(name);
        out.push('\n');
        for line in skeleton_lines(&root.join(name)) {
            out.push_str("  ");
            out.push_str(&line);
            out.push('\n');
        }
    }
    if total > SKELETON_FILES {
        out.push_str(&format!(
            "[skeletons truncated: {SKELETON_FILES} of {total} files]\n"
        ));
    }
    Ok(out.trim_end().to_owned())
}

fn skeleton_lines(path: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|line| DECL_HEADS.iter().any(|head| line.starts_with(head)))
        .map(|line| line.trim_end().trim_end_matches('{').trim_end().to_owned())
        .take(SKELETON_LINES)
        .collect()
}

fn git_heat(context: &ToolContext) -> LayerBody {
    let mut spawn = command("git");
    spawn
        .arg("-C")
        .arg(&context.cwd)
        .arg("log")
        .arg("--no-merges")
        .arg(format!("-{HEAT_COMMITS}"))
        .arg("--format=")
        .arg("--name-only");
    let capture = run_captured(spawn, None, &context.cancelled, OUTPUT_CAP)
        .map_err(|error| format!("git not runnable: {error}"))?;
    if capture.exit_code != Some(0) {
        return Err(format!(
            "git log exited {:?}: {}",
            capture.exit_code,
            capture.stderr.lines().next().unwrap_or_default()
        ));
    }
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for line in capture.stdout.lines().filter(|line| !line.is_empty()) {
        let count = counts.entry(line).or_default();
        *count = count.saturating_add(1);
    }
    if counts.is_empty() {
        return Err("git log named no changed files".to_owned());
    }
    let total = counts.len();
    // A tie on count would otherwise order by hash-map chance; the packet is
    // compared byte for byte across runs.
    let mut rows: Vec<(usize, &str)> = counts.into_iter().map(|(p, c)| (c, p)).collect();
    rows.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(right.1)));
    let mut out = String::new();
    for (count, path) in rows.iter().take(HEAT_ROWS) {
        out.push_str(&format!("{count:>4}  {path}\n"));
    }
    if total > HEAT_ROWS {
        out.push_str(&format!("[heat truncated: {HEAT_ROWS} of {total} paths]\n"));
    }
    if capture.truncated {
        out.push_str("[git log output was capped before counting]\n");
    }
    Ok(out.trim_end().to_owned())
}

fn gates(root: &Path) -> LayerBody {
    let lines: Vec<String> = GATES
        .iter()
        .filter(|(file, _)| root.join(file).exists())
        .map(|(file, gate)| format!("{gate}  ({file})"))
        .collect();
    if lines.is_empty() {
        return Err("no gate manifest in the working directory".to_owned());
    }
    Ok(lines.join("\n"))
}

fn issues(root: &Path) -> LayerBody {
    let text = std::fs::read_to_string(root.join(ISSUES_PATH))
        .map_err(|error| format!("{ISSUES_PATH} unreadable: {error}"))?;
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let total = lines.len();
    let tail = lines.split_at(total.saturating_sub(ISSUE_ROWS)).1;
    let mut out = String::new();
    let mut unparsed = 0usize;
    for line in tail {
        match issue_row(line) {
            Some(row) => out.push_str(&format!("{row}\n")),
            None => unparsed = unparsed.saturating_add(1),
        }
    }
    if out.is_empty() {
        return Err(format!("{ISSUES_PATH} has no fingerprinted rows"));
    }
    if total > tail.len() {
        out.push_str(&format!(
            "[issues truncated: last {} of {total} rows]\n",
            tail.len()
        ));
    }
    if unparsed > 0 {
        out.push_str(&format!("[{unparsed} rows unparsed]\n"));
    }
    Ok(out.trim_end().to_owned())
}

fn issue_row(line: &str) -> Option<String> {
    let value: Value = serde_json::from_str(line).ok()?;
    let fingerprint = value.get("fingerprint")?.as_str()?;
    let text = ["title", "text", "summary"]
        .iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .unwrap_or("(no text)");
    Some(format!("{fingerprint}  {text}"))
}
