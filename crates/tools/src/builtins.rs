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
        let before = fs::read_to_string(&resolved).unwrap_or_default();
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
        if let Some(refusal) = crate::document::write_refusal(home.as_deref(), &path) {
            return error_output(refusal);
        }
        if let Some(parent) = path.parent()
            && let Err(error) = fs::create_dir_all(parent)
        {
            return error_output(format!("failed to create {}: {error}", parent.display()));
        }
        // Read before the write: nothing else reconstructs the ground it replaced. Past the
        // cap no base is read and no patch claimed, since a missing base reads as an add.
        let cap = u64::try_from(DETAIL_CAP).unwrap_or(u64::MAX);
        let before = match fs::metadata(&path) {
            Ok(meta) if meta.len() > cap => None,
            Ok(_) => Some(fs::read_to_string(&path).unwrap_or_default()),
            Err(_) => Some(String::new()),
        };
        match fs::write(&path, content) {
            Ok(()) => {
                // Incident: the tag minted here was never shown, so 30 F0e edits after a write
                // cited an invented one; the header is the anchor an edit must copy (#473).
                let tag = self.hashline.as_ref().map(|state| {
                    crate::hashline::tool::record_write_snapshot(state, &path, content)
                });
                let syntax = crate::syntax::verdict(&path);
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
            Err(error) => error_output(format!("failed to write {}: {error}", path.display())),
        }
    }
}

pub(crate) fn walk_files(root: &Path, visit: &mut dyn FnMut(&Path) -> bool) {
    let mut ignore = crate::ignore::Ignore::default();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        ignore.push_dir(&dir);
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                if !ignore.ignored(&path, true) {
                    stack.push(path);
                }
            } else if file_type.is_file() && !ignore.ignored(&path, false) && !visit(&path) {
                return;
            }
        }
    }
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

/// Bare `cat` shows every row; any other view shows the rows whose text is in the output.
fn viewed_lines(command: &str, content: &str, output: &str) -> Vec<u64> {
    let bare_cat = command
        .split_whitespace()
        .next()
        .is_some_and(|verb| verb.rsplit('/').next() == Some("cat"))
        && !command
            .split_whitespace()
            .any(|token| token.starts_with('-'));
    content
        .split('\n')
        .enumerate()
        .filter(|(_, line)| {
            let trimmed = line.trim();
            bare_cat || (trimmed.len() >= 3 && output.contains(trimmed))
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
        "Run a shell command with bash -c (sh where bash is absent) in the working directory and return its output and exit code. Each call starts fresh in the session's working directory; a `cd` does not carry over to the next call. `a && b` stops at the first nonzero segment and `x | head` exits 141, so later segments silently never run: a chain that stopped is reported, a truncation is not the cause. Output over 30,000 bytes per stream is cut with [output truncated]; over 8,192 bytes it is reduced ([N lines omitted: A-B]) and the full text is at [full output: path], which read opens; max_output_lines raises the reducer's budget and -v/--verbose bypass it. wait is clamped 5-300 s; a longer command becomes a job you check by calling bash with no command. A command is killed at timeout_secs (default 300 s, ceiling 600 s); raise it for a build or a test suite. An unbounded walk of / or ~ (find /, grep -r … /, rg … /, du /, ls -R /) is refused before it runs: search from the cwd, bound it (-maxdepth, --max-depth, -d), or name the directory. In auto mode a command the gate cannot prove runs contained where a sandbox exists (no network, no socket bind, writes only under cwd and tmp); a PermissionDenied there says nothing about the code."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "Shell command to run. Omit it to check on a background job instead."},
                "job": {"type": "integer", "description": "Background job to check on; defaults to the most recent"},
                "wait": {"type": "integer", "description": "Seconds to wait for that job, clamped to 5-300"},
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
            return poll_job(&input);
        }
        if let Some(refusal) = broad_search(command) {
            return error_output(refusal);
        }
        let requested_timeout = input.get("timeout_secs").and_then(Value::as_u64);
        let timeout = crate::jobs::clamp_timeout(requested_timeout);
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
            &context.cwd,
            &context.cancelled,
            context.auto_background,
            timeout,
            context.sandbox.as_ref().filter(|_| placed.is_none()),
            placed.as_ref().map(|(_, sweep)| sweep.as_str()),
        ) {
            Ok(crate::jobs::Run::Finished(capture)) => (*capture, false),
            Ok(crate::jobs::Run::TimedOut(capture)) => (*capture, true),
            Ok(crate::jobs::Run::Backgrounded(id)) => {
                sections.push(format!(
                    "Backgrounded as job {id}. Call bash with no command (optionally job={id}) to check on it."
                ));
                let mut output = text_output(sections.join("\n"));
                output.result.details = json!({ "job": id.0, "backgrounded": true });
                return output;
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
            context.recovery_dir.as_deref(),
            max_lines,
        );
        if !reduced.text.is_empty() {
            sections.push(reduced.text.clone());
        }
        if capture.truncated {
            sections.push("[output truncated]".to_owned());
        }
        if timed_out && nudge.is_some() {
            sections.push(format!("[timed out after {}s]", timeout.as_secs()));
        } else if timed_out {
            sections.push(format!(
                "[timed out after {}s; pass timeout_secs up to {} for a longer run, or narrow the command]",
                timeout.as_secs(),
                crate::jobs::MAX_TIMEOUT_SECS
            ));
        } else if capture.cancelled {
            sections.push("[command aborted]".to_owned());
        }
        if let Some(error) = &capture.kill_error {
            sections.push(format!("[group kill failed: {error}]"));
        }
        let exit_code = capture.exit_code.unwrap_or(-1);
        if exit_code != 0 {
            sections.push(format!("exit code: {exit_code}"));
            if command.contains("&&") {
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
        if context.sandbox.is_some()
            && let Some(hint) = crate::sandbox::denial_hint(
                capture.exit_code,
                &format!("{}{}", capture.stdout, capture.stderr),
                command,
            )
        {
            sections.push(hint);
        }
        let mut text = if sections.is_empty() {
            "(no output)".to_owned()
        } else {
            sections.join("\n")
        };
        let bridge = match &self.hashline {
            Some(state) if exit_code == 0 && !capture.cancelled => {
                match bridge_target(command, &context.cwd) {
                    Bridge::Tag { typed, path } => match fs::read_to_string(&path) {
                        Ok(content) => {
                            let seen = viewed_lines(command, &content, &capture.stdout);
                            let tag = crate::hashline::tool::record_view_snapshot(
                                state, &path, &content, &seen,
                            );
                            let header =
                                crate::hashline::format::format_hashline_header(&typed, tag);
                            text = format!("{header}\n{text}");
                            "tag"
                        }
                        Err(_) => SkipReason::NotOneFile.name(),
                    },
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
        });
        output.is_error = exit_code != 0 || capture.cancelled;
        output
    }
}

/// The same tool with no command, never a second tool to discover.
fn poll_job(input: &Map<String, Value>) -> ToolOutput {
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
    loop {
        let report = match requested {
            Some(id) => crate::jobs::registry().report(id),
            None => crate::jobs::registry().latest(),
        };
        let Some(report) = report else {
            return error_output("no background job to check on".to_owned());
        };
        if report.finished {
            let exit_code = report.exit_code.unwrap_or(-1);
            let mut output = text_output(format!(
                "job {} finished (exit {exit_code}): {}\n{}",
                report.id, report.command, report.output
            ));
            output.result.details = json!({ "job": report.id.0, "exitCode": exit_code });
            output.is_error = exit_code != 0;
            return output;
        }
        match deadline {
            Some(limit) if started.elapsed() < limit => {
                crate::jobs::registry()
                    .wait_settled(requested, limit.saturating_sub(started.elapsed()));
            }
            _ => {
                let mut output = text_output(format!(
                    "job {} still running: {}",
                    report.id, report.command
                ));
                output.result.details = json!({ "job": report.id.0, "running": true });
                return output;
            }
        }
    }
}

/// Gitignore-filtered and sorted, for surfaces offering a file picker.
pub fn list_files(root: &Path, cap: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    walk_files(root, &mut |path| {
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
