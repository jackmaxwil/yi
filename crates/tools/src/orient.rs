use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::process::{OUTPUT_CAP, command, run_captured};
use crate::tool::{Tool, ToolContext, ToolKind, ToolOutput, text_output};

const LAYER_CAP: usize = 4_000;
const NEAR_ROWS: usize = 20;
/// The skeleton footer's longest form, held back from the layer so the footer always fits.
const FOOTER_ROOM: usize = 200;
const SKELETON_FILES: usize = 40;
const SKELETON_LINES: usize = 12;
const SKELETON_SCAN: usize = 2_000;
const HEAT_ROWS: usize = 15;
const HEAT_COMMITS: usize = 200;
const ISSUE_ROWS: usize = 10;
const ISSUES_PATH: &str = ".yi/mining/issues.jsonl";

pub(crate) const SKELETON_EXTS: [&str; 6] = ["rs", "py", "ts", "tsx", "js", "go"];

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
        "One orientation packet for the working directory: task map, symbol neighborhood, file skeletons, git change heat, gate commands, prior mining issues. Layers are clamped and name what they cut; the header says how many were available. Call it once, first, in a repository you have not read this session. PARTIAL - k of n layers means the missing layers are absent, not empty; read what the packet names."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task": {"type": "string", "description": "Optional task in plain words for the task-map layer"},
                "symbol": {"type": "string", "description": "Optional function or type name for the symbol-neighborhood layer"}
            }
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Read
    }

    fn execute(&self, input: Map<String, Value>, context: &ToolContext) -> ToolOutput {
        let task = input.get("task").and_then(Value::as_str);
        let symbol = input.get("symbol").and_then(Value::as_str);
        text_output(packet(task, symbol, context))
    }
}

fn packet(task: Option<&str>, symbol: Option<&str>, context: &ToolContext) -> String {
    let root = context.cwd.as_path();
    // The ripwire calls wait on another process, so they run beside the heat and the walk.
    let (map, near, heat, skeleton) = std::thread::scope(|scope| {
        let map = scope.spawn(|| {
            let _span = yi_types::trace::span("context.ripwire");
            task_map(task, context)
        });
        let near = scope.spawn(|| {
            let _span = yi_types::trace::span("context.ripwire");
            neighborhood(symbol, context)
        });
        let heat_span = yi_types::trace::span("context.heat");
        let heat = change_heat(context);
        drop(heat_span);
        let _span = yi_types::trace::span("context.skeletons");
        let skeleton = skeletons(root, symbol, heat.as_ref().ok(), context);
        (joined(map), joined(near), heat, skeleton)
    });
    let layers: [(&str, LayerBody); 6] = [
        ("task map", map),
        ("symbol neighborhood", near),
        ("file skeletons", skeleton),
        ("git change heat", git_heat(heat)),
        ("gate commands", gates(root)),
        ("prior issues", issues(root, context)),
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

fn joined(layer: std::thread::ScopedJoinHandle<'_, LayerBody>) -> LayerBody {
    layer
        .join()
        .unwrap_or_else(|_| Err("the layer's thread panicked".to_owned()))
}

fn clamp(name: &str, body: String) -> String {
    if body.len() <= LAYER_CAP {
        return body;
    }
    let mut end = body.floor_char_boundary(LAYER_CAP);
    // A cut mid-row reads as a row; the last whole one is where the layer ends.
    if let Some(row_end) = body.get(..end).and_then(|kept| kept.rfind('\n')) {
        end = row_end;
    }
    let kept = body.get(..end).unwrap_or_default();
    format!("{kept}\n[{name} truncated at {LAYER_CAP} bytes]")
}

fn task_map(task: Option<&str>, context: &ToolContext) -> LayerBody {
    let task = task.ok_or_else(|| "no task argument was given".to_owned())?;
    let root = &crate::ripwire::root(&context.cwd);
    let answer = crate::ripwire::json(root, &[&format!("--for={task}")], &context.cancelled)?;
    let rows = answer.get("sigs").and_then(Value::as_array);
    let mut out = vec![format!(
        "ripwire confidence {} (margin {}%)",
        field(&answer, "confidence"),
        field(&answer, "margin_pct")
    )];
    out.extend(rows.into_iter().flatten().map(|row| {
        format!(
            "{}:{}  {}",
            field(row, "p"),
            field(row, "l"),
            field(row, "sig")
        )
    }));
    if let Some(floor) = answer.get("relevance_floor").and_then(Value::as_str) {
        let floor = floor.trim().trim_start_matches('[').trim_end_matches(']');
        out.push(format!("[task map — {floor}]"));
    }
    if answer.get("capped").and_then(Value::as_bool) == Some(true) {
        let spent = answer
            .get("est_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(4_000);
        out.push(format!(
            "[task map: {} of {} signatures, cut at ripwire's byte budget — bash: ripwire . --for={} --json --token-budget={}]",
            field(&answer, "sigs_shown"),
            field(&answer, "sigs_total"),
            crate::process::quoted(task),
            spent.saturating_mul(2).div_ceil(1_000).saturating_mul(1_000),
        ));
    }
    Ok(out.join("\n"))
}

fn neighborhood(symbol: Option<&str>, context: &ToolContext) -> LayerBody {
    let symbol = bare(symbol.ok_or_else(|| "no symbol argument was given".to_owned())?);
    let root = &crate::ripwire::root(&context.cwd);
    let ask = |verb: &str| -> Result<(String, bool), String> {
        let flag = format!("--{verb}={symbol}");
        let answer = crate::ripwire::json(root, &[&flag], &context.cancelled)?;
        let rows: &[Value] = match answer.get(verb) {
            Some(Value::Array(rows)) => rows,
            _ => return Err(format!("the answer named no {verb} rows")),
        };
        let count = field(&answer, "count");
        let mut out = vec![format!(
            "{verb} of {symbol}: {count}, matched by name: a row can be a same-named function elsewhere, and calls ripwire could not resolve are missing"
        )];
        out.extend(
            rows.iter()
                .take(NEAR_ROWS)
                .map(|row| format!("  {} {}", field(row, "n"), field(row, "p"))),
        );
        if rows.len() > NEAR_ROWS {
            out.push(format!(
                "[{verb}: {NEAR_ROWS} of {count} rows, cap {NEAR_ROWS} — bash: ripwire . --{verb}={} --json --limit={count}]",
                crate::process::quoted(&symbol)
            ));
        }
        Ok((out.join("\n"), rows.is_empty()))
    };
    let (callers, callees) = std::thread::scope(|scope| {
        let callees = scope.spawn(|| ask("callees"));
        let callers = ask("callers");
        let panicked = |_| Err("the layer's thread panicked".to_owned());
        (callers, callees.join().unwrap_or_else(panicked))
    });
    let ((callers, no_callers), (callees, no_callees)) = (callers?, callees?);
    let mut out = format!("{callers}\n{callees}");
    if no_callers && no_callees {
        out.push_str(&format!(
            "\n[{symbol}: no call found either way; ripwire matches calls by name and does not track uses of a type — grep -rn {} for those]",
            crate::process::quoted(&symbol)
        ));
    }
    Ok(out)
}

/// Incident: a path-qualified name missed; ripwire takes `name` or `Type::name`.
fn bare(symbol: &str) -> String {
    let mut parts = symbol.rsplit("::");
    let name = parts.next().unwrap_or(symbol);
    match parts.next() {
        Some(owner) if owner.starts_with(char::is_uppercase) => format!("{owner}::{name}"),
        _ => name.to_owned(),
    }
}

fn field(row: &Value, key: &str) -> String {
    match row.get(key) {
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
        None => "?".to_owned(),
    }
}

fn skeletons(
    root: &Path,
    symbol: Option<&str>,
    heat: Option<&Heat>,
    context: &ToolContext,
) -> LayerBody {
    let mut files: Vec<PathBuf> = Vec::new();
    let walled = crate::builtins::walk_files(root, &context.deny_read, &mut |path| {
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
        let rows = ["no source files under the working directory".to_owned()];
        return Err(rows
            .into_iter()
            .chain(walled.notices())
            .collect::<Vec<_>>()
            .join("\n"));
    }
    let gate = context.read_gate();
    // Incident: name order spent the whole layer on the alphabetically first
    // crate, so a symbol's own file was never shown.
    let mut ranked: Vec<(u8, usize, String)> = files
        .iter()
        .map(|path| {
            let name = path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .into_owned();
            // ponytail: heat keys are repo-root relative, so from a subdirectory
            // cwd nothing matches; the order line then reports 0 with heat.
            let hot = heat.and_then(|heat| heat.counts.get(&name)).copied();
            (
                symbol_rank(path, symbol, &gate, context),
                hot.unwrap_or(0),
                name,
            )
        })
        .collect();
    ranked.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then(right.1.cmp(&left.1))
            .then_with(|| left.2.cmp(&right.2))
    });
    let total = ranked.len();
    let mut out = order_line(&ranked, symbol);
    let mut shown = 0_usize;
    let mut cap = format!("the {SKELETON_FILES}-file cap");
    for (_, _, name) in ranked.iter().take(SKELETON_FILES) {
        let mut block = format!("{name}\n");
        let text = (gate.open(&root.join(name), &context.deny_read))
            .and_then(std::io::read_to_string)
            .unwrap_or_default();
        for line in skeleton(&text, SKELETON_LINES).0 {
            block.push_str(&format!("  {line}\n"));
        }
        // Whole files only: the layer clamp cuts from the end, and this footer is what it cut.
        if out.len().saturating_add(block.len()) > LAYER_CAP.saturating_sub(FOOTER_ROOM) {
            cap = format!("the {LAYER_CAP}-byte layer budget");
            break;
        }
        out.push_str(&block);
        shown = shown.saturating_add(1);
    }
    if shown < total {
        out.push_str(&format!(
            "[skeletons truncated: {shown} of {total} files at {cap}; \
             read a directory for its skeletons, or pass symbol to rank its files first]\n"
        ));
    }
    for notice in walled.notices() {
        out.push_str(&notice);
    }
    Ok(out.trim_end().to_owned())
}

/// 2 defines the symbol, 1 only mentions it, 0 neither or no symbol given.
fn symbol_rank(
    path: &Path,
    symbol: Option<&str>,
    gate: &yi_permission::ReadGate,
    context: &ToolContext,
) -> u8 {
    let Some(symbol) = symbol else { return 0 };
    let opened = gate.open(path, &context.deny_read);
    let Ok(text) = opened.and_then(std::io::read_to_string) else {
        return 0;
    };
    if text
        .lines()
        .any(|line| is_decl(line) && line.contains(symbol))
    {
        2
    } else {
        u8::from(text.contains(symbol))
    }
}

/// Invariant: printed first, because the layer clamp cuts from the end.
fn order_line(ranked: &[(u8, usize, String)], symbol: Option<&str>) -> String {
    let count =
        |keep: fn(&(u8, usize, String)) -> bool| ranked.iter().filter(|row| keep(row)).count();
    let hot = count(|row| row.1 > 0);
    match symbol {
        Some(symbol) => format!(
            "[order: {} defining `{symbol}`, {} mentioning it, {hot} with change heat, then name]\n",
            count(|row| row.0 == 2),
            count(|row| row.0 == 1),
        ),
        None => format!("[order: {hot} with change heat, then name]\n"),
    }
}

pub(crate) fn skeleton(text: &str, cap: usize) -> (Vec<String>, usize) {
    let mut heads: Vec<String> = text
        .lines()
        .filter(|line| DECL_HEADS.iter().any(|head| line.starts_with(head)))
        .map(decl_head)
        .collect();
    let total = heads.len();
    if total > cap {
        heads.truncate(cap);
        heads.push(format!(
            "[{cap} of {total} heads, cap {cap} per file — grep def=true for all]"
        ));
    }
    (heads, total)
}

/// Visibility, export and storage words that may open a declaration before its keyword.
const DECL_PREFIXES: &str =
    "pub crate super export default async unsafe extern static inline public private protected";
/// Words that introduce a named declaration; `const fn` and `static mut` chain two.
const DECL_KEYWORDS: &str = "fn struct enum union trait type const mut mod class def function \
    func interface typedef";
/// Words that open a statement, never a C `type name(` declaration.
const STATEMENT_HEADS: &str = "let var return await yield throw new if while for assert";

fn in_word_list(list: &str, word: &str) -> bool {
    list.split_whitespace().any(|entry| entry == word)
}

fn identifiers(text: &str) -> Vec<&str> {
    text.split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .filter(|word| !word.is_empty())
        .collect()
}

/// The word after a declaration's keyword (a Go method's, past its receiver), past visibility
/// and storage words, or in C the word before the `(` of an unindented `type name(`.
pub(crate) fn defined_name(line: &str) -> Option<String> {
    let indented = line.starts_with(char::is_whitespace);
    let trimmed = line.trim_start();
    (trimmed.starts_with(|first: char| first.is_ascii_alphabetic())).then_some(())?;
    let method = (trimmed.strip_prefix("func (")).and_then(|rest| rest.split_once(')'));
    let trimmed = method.map_or_else(|| trimmed.to_owned(), |(_, name)| format!("func{name}"));
    let prefixed = |word: &&str| in_word_list(DECL_PREFIXES, word);
    let all = identifiers(&trimmed);
    let mut rest = all.iter().copied().skip_while(prefixed).peekable();
    let head = *rest.peek()?;
    if in_word_list(DECL_KEYWORDS, head) {
        return rest
            .find(|word| !in_word_list(DECL_KEYWORDS, word))
            .map(str::to_owned);
    }
    let (text, _) = trimmed.split_once('(')?;
    let before: Vec<&str> = identifiers(text).into_iter().skip_while(prefixed).collect();
    let typed = !indented
        && before.len() >= 2
        && !text.contains(['.', '='])
        && !in_word_list(STATEMENT_HEADS, head);
    typed
        .then(|| before.last().map(|word| (*word).to_owned()))
        .flatten()
}

pub(crate) fn decl_head(line: &str) -> String {
    line.trim_end().trim_end_matches('{').trim_end().to_owned()
}

/// A definition line at any indentation: a method inside an impl or a class counts.
pub(crate) fn is_decl(line: &str) -> bool {
    let trimmed = line.trim_start();
    DECL_HEADS.iter().any(|head| trimmed.starts_with(head))
}

struct Heat {
    counts: BTreeMap<String, usize>,
    truncated: bool,
}

fn change_heat(context: &ToolContext) -> Result<Heat, String> {
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
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for line in capture.stdout.lines().filter(|line| !line.is_empty()) {
        let count = counts.entry(line.to_owned()).or_default();
        *count = count.saturating_add(1);
    }
    if counts.is_empty() {
        return Err("git log named no changed files".to_owned());
    }
    Ok(Heat {
        counts,
        truncated: capture.truncated,
    })
}

fn git_heat(heat: Result<Heat, String>) -> LayerBody {
    let heat = heat?;
    let total = heat.counts.len();
    // A tie on count would otherwise order by hash-map chance; the packet is
    // compared byte for byte across runs.
    let mut rows: Vec<(usize, &str)> = heat.counts.iter().map(|(p, c)| (*c, p.as_str())).collect();
    rows.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(right.1)));
    let mut out = String::new();
    for (count, path) in rows.iter().take(HEAT_ROWS) {
        out.push_str(&format!("{count:>4}  {path}\n"));
    }
    if total > HEAT_ROWS {
        out.push_str(&format!("[heat truncated: {HEAT_ROWS} of {total} paths]\n"));
    }
    if heat.truncated {
        out.push_str("[git log output was capped before counting]\n");
    }
    Ok(out.trim_end().to_owned())
}

fn gates(root: &Path) -> LayerBody {
    let mut found: Vec<&(&str, &str)> = GATES
        .iter()
        .filter(|(file, _)| root.join(file).exists())
        .collect();
    found.dedup_by_key(|(_, gate)| *gate);
    let lines: Vec<String> = found
        .iter()
        .map(|(file, gate)| format!("{gate}  ({file})"))
        .collect();
    if lines.is_empty() {
        return Err("no gate manifest in the working directory".to_owned());
    }
    Ok(lines.join("\n"))
}

fn issues(root: &Path, context: &ToolContext) -> LayerBody {
    let path = root.join(ISSUES_PATH);
    if crate::builtins::walled(&context.deny_read, &path) {
        return Err(format!("{ISSUES_PATH} is denied to this agent"));
    }
    let text = (context.read(&path))
        .and_then(|bytes| String::from_utf8(bytes).map_err(std::io::Error::other))
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

#[cfg(test)]
mod tests {
    #[test]
    fn a_clamped_layer_ends_on_a_whole_row() {
        let clamped = super::clamp("heat", "abcdef\n".repeat(1_000));
        let (kept, marker) = clamped.rsplit_once('\n').unwrap_or_default();
        assert_eq!(marker, "[heat truncated at 4000 bytes]");
        assert!(kept.lines().all(|row| row == "abcdef"), "{kept}");
    }
}
