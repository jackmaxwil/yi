use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::jobs::MAX_TIMEOUT_SECS;
use crate::tool::{
    DETAIL_CAP, Tool, ToolContext, ToolKind, ToolOutput, error_output, require_str, resolve_path,
    text_output,
};

/// The converters a model reaches for by habit; `read` already does their job.
fn document_hint(command: &str) -> Option<String> {
    let verb = command
        .split(['|', ';', '&'])
        .filter_map(|segment| segment.split_whitespace().next())
        .find(|verb| {
            matches!(
                verb.rsplit('/').next().unwrap_or(verb),
                "pdftotext" | "pandoc" | "textutil" | "soffice" | "libreoffice" | "docx2txt"
            )
        })?;
    Some(format!(
        "[read converts office documents and PDFs to Markdown itself; read <path> replaces {verb} here]"
    ))
}

#[derive(Default)]
pub struct WriteTool {
    pub hashline: Option<crate::hashline::tool::SharedHashline>,
}

impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "Write content to a file, creating parent directories and overwriting any existing file."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File path (absolute, or relative to the working directory)"},
                "content": {"type": "string", "description": "Full file content to write"}
            },
            "required": ["path", "content"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Write
    }

    fn preview(&self, input: &Map<String, Value>, cwd: &Path) -> Option<String> {
        let path = input.get("path").and_then(Value::as_str)?;
        let content = input.get("content").and_then(Value::as_str)?;
        let resolved = yi_permission::resolve_target(path, cwd);
        // The diff reaches the approval and the auto-reviewer's model before any check runs.
        let gate = yi_permission::ReadGate::new(&yi_permission::CatastrophicContext::detect(cwd));
        let opened = gate.open(&resolved, &[]);
        let before = opened.and_then(std::io::read_to_string).unwrap_or_default();
        let patch = crate::diff::patch(&before, content, &resolved);
        (!patch.is_empty()).then(|| patch.as_str().to_owned())
    }

    fn execute(&self, input: Map<String, Value>, context: &ToolContext) -> ToolOutput {
        let given = match require_str(&input, "path") {
            Ok(path) => path,
            Err(message) => return error_output(message),
        };
        let path = resolve_path(context, given);
        let content = match require_str(&input, "content") {
            Ok(content) => content,
            Err(message) => return error_output(message),
        };
        let home = self
            .hashline
            .as_ref()
            .and_then(crate::hashline::tool::documents)
            .map(|documents| documents.home);
        let (before, syntax) = match land(home.as_deref(), &path, content, context) {
            Ok(landed) => landed,
            Err(message) => return error_output(message),
        };
        // Incident: the tag minted here was never shown, so 30 F0e edits after a write
        // cited an invented one; the header is the anchor an edit must copy (#473).
        let tag = self
            .hashline
            .as_ref()
            .map(|state| crate::hashline::tool::record_write_snapshot(state, &path, content));
        let mut text = String::new();
        if let Some(tag) = tag {
            text.push_str(&crate::hashline::format::format_hashline_header(given, tag));
            text.push('\n');
        }
        text.push_str(&format!(
            "Wrote {} bytes to {}",
            content.len(),
            path.display()
        ));
        if let Some(line) = &syntax {
            text.push('\n');
            text.push_str(line);
        }
        let mut output = text_output(text);
        if let Some(before) = &before {
            let patch = crate::diff::patch(before, content, &path);
            if !patch.is_empty() {
                output.result.details = crate::diff::patch_details(&patch);
            }
        }
        if let Value::Object(details) = &mut output.result.details {
            details.insert(
                "syntax".to_owned(),
                syntax.map_or(Value::Null, Value::String),
            );
        }
        output
    }
}

/// Approval of a diff is not approval of a path: through a symlink the bytes land elsewhere.
pub(crate) fn symlink_refusal(path: &Path, shown: &str) -> Option<String> {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
        .then(|| {
            format!(
                "{shown} is a symlink; refusing to write through it. Edit the target file directly."
            )
        })
}

/// Invariant: the one write of a named file outside `edit`'s patcher. Returns the ground it
/// replaced (none past [`DETAIL_CAP`], since a missing base reads as an add) and the syntax verdict.
pub(crate) fn land(
    home: Option<&Path>,
    path: &Path,
    content: &str,
    context: &ToolContext,
) -> Result<(Option<String>, Option<String>), String> {
    if let Some(refusal) = crate::document::write_refusal(home, path)
        .or_else(|| symlink_refusal(path, &path.display().to_string()))
    {
        return Err(refusal);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    let failed = |error: std::io::Error| format!("failed to write {}: {error}", path.display());
    let mut file = context
        .read_gate()
        .open_write(path, &context.write_walls())
        .map_err(failed)?;
    let cap = u64::try_from(DETAIL_CAP).unwrap_or(u64::MAX);
    let before = match file.metadata() {
        Ok(meta) if meta.len() > cap => None,
        Ok(_) => Some(std::io::read_to_string(&mut file).unwrap_or_default()),
        Err(_) => Some(String::new()),
    };
    crate::tool::overwrite(&mut file, content.as_bytes()).map_err(failed)?;
    Ok((before, crate::syntax::verdict(path)))
}

pub fn wall_refusal(tool_name: &str, path: &str, list: &str) -> String {
    format!(
        "Denied by the reviewer wall: {tool_name} targets {path}, which this agent's {list} covers. \
         The standard is fixed for the run: report the mismatch instead of changing it."
    )
}

/// Invariant: walled when it or an ancestor is a deny entry by spelling or file identity, so a
/// symlink (dangling too), `..` or letter case cannot reach around it; hard links can.
pub fn walled(deny: &[PathBuf], path: &Path) -> bool {
    yi_permission::beneath(deny, path)
}

/// What a walk left out: entries a wall covers, and whether it stopped at [`WALK_CAP`].
#[derive(Clone, Copy, Default)]
pub(crate) struct Walked {
    pub(crate) walled: usize,
    pub(crate) stopped: bool,
    /// The root is guarded, so nothing under it was walked.
    pub(crate) refused: bool,
}

impl Walked {
    pub(crate) fn notices(self) -> Vec<String> {
        let walled = format!(
            "[{} paths left out: the reviewer wall's deny_read covers them, and no call reaches them this run]",
            self.walled
        );
        let stopped = format!("[the walk stopped after {WALK_CAP} entries: name a narrower path]");
        let refused = "[no walk enters this directory: it is a home, a system root or a key store; \
                       give path= a directory under the working tree]";
        (self.walled > 0)
            .then_some(walled)
            .into_iter()
            .chain(self.stopped.then_some(stopped))
            .chain(self.refused.then(|| refused.to_owned()))
            .collect()
    }
}

/// Entries one walk visits at most, so `/usr/**` ends.
const WALK_CAP: usize = 200_000;

/// Name order: filesystem order differs by machine and every cap would keep a different set.
/// Links are never followed, so each entry is judged by its own path and identity alone.
pub(crate) fn walk_files(
    root: &Path,
    deny: &[PathBuf],
    visit: &mut dyn FnMut(&Path) -> bool,
) -> Walked {
    walk_capped(root, deny, WALK_CAP, visit)
}

fn walk_capped(
    root: &Path,
    deny: &[PathBuf],
    cap: usize,
    visit: &mut dyn FnMut(&Path) -> bool,
) -> Walked {
    let mut walked = Walked::default();
    if walled(deny, root) {
        walked.walled = 1;
        return walked;
    }
    let root = yi_permission::lexical_normalize(root);
    let ids = yi_permission::identities(deny);
    let gate = yi_permission::CatastrophicContext::detect(&root);
    // Judged by identity, so letter case, a link, `/private` or a firmlink name no way in.
    let guard = yi_permission::ReadGate::new(&gate);
    if guard.denies(&root) {
        walked.refused = true;
        return walked;
    }
    let mut ignore = crate::ignore::Ignore::default();
    // Rules above the root bind it, from the nearest repository top down; a root that is one
    // takes none from above.
    let above: Vec<&Path> = root.ancestors().collect();
    if let Some(top) = above.iter().position(|dir| dir.join(".git").exists()) {
        (above.iter().take(top.saturating_add(1)).skip(1).rev())
            .for_each(|dir| ignore.push_dir(dir));
    }
    let mut seen = 0_usize;
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        ignore.push_dir(&dir);
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<fs::DirEntry> = entries.flatten().collect();
        entries.sort_by_key(fs::DirEntry::file_name);
        seen = seen.saturating_add(entries.len());
        if seen > cap {
            walked.stopped = true;
            return walked;
        }
        let mut subdirs: Vec<PathBuf> = Vec::new();
        for entry in entries {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            // Every guarded identity is a directory's but a linked worktree's `.git` file, which
            // the ignore rules drop by name, so a file's is read only under a wall.
            let meta = (file_type.is_dir() || !ids.is_empty())
                .then(|| entry.metadata().ok())
                .flatten();
            if guard.denies_entry(meta.as_ref()) {
                continue;
            }
            if yi_permission::lexically_beneath(deny, &path)
                || yi_permission::denied_file(&ids, meta.as_ref())
            {
                walked.walled = walked.walled.saturating_add(1);
            } else if file_type.is_dir() {
                if !ignore.ignored(&path, true) {
                    subdirs.push(path);
                }
            } else if file_type.is_file() && !ignore.ignored(&path, false) && !visit(&path) {
                return walked;
            }
        }
        stack.extend(subdirs.into_iter().rev());
    }
    walked
}

/// Every other verb keeps the `Exec` default: the flag warns about blast
/// radius, and this is the set with none.
const READ_ONLY_VERBS: [&str; 24] = [
    "ls", "cat", "head", "tail", "wc", "pwd", "echo", "printf", "which", "type", "file", "stat",
    "du", "df", "date", "env", "printenv", "grep", "rg", "ag", "find", "fd", "diff", "true",
];

/// `git` is half a read tool: only the reporting subcommands qualify.
const READ_ONLY_GIT: [&str; 10] = [
    "log",
    "status",
    "diff",
    "show",
    "blame",
    "branch",
    "describe",
    "rev-parse",
    "ls-files",
    "shortlog",
];

/// Grid's query verbs read the chart; `survey` and `edit` write.
const READ_ONLY_GRID: [&str; 14] = [
    "resolve", "uses", "scope", "todo", "roots", "explain", "version", "orphans", "hotspots",
    "log", "diff", "drift", "check", "help",
];

/// Incident: every bash call reported as irreversible, so the advisor flagged `ls -la && git
/// log` into meaninglessness. Each segment is screened; all must be read-only.
fn read_only_command(command: &str) -> bool {
    command
        .split(['|', ';', '\n'])
        .flat_map(|part| part.split("&&"))
        .all(read_only_segment)
}

fn read_only_segment(segment: &str) -> bool {
    let tokens: Vec<&str> = segment
        .split_whitespace()
        // `2>/dev/null` overwrites nothing that exists.
        .filter(|token| !(token.contains('>') && token.ends_with("/dev/null")))
        .collect();
    if tokens
        .iter()
        .any(|token| token.contains('>') || token.contains('`') || token.contains("$("))
    {
        return false;
    }
    let mut words = tokens.iter().skip_while(|word| word.contains('='));
    let Some(verb) = words.next() else {
        return true;
    };
    let verb = verb.rsplit('/').next().unwrap_or(verb);
    match verb {
        "git" => words.next().is_some_and(|sub| READ_ONLY_GIT.contains(sub)),
        "grid" => words.next().is_some_and(|sub| READ_ONLY_GRID.contains(sub)),
        _ => READ_ONLY_VERBS.contains(&verb),
    }
}

const BROAD_ROOTS: [&str; 10] = [
    "/", "/*", "~", "$HOME", "${HOME}", "/home", "/opt", "/root", "/usr", "/var",
];

fn broad_root(token: &str) -> bool {
    let trimmed = if token.len() > 1 {
        token.trim_end_matches('/')
    } else {
        token
    };
    BROAD_ROOTS.contains(&trimmed)
}

/// A search that walks a whole tree with no bound is refused before it spawns: the gate calls
/// `find /` safe, and one rollout spent its whole attempt inside `find / -name slug.py`.
pub(crate) fn broad_search(command: &str) -> Option<String> {
    command
        .split(['|', ';', '\n'])
        .flat_map(|part| part.split("&&"))
        .find_map(broad_segment)
}

// ponytail: whitespace tokens, no `timeout`/`nice` passthrough; add when a rollout shows one.
fn broad_segment(segment: &str) -> Option<String> {
    let words: Vec<&str> = segment
        .split_whitespace()
        .filter(|word| !word.contains('>'))
        .skip_while(|word| word.contains('='))
        .collect();
    let (verb, rest) = words.split_first()?;
    let verb = verb.rsplit('/').next().unwrap_or(verb);
    let short = |letter: char| {
        rest.iter()
            .any(|word| word.starts_with('-') && !word.starts_with("--") && word.contains(letter))
    };
    let long = |name: &str| rest.iter().any(|word| word.starts_with(name));
    let (walks, bound, hint) = match verb {
        "find" => (
            true,
            rest.iter()
                .any(|word| matches!(*word, "-maxdepth" | "-prune" | "-quit")),
            "add -maxdepth N",
        ),
        "grep" => (
            short('r') || short('R') || long("--recursive") || long("--dereference-recursive"),
            false,
            "drop -r",
        ),
        "rg" => (
            true,
            long("--max-depth") || long("--maxdepth"),
            "add --max-depth N",
        ),
        "du" => (true, short('d') || long("--max-depth"), "add -d N"),
        "ls" => (short('R') || long("--recursive"), false, "drop -R"),
        _ => return None,
    };
    if !walks || bound {
        return None;
    }
    let pattern_named = rest
        .iter()
        .any(|word| matches!(*word, "-e" | "--regexp" | "-f"));
    let skip = usize::from(matches!(verb, "grep" | "rg") && !pattern_named);
    let root = rest
        .iter()
        .filter(|word| !word.starts_with('-'))
        .skip(skip)
        .find(|word| broad_root(word))?;
    let scope = if matches!(*root, "/" | "/*") {
        "the whole filesystem".to_owned()
    } else {
        format!("all of {root}")
    };
    Some(format!(
        "[refused: `{verb} {root}` walks {scope}; search from the cwd, {hint}, or name the directory you expect]"
    ))
}

/// Persisted per call so a stats pass can see shell searches the grep tool
/// should have served.
pub(crate) fn command_category(command: &str) -> &'static str {
    let mut categories = command
        .split(['|', ';', '\n'])
        .flat_map(|part| part.split("&&"))
        .filter_map(segment_category);
    let Some(first) = categories.next() else {
        return "unknown";
    };
    if categories.all(|category| category == first) {
        first
    } else {
        "mixed"
    }
}

fn segment_category(segment: &str) -> Option<&'static str> {
    let mut words = segment
        .split_whitespace()
        .skip_while(|word| word.contains('='));
    let verb = words.next()?;
    let verb = verb.rsplit('/').next().unwrap_or(verb);
    Some(match verb {
        "grep" | "rg" | "ag" => "search",
        "cat" | "head" | "tail" | "less" | "more" | "wc" | "file" | "stat" => "read",
        "ls" | "find" | "fd" | "tree" => "list_files",
        "git" | "jj" => "vcs",
        "cargo" | "just" => match words.next() {
            Some("test" | "nextest") => "test",
            _ => "build",
        },
        "make" | "npm" | "pnpm" | "yarn" | "go" | "rustc" | "cc" | "gcc" | "tsc" => "build",
        "pytest" => "test",
        "mkdir" | "touch" | "rm" | "mv" | "cp" | "chmod" | "ln" | "tee" => "write",
        // `sed -n 'Np'` is the read tool's clip escape hatch; only `-i` writes.
        "sed" => {
            if words.any(|word| word.starts_with("-i")) {
                "write"
            } else {
                "read"
            }
        }
        _ => "unknown",
    })
}

#[derive(Default)]
pub struct BashTool {
    pub hashline: Option<crate::hashline::tool::SharedHashline>,
    /// The last four calls that ran, newest in bit 0; a set bit hit its time limit.
    pub(crate) recent_ceilings: std::sync::atomic::AtomicU8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SkipReason {
    Compound,
    NotAView,
    NotOneFile,
}

enum Bridge {
    Tag { typed: String, path: PathBuf },
    Skip(SkipReason),
}

impl SkipReason {
    fn name(self) -> &'static str {
        match self {
            Self::Compound => "compound",
            Self::NotAView => "not-a-view",
            Self::NotOneFile => "not-one-file",
        }
    }
}

const VIEW_VERBS: [&str; 6] = ["cat", "head", "tail", "sed", "rg", "grep"];

/// One plain view of one existing file can anchor an edit: the command is a single segment,
/// its verb prints file text, and exactly one argument names a regular file.
fn bridge_target(command: &str, cwd: &Path) -> Bridge {
    if command.contains(['|', ';', '\n', '>', '`'])
        || command.contains("&&")
        || command.contains("$(")
    {
        return Bridge::Skip(SkipReason::Compound);
    }
    let mut tokens = command
        .split_whitespace()
        .skip_while(|token| token.contains('='));
    let verb = tokens
        .next()
        .map(|verb| verb.rsplit('/').next().unwrap_or(verb));
    if !verb.is_some_and(|verb| VIEW_VERBS.contains(&verb)) {
        return Bridge::Skip(SkipReason::NotAView);
    }
    let files: Vec<(String, PathBuf)> = tokens
        .filter(|token| !token.starts_with('-'))
        .map(|token| (token.to_owned(), cwd.join(token)))
        .filter(|(_, path)| path.is_file())
        .collect();
    match files.as_slice() {
        [(typed, path)] => Bridge::Tag {
            typed: typed.clone(),
            path: path.clone(),
        },
        _ => Bridge::Skip(SkipReason::NotOneFile),
    }
}

/// What a bash view put in front of the model.
enum Shown<'a> {
    /// A bare `cat` whose output reached the model uncut.
    Whole,
    Text(&'a str),
}

fn shown<'a>(command: &str, reduced: &'a crate::reduce::Reduced, truncated: bool) -> Shown<'a> {
    let bare_cat = command
        .split_whitespace()
        .next()
        .is_some_and(|verb| verb.rsplit('/').next() == Some("cat"))
        && !command
            .split_whitespace()
            .any(|token| token.starts_with('-'));
    if bare_cat && !truncated && reduced.out_bytes == reduced.raw_bytes {
        Shown::Whole
    } else {
        Shown::Text(&reduced.text)
    }
}

/// Records the view against the file as it is now and returns its `[path#TAG]` header.
fn tagged_header(
    state: &crate::hashline::tool::SharedHashline,
    typed: &str,
    path: &Path,
    view: &Shown<'_>,
    context: &ToolContext,
) -> Option<String> {
    let content = String::from_utf8(context.read(path).ok()?).ok()?;
    let seen = viewed_lines(&content, view);
    let tag = crate::hashline::tool::record_view_snapshot(state, path, &content, &seen);
    Some(crate::hashline::format::format_hashline_header(typed, tag))
}

/// A line under three characters (`}`, `x`) is in almost any output, so it counts as seen only
/// when the output shows it beside a neighbour that counts on its own.
fn viewed_lines(content: &str, shown: &Shown<'_>) -> Vec<u64> {
    let lines: Vec<&str> = content.split('\n').collect();
    let output = match shown {
        Shown::Whole => "",
        Shown::Text(output) => output,
    };
    let rows: Vec<&str> = output.split('\n').collect();
    let pairs: std::collections::HashSet<(&str, &str)> = rows
        .iter()
        .copied()
        .zip(rows.iter().copied().skip(1))
        .collect();
    let alone = |line: &str| line.trim().len() >= 3 && output.contains(line.trim());
    let beside = |index: usize, line: &str| {
        let before = index
            .checked_sub(1)
            .and_then(|prev| lines.get(prev))
            .is_some_and(|&prev| alone(prev) && pairs.contains(&(prev, line)));
        before
            || lines
                .get(index.saturating_add(1))
                .is_some_and(|&next| alone(next) && pairs.contains(&(line, next)))
    };
    lines
        .iter()
        .enumerate()
        .filter(|&(index, &line)| match shown {
            Shown::Whole => true,
            Shown::Text(_) => alone(line) || beside(index, line),
        })
        .filter_map(|(index, _)| u64::try_from(index).ok()?.checked_add(1))
        .collect()
}

/// Incident: layout-config-recreation's cv2 search died three times under its own 590 s and 480 s
/// `timeout` wrappers, as exit 124. A wrapped command that exits 124 by itself ends sooner.
fn own_timeout_fired(command: &str, exit_code: i32, elapsed: std::time::Duration) -> bool {
    exit_code == 124
        && command
            .split(['|', ';', '\n', '&', '('])
            .filter_map(timeout_limit)
            .any(|limit| elapsed >= limit)
}

fn timeout_limit(segment: &str) -> Option<std::time::Duration> {
    let mut words = segment
        .split_whitespace()
        .skip_while(|word| word.contains('='));
    if words.next()?.rsplit('/').next()? != "timeout" {
        return None;
    }
    let limit = loop {
        let word = words.next()?;
        if matches!(word, "-k" | "-s" | "--kill-after" | "--signal") {
            words.next()?;
        } else if !word.starts_with('-') {
            break word;
        }
    };
    let scale = match limit.chars().last()? {
        'm' => 60.0,
        'h' => 3600.0,
        'd' => 86400.0,
        _ => 1.0,
    };
    let seconds: f64 = limit.trim_end_matches(['s', 'm', 'h', 'd']).parse().ok()?;
    std::time::Duration::try_from_secs_f64(seconds * scale).ok()
}

/// Invariant: a list's status is its last command's, so only a final top-level `&&` can
/// mean a later segment was skipped.
fn ends_in_and_chain(command: &str) -> bool {
    let mut chars = command.trim_end().chars().peekable();
    let (mut quote, mut depth, mut prev, mut and_last) = (None, 0usize, ' ', false);
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), _) => {}
            (_, '\\') => {
                chars.next();
            }
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(c),
            (None, '(') => depth = depth.saturating_add(1),
            (None, ')') => depth = depth.saturating_sub(1),
            (None, '&') if matches!(prev, '>' | '<' | '|') || chars.peek() == Some(&'>') => {}
            (None, '&' | '|' | ';' | '\n') if depth == 0 => {
                and_last = c == '&' && chars.next_if_eq(&'&').is_some();
                if c == '|' {
                    chars.next_if_eq(&'|');
                }
            }
            (None, _) => {}
        }
        prev = c;
    }
    and_last
}

/// Where no `bash` is on PATH the tool falls back to POSIX `sh`, so a bashism is a shell
/// refusal the model cannot see in the output; named only when the shape matches (#476).
fn posix_shell_hint(command: &str, exit_code: i32, stderr: &str) -> Option<String> {
    const BASHISMS: [&str; 4] = ["<(", ">(", "[[", "declare"];
    let shaped = exit_code == 2
        && crate::jobs::interpreter() == "sh"
        && stderr.contains("Syntax error")
        && BASHISMS.iter().any(|token| command.contains(token));
    shaped.then(|| {
        "[this shell is POSIX sh, not bash: process substitution, [[ ]] and declare need \
`bash -c '...'` or a temp file]"
            .to_owned()
    })
}

impl BashTool {
    /// How many of the last four calls hit their time limit, when this one did and one more did.
    fn ceiling_nudge(
        &self,
        command: &str,
        timed_out: bool,
        exit_code: i32,
        elapsed: std::time::Duration,
    ) -> Option<u32> {
        let hit = timed_out || own_timeout_fired(command, exit_code, elapsed);
        let push = |bits: u8| (bits << 1 | u8::from(hit)) & 0b1111;
        let ordering = std::sync::atomic::Ordering::Relaxed;
        let (Ok(before) | Err(before)) =
            self.recent_ceilings
                .fetch_update(ordering, ordering, |bits| Some(push(bits)));
        let hits = push(before).count_ones();
        (hit && hits >= 2).then_some(hits)
    }
}

impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Run a shell command with bash -c (sh where bash is absent) in the working directory and return its output and exit code. Each call starts fresh in the session's working directory; a `cd` does not carry over to the next call. `a && b` stops at the first nonzero segment, so later segments silently never run: a chain that stopped is reported, a truncation is not the cause. Output over 30,000 bytes per stream keeps its first and last 15,000 bytes ([N bytes omitted from the middle]), and over 8,192 bytes it is reduced ([N lines omitted: A-B]); a cut or reduced output names [full output: path], a file with every byte (the first 256 MiB), which read opens; max_output_lines raises the reducer's budget and -v/--verbose bypass it. Pass wait (5-300 s, below timeout_secs) with a command to get the turn back: still running then, it becomes a job. Its result reaches you on its own only if it exits while your turn is still running, so before you end the turn, wait for it: bash with job=N, wait and no command returns once it exits or wait passes. A command is killed at timeout_secs (default 300 s, ceiling 600 s), job or not; raise it for a build or a test suite. An unbounded walk of / or ~ (find /, grep -r … /, rg … /, du /, ls -R /) is refused before it runs: search from the cwd, bound it (-maxdepth, --max-depth, -d), or name the directory. Where a sandbox exists (macOS) every command runs contained, outside yolo mode: no network except loopback, no unix socket, writes only under cwd, its git dirs and tmp, no reads of credential stores or walled paths; a PermissionDenied there says nothing about the code. After a refusal the next such call asks, and approving widens that run by the refused directory; \"always\" on a refusal naming no path passes that exact command outside for the session. Unasked, only a command needing the network, an install or a credential store runs outside, and its result says so; a compound mixing such a part with one that could stay inside, or one Yi cannot parse, asks. An approval runs outside only where its question says so. A container child's commands have its image's network."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "Shell command to run. Omit it to check on a background job instead."},
                "job": {"type": "integer", "description": "Background job to check on; defaults to the most recent"},
                "wait": {"type": "integer", "description": "With command: seconds, below timeout_secs, before a still-running command becomes a background job. Without: seconds to wait for that job. Clamped 5-300."},
                "timeout_secs": {"type": "integer", "description": "Wall-clock limit in seconds, default 300, ceiling 600; raise it for a build or a test suite. Past it the command is killed and reported as timed out."},
                "max_output_lines": {"type": "integer", "description": "Per-call reducer line budget, for when the full output matters"}
            }
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Exec
    }

    fn kind_for(&self, input: &Map<String, Value>) -> ToolKind {
        match input.get("command").and_then(Value::as_str) {
            Some(command) if read_only_command(command) => ToolKind::Read,
            Some(_) | None => ToolKind::Exec,
        }
    }

    fn irreversible(&self, input: &Map<String, Value>) -> bool {
        match input.get("command").and_then(Value::as_str) {
            Some(command) => !read_only_command(command),
            None => false,
        }
    }

    fn execute(&self, input: Map<String, Value>, context: &ToolContext) -> ToolOutput {
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if command.trim().is_empty() {
            return poll_job(&input, &context.cancelled);
        }
        if let Some(refusal) = broad_search(command) {
            return error_output(refusal);
        }
        let requested_timeout = input.get("timeout_secs").and_then(Value::as_u64);
        let timeout = crate::jobs::clamp_timeout(requested_timeout);
        let asked_wait = input.get("wait").and_then(Value::as_u64);
        let background_after = asked_wait
            .map(crate::jobs::clamp_wait)
            .or(context.auto_background);
        let started = std::time::Instant::now();
        let mut sections = Vec::new();
        if let Some(asked) = requested_timeout.filter(|asked| *asked > MAX_TIMEOUT_SECS) {
            sections.push(format!(
                "[timeout_secs {asked} capped at {MAX_TIMEOUT_SECS}]"
            ));
        }
        let running = yi_types::trace::span("bash.run");
        let placed = (context.container.as_deref())
            .map(|name| crate::jobs::in_container(name, &context.cwd, command));
        let (capture, timed_out) = match crate::jobs::run_or_background(
            placed.as_ref().map_or(command, |(run, _)| run),
            context,
            background_after,
            timeout,
            context.sandbox.as_ref().filter(|_| placed.is_none()),
            placed.as_ref().map(|(_, sweep)| sweep.as_str()),
        ) {
            Ok(crate::jobs::Run::Finished(capture)) => (*capture, false),
            Ok(crate::jobs::Run::TimedOut(capture)) => (*capture, true),
            Ok(crate::jobs::Run::Backgrounded(id)) => {
                let after = background_after.unwrap_or_default();
                return backgrounded_output(sections, id, after, timeout);
            }
            Err(message) => return error_output(message),
        };
        drop(running);
        let _format = yi_types::trace::span("bash.format");
        let exit_code_for_reduce = capture.exit_code.unwrap_or(-1);
        let nudge = self.ceiling_nudge(command, timed_out, exit_code_for_reduce, started.elapsed());
        let max_lines = input
            .get("max_output_lines")
            .and_then(Value::as_u64)
            .and_then(|lines| usize::try_from(lines).ok())
            .filter(|lines| *lines > 0);
        let reduced = crate::reduce::reduce(
            command,
            &capture.stdout,
            &capture.stderr,
            exit_code_for_reduce,
            capture.spill.as_deref(),
            context
                .recovery_dir
                .as_deref()
                .filter(|_| !capture.truncated),
            max_lines,
        );
        let body = match (&reduced.recovery, capture.truncated) {
            (Some(note), true) => (reduced.text.strip_suffix(note.as_str()))
                .map_or(reduced.text.as_str(), str::trim_end),
            _ => reduced.text.as_str(),
        };
        if !body.is_empty() {
            sections.push(body.to_owned());
        }
        sections.extend(capture.cut_row());
        sections.extend(capture.cut_note());
        if timed_out {
            sections.extend(timed_out_notes(timeout, nudge.is_some(), asked_wait));
        } else if capture.cancelled {
            sections.push("[command aborted]".to_owned());
        }
        if let Some(error) = &capture.kill_error {
            sections.push(format!("[group kill failed: {error}]"));
        }
        let exit_code = capture.exit_code.unwrap_or(-1);
        if exit_code != 0 {
            sections.push(format!("exit code: {exit_code}"));
            if ends_in_and_chain(command) {
                sections.push(format!(
                    "[exit {exit_code} inside a && chain: any segment after the failing one did not run]"
                ));
            }
            if let Some(hint) = posix_shell_hint(command, exit_code, &capture.stderr) {
                sections.push(hint);
            }
        }
        if let Some(hits) = nudge {
            sections.push(format!(
                "[{hits} of the last 4 bash calls hit their time limit; change the method, not the limit: bound the work (a smaller input, a sample, an early exit), vectorize it, or run it in the background (nohup CMD > out.log 2>&1 &) and read the log in a later call]"
            ));
        }
        if let Some(hint) = document_hint(command) {
            sections.push(hint);
        }
        // The raw output, not the reduced text: a refusal line the reducer cut still counts.
        let raw = format!("{}{}", capture.stdout, capture.stderr);
        let refusal = context.sandbox.as_ref().and_then(|sandbox| {
            crate::sandbox::sandbox_refusal(sandbox, &context.cwd, capture.exit_code, &raw, command)
        });
        sections.extend(refusal.as_ref().map(crate::sandbox::denial_hint));
        let note = (context.sandbox.as_ref())
            .and_then(|_| crate::sandbox::outcome_note(&raw, command, exit_code));
        sections.extend(note);
        let mut text = if sections.is_empty() {
            "(no output)".to_owned()
        } else {
            sections.join("\n")
        };
        let bridge = match &self.hashline {
            Some(state) if exit_code == 0 && !capture.cancelled => {
                match bridge_target(command, &context.cwd) {
                    Bridge::Tag { typed, path } => {
                        let view = shown(command, &reduced, capture.truncated);
                        match tagged_header(state, &typed, &path, &view, context) {
                            Some(header) => {
                                text = format!("{header}\n{text}");
                                "tag"
                            }
                            None => SkipReason::NotOneFile.name(),
                        }
                    }
                    Bridge::Skip(reason) => reason.name(),
                }
            }
            Some(_) | None => SkipReason::NotAView.name(),
        };
        let mut output = text_output(text);
        output.result.details = json!({
            "exitCode": exit_code,
            "truncated": capture.truncated,
            "cancelled": capture.cancelled,
            "timedOut": timed_out,
            "rawBytes": reduced.raw_bytes,
            "outBytes": reduced.out_bytes,
            "category": command_category(command),
            "bridge": bridge,
            "sandboxRefusal": refusal.as_ref().map(crate::sandbox::SandboxRefusal::to_json),
        });
        output.is_error = exit_code != 0 || capture.cancelled;
        output
    }
}

/// A command still running at its wait: the turn gets the job and a bounded tail printed so far.
fn backgrounded_output(
    mut sections: Vec<String>,
    id: crate::jobs::JobId,
    after: std::time::Duration,
    timeout: std::time::Duration,
) -> ToolOutput {
    const TAIL_LINES: usize = 20;
    const TAIL_BYTES: usize = 2_048;
    let timeout = timeout.as_secs();
    sections.push(format!(
        "[still running after {after:?}: now job {id}, killed {timeout}s after it started (timeout_secs) or by an interrupt (Esc, the deadline). Its result reaches you on its own only if it exits while this turn is still running; before you end the turn, wait for it with bash job={id} wait=<s>.]"
    ));
    let chunk = crate::jobs::registry().output_since(id, 0);
    let (so_far, printed) = chunk.map_or((String::new(), 0), |chunk| (chunk.text, chunk.next));
    let lines: Vec<&str> = so_far.lines().collect();
    let tail = lines.get(lines.len().saturating_sub(TAIL_LINES)..);
    let tail = tail.unwrap_or_default().join("\n");
    let from = (tail.len().saturating_sub(TAIL_BYTES)..tail.len())
        .find(|at| tail.is_char_boundary(*at))
        .unwrap_or(tail.len());
    if lines.len() > TAIL_LINES || from > 0 {
        sections.push(format!(
            "[the last {} of {} bytes printed so far (the preview keeps {TAIL_LINES} lines, at most {TAIL_BYTES} bytes); bash job={id} wait=<s> returns its output once it exits]",
            tail.len().saturating_sub(from),
            printed
        ));
    }
    sections.extend(
        tail.get(from..)
            .filter(|kept| !kept.is_empty())
            .map(str::to_owned),
    );
    let mut output = text_output(sections.join("\n"));
    output.result.details = json!({
        "job": id.0, "backgrounded": true, "timeoutSecs": timeout,
        "afterMs": u64::try_from(after.as_millis()).unwrap_or(u64::MAX),
    });
    output
}

/// The timeout's remedy, unless the nudge names one, and why a `wait` did not make a job.
fn timed_out_notes(timeout: std::time::Duration, nudged: bool, asked: Option<u64>) -> Vec<String> {
    let secs = timeout.as_secs();
    let mut notes = vec![if nudged {
        format!("[timed out after {secs}s]")
    } else {
        format!(
            "[timed out after {secs}s; pass timeout_secs up to {} for a longer run, or narrow the command]",
            crate::jobs::MAX_TIMEOUT_SECS
        )
    }];
    if let Some(asked) = asked.filter(|asked| crate::jobs::clamp_wait(*asked) >= timeout) {
        let wait = crate::jobs::clamp_wait(asked).as_secs();
        let clamped = if wait == asked {
            String::new()
        } else {
            format!(" (clamped to {wait}s)")
        };
        notes.push(format!(
            "[wait {asked}s{clamped} is not below timeout_secs {secs}s, so it ran in the turn; to get the turn back, pass a wait below timeout_secs or raise timeout_secs above the wait]"
        ));
    }
    notes
}

/// The same tool with no command; it holds the job's report so what it returns is not resent.
fn poll_job(input: &Map<String, Value>, cancelled: &crate::CancelFlag) -> ToolOutput {
    let jobs = crate::jobs::registry();
    let requested = input
        .get("job")
        .and_then(Value::as_u64)
        .map(crate::jobs::JobId);
    let deadline = input
        .get("wait")
        .and_then(Value::as_u64)
        .map(crate::jobs::clamp_wait);
    let started = std::time::Instant::now();
    let _span = yi_types::trace::span("wait.job");
    let Some(id) = requested.or_else(|| jobs.latest().map(|report| report.id)) else {
        return error_output("no background job to check on".to_owned());
    };
    jobs.set_reported(id, true);
    loop {
        let Some(report) = jobs.report(id) else {
            return error_output("no background job to check on".to_owned());
        };
        if report.finished {
            jobs.mark_delivered(id);
            let exit_code = report.exit_code.unwrap_or(-1);
            let mut output = text_output(format!("{}\n{}", report.headline(), report.output));
            output.result.details = json!({ "job": id.0, "exitCode": exit_code });
            output.is_error = exit_code != 0;
            return output;
        }
        match deadline {
            // In slices, so an interrupt (Esc, the deadline) ends the wait within a second.
            Some(limit) if started.elapsed() < limit && !cancelled() => {
                let left = limit.saturating_sub(started.elapsed());
                jobs.wait_settled(Some(id), left.min(std::time::Duration::from_secs(1)));
            }
            _ => {
                jobs.set_reported(id, false);
                let mut output = text_output(report.headline());
                output.result.details = json!({ "job": id.0, "running": true });
                return output;
            }
        }
    }
}

/// Gitignore-filtered and sorted, for surfaces offering a file picker.
pub fn list_files(root: &Path, cap: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    walk_files(root, &[], &mut |path| {
        if let Ok(relative) = path.strip_prefix(root) {
            out.push(relative.to_string_lossy().into_owned());
        }
        out.len() < cap
    });
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::{BashTool, broad_search};
    use crate::Tool;
    use std::time::Duration;

    /// A walk that reaches its cap says so, matched or not, and visits nothing past it.
    #[test]
    fn a_walk_past_its_cap_stops_and_says_so() -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!("yi-walk-cap-{}", std::process::id()));
        std::fs::create_dir_all(root.join("d"))?;
        for name in ["a", "b", "d/c", "d/e"] {
            std::fs::write(root.join(name), "x")?;
        }
        let mut visited = 0_usize;
        let walked = super::walk_capped(&root, &[], 4, &mut |_| {
            visited += 1;
            true
        });
        let _ = std::fs::remove_dir_all(&root);
        assert!(
            walked.stopped && visited == 2,
            "stopped {} after {visited}",
            walked.stopped
        );
        assert!(
            walked
                .notices()
                .iter()
                .any(|row| row.contains("the walk stopped"))
        );
        Ok(())
    }

    #[test]
    fn the_bash_description_names_the_reducer_floor() {
        let floor = crate::reduce::REDUCE_FLOOR;
        let thousands = format!("{},{:03}", floor / 1000, floor % 1000);
        assert!(
            BashTool::default()
                .description()
                .contains(&format!("over {thousands} bytes it is reduced")),
            "the description must name REDUCE_FLOOR ({floor})"
        );
    }

    #[test]
    fn the_bash_description_names_the_timeout_default_and_ceiling() {
        let tool = BashTool::default();
        let description = tool.description();
        assert!(
            description
                .contains(format!("default {} s", crate::jobs::DEFAULT_TIMEOUT_SECS).as_str()),
            "the description must name DEFAULT_TIMEOUT_SECS"
        );
        assert!(
            description.contains(format!("ceiling {} s", crate::jobs::MAX_TIMEOUT_SECS).as_str()),
            "the description must name MAX_TIMEOUT_SECS"
        );
    }

    #[test]
    fn broad_search_refuses_root_walks_and_passes_bounded_ones() {
        let refused = [
            "find / -name slug.py",
            "find / -name x 2>/dev/null",
            "find ~ -type f",
            "find $HOME -name x",
            "find /usr/ -name x",
            "find -L / -name x",
            "grep -rn slug /",
            "grep -R slug /home",
            "grep --recursive -e slug /",
            "rg slug /",
            "rg -t py slug /opt",
            "du -sh /",
            "du /var",
            "ls -R /",
            "ls -lR ~/",
            "cd x && find / -name y",
        ];
        for command in refused {
            assert!(
                broad_search(command).is_some(),
                "{command} should be refused"
            );
        }
        assert_eq!(
            broad_search("find / -name slug.py").as_deref(),
            Some(
                "[refused: `find /` walks the whole filesystem; search from the cwd, add -maxdepth N, or name the directory you expect]"
            )
        );
        assert_eq!(
            broad_search("du /var").as_deref(),
            Some(
                "[refused: `du /var` walks all of /var; search from the cwd, add -d N, or name the directory you expect]"
            )
        );
        let allowed = [
            "find . -name x",
            "find crates -name x",
            "find /app -name x",
            "find / -maxdepth 2 -name x",
            "find / -name x -quit",
            "grep slug /",
            "grep -rn slug .",
            "grep -r / src",
            "rg slug",
            "rg --max-depth 2 slug /",
            "du -d 1 /",
            "du -sh /var/log",
            "ls -R",
            "ls -la /",
            "locate slug.py",
            "cat /etc/hosts | grep -r x .",
        ];
        for command in allowed {
            assert!(broad_search(command).is_none(), "{command} should run");
        }
    }

    #[test]
    fn the_ceiling_nudge_fires_on_a_second_hit_within_four_calls() {
        // Invented bash results, one session per case; the 2026-09-11 sweep replay cited by D190
        // read the real transcripts outside the repository and is not committed.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/bash-time-limits.jsonl");
        let rows = std::fs::read_to_string(path).unwrap();
        let mut tools = std::collections::BTreeMap::<String, BashTool>::new();
        let mut fired = Vec::new();
        for line in rows.lines() {
            let row: serde_json::Value = serde_json::from_str(line).unwrap();
            let session = row["session"].as_str().unwrap();
            let nudge = tools.entry(session.to_owned()).or_default().ceiling_nudge(
                row["command"].as_str().unwrap(),
                row["timedOut"].as_bool().unwrap(),
                i32::try_from(row["exitCode"].as_i64().unwrap()).unwrap(),
                Duration::from_millis(row["durationMs"].as_u64().unwrap()),
            );
            if nudge.is_some() {
                fired.push(format!("{session} call {}", row["call"]));
            }
        }
        assert_eq!((rows.lines().count(), tools.len()), (44, 12));
        assert_eq!(
            fired,
            [
                "kill-then-own-timeout call 3",
                "kill-two-successes-kill call 4",
                "own-timeouts-in-a-row call 2",
                "own-timeouts-in-a-row call 3",
                "search-kills-in-a-row call 2",
                "kill-after-and-signal-options call 3",
                "env-path-and-chain-before-timeout call 2",
                "env-path-and-chain-before-timeout call 3",
                "later-segment-exits-124-misfires call 2"
            ]
        );
    }
}
