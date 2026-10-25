use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use super::Event;

/// The project-scope files `yi trust` grants; they run only while their granted hash holds.
pub const PROJECT_FILES: [&str; 2] = [".claude/settings.json", ".claude/settings.local.json"];

pub const MANAGED_DIR: &str = if cfg!(target_os = "macos") {
    "/Library/Application Support/ClaudeCode"
} else {
    "/etc/claude-code"
};

const DEFAULT_TIMEOUT_SECS: u64 = 600;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Managed,
    User,
    Project,
    Local,
    Plugin,
}

#[derive(Clone)]
pub(super) enum Matcher {
    Any,
    Names(Vec<String>),
    Regex(regex::Regex),
}

impl Matcher {
    fn parse(text: &str) -> Result<Self, String> {
        if text.is_empty() || text == "*" {
            return Ok(Self::Any);
        }
        let plain =
            (text.chars()).all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | ' ' | ',' | '|'));
        if plain {
            let names = text.split([',', '|']).map(str::trim);
            let names = names.filter(|name| !name.is_empty()).map(str::to_owned);
            return Ok(Self::Names(names.collect()));
        }
        regex::Regex::new(text)
            .map(Self::Regex)
            .map_err(|error| error.to_string())
    }

    pub(super) fn matches(&self, name: &str) -> bool {
        match self {
            Self::Any => true,
            Self::Names(names) => names.iter().any(|candidate| candidate == name),
            Self::Regex(regex) => regex.is_match(name),
        }
    }
}

/// An `if` rule, `Tool` or `Tool(glob)`; the glob is matched as Yi's own permission rules match.
pub(super) struct Condition {
    tool: String,
    pattern: Option<yi_permission::PathGlob>,
}

impl Condition {
    fn parse(text: &str) -> Result<Self, String> {
        let (tool, pattern) = match text.trim().split_once('(') {
            Some((tool, rest)) => {
                let pattern = rest
                    .strip_suffix(')')
                    .ok_or("its rule has no closing `)`")?;
                (tool, Some(yi_permission::PathGlob::new(pattern)?))
            }
            None => (text.trim(), None),
        };
        let tool = tool.to_owned();
        Ok(Self { tool, pattern })
    }

    /// Invariant: an input the glob cannot be judged on runs the hook, as Claude Code does.
    pub(super) fn admits(&self, name: &str, input: &Value, cwd: &Path) -> bool {
        let mcp_server = name
            .strip_prefix(&self.tool)
            .is_some_and(|rest| rest.starts_with("__"));
        if self.tool != name && !(self.tool.starts_with("mcp__") && mcp_server) {
            return false;
        }
        let Some(pattern) = &self.pattern else {
            return true;
        };
        let text = |key: &str| input.get(key).and_then(Value::as_str);
        if let Some(command) = text("command").filter(|_| name == "Bash") {
            let yi_permission::Parsed::Segments(segments) = yi_permission::parse(command) else {
                return true;
            };
            return segments.iter().any(|argv| {
                let words = argv.iter().skip_while(|word| assigns(word));
                pattern.is_match(&words.cloned().collect::<Vec<_>>().join(" "))
            });
        }
        match text("file_path").or_else(|| text("path")) {
            Some(path) => {
                let relative = Path::new(path).strip_prefix(cwd).ok();
                let shown = relative.map_or_else(|| path.into(), Path::to_string_lossy);
                pattern.is_match(&shown)
            }
            None => true,
        }
    }
}

fn assigns(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

pub(super) struct Handler {
    pub(super) event: Event,
    pub(super) matcher: Matcher,
    pub(super) condition: Option<Condition>,
    pub(super) command: String,
    pub(super) args: Option<Vec<String>>,
    pub(super) timeout: Duration,
    pub(super) block_on_failure: bool,
    pub(super) detached: bool,
    /// `asyncRewake`: of the background handlers, only it keeps its timeout.
    pub(super) rewake: bool,
    pub(super) plugin_root: Option<PathBuf>,
    managed: bool,
}

impl Handler {
    /// One handler defined in several settings files runs once; a plugin's copy stays separate.
    pub(super) fn key(&self) -> (Option<&Path>, &str, Option<&[String]>) {
        (
            self.plugin_root.as_deref(),
            &self.command,
            self.args.as_deref(),
        )
    }
}

pub struct Hooks {
    pub(super) handlers: Vec<Handler>,
    pub(super) notices: Vec<String>,
    pub(super) project: PathBuf,
}

struct Source {
    scope: Scope,
    path: PathBuf,
    plugin_root: Option<PathBuf>,
}

fn sources(project: &Path, home: Option<&Path>, managed: &Path) -> Vec<Source> {
    let source = |scope, path: PathBuf, plugin_root| Source {
        scope,
        path,
        plugin_root,
    };
    let mut drop_ins: Vec<PathBuf> = std::fs::read_dir(managed.join("managed-settings.d"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .filter(|path| {
            !path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('.'))
        })
        .collect();
    drop_ins.sort();
    let mut found = vec![source(
        Scope::Managed,
        managed.join("managed-settings.json"),
        None,
    )];
    found.extend(
        drop_ins
            .into_iter()
            .map(|path| source(Scope::Managed, path, None)),
    );
    found.extend(home.map(|home| source(Scope::User, home.join(".claude/settings.json"), None)));
    let [shared, local] = PROJECT_FILES;
    found.push(source(Scope::Project, project.join(shared), None));
    found.push(source(Scope::Local, project.join(local), None));
    let listed = std::env::var_os("YI_CLAUDE_HOOKS_FILES").unwrap_or_default();
    for path in std::env::split_paths(&listed).filter(|path| !path.as_os_str().is_empty()) {
        // The Claude Code plugin lists each enabled plugin's install root.
        if path.is_dir() {
            found.push(source(
                Scope::Plugin,
                path.join("hooks/hooks.json"),
                Some(path),
            ));
            continue;
        }
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let root = match dir.file_name().is_some_and(|name| name == "hooks") {
            true => dir.parent().map(Path::to_path_buf).unwrap_or(dir),
            false => dir,
        };
        found.push(source(Scope::Plugin, path, Some(root)));
    }
    found
}

impl Hooks {
    pub fn load(origin: &Path) -> Option<Arc<Self>> {
        if std::env::var("YI_CLAUDE_HOOKS").ok().as_deref() != Some("1") {
            return None;
        }
        let home = std::env::var_os("HOME").map(PathBuf::from);
        Some(Self::load_from(
            origin,
            home.as_deref(),
            Path::new(MANAGED_DIR),
        ))
    }

    /// The project is the git root of an inherited `CLAUDE_PROJECT_DIR`, else of `origin`, the
    /// root session's directory: a worktree agent and a lane child read the session's project.
    pub fn load_from(origin: &Path, home: Option<&Path>, managed: &Path) -> Arc<Self> {
        let inherited = std::env::var_os("CLAUDE_PROJECT_DIR").map(PathBuf::from);
        let origin = inherited
            .as_deref()
            .filter(|dir| dir.is_dir())
            .unwrap_or(origin);
        let project = yi_tools::git_root(origin).unwrap_or_else(|| origin.to_path_buf());
        let gate = home.map(crate::ext::TrustGate::new);
        let mut hooks = Self {
            handlers: Vec::new(),
            notices: Vec::new(),
            project,
        };
        let (mut managed_flag, mut managed_only) = (None, false);
        let mut disabled: [Option<bool>; 3] = [None; 3];
        for source in sources(&hooks.project, home, managed) {
            let Some(value) = hooks.read(&source, gate.as_ref()) else {
                continue;
            };
            let flag = value.get("disableAllHooks").and_then(Value::as_bool);
            match source.scope {
                Scope::Managed => {
                    managed_flag = flag.or(managed_flag);
                    managed_only |= value.get("allowManagedHooksOnly") == Some(&Value::Bool(true));
                }
                Scope::User => disabled[2] = flag.or(disabled[2]),
                Scope::Project => disabled[1] = flag.or(disabled[1]),
                Scope::Local => disabled[0] = flag.or(disabled[0]),
                Scope::Plugin => {}
            }
            hooks.parse(&source, &value);
        }
        // Precedence is managed, local, project, user; only a managed flag reaches managed hooks.
        let lower = disabled.into_iter().flatten().next();
        if managed_flag == Some(true) {
            hooks.handlers.clear();
        } else if managed_only || managed_flag.is_none() && lower == Some(true) {
            hooks.handlers.retain(|handler| handler.managed);
        }
        Arc::new(hooks)
    }

    pub fn notices(&self) -> &[String] {
        &self.notices
    }

    pub fn protected(&self) -> PathBuf {
        self.project.join(".claude")
    }

    fn read(&mut self, source: &Source, gate: Option<&crate::ext::TrustGate>) -> Option<Value> {
        let shown = source.path.display();
        let text = match std::fs::read_to_string(&source.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
            Err(error) => {
                (self.notices).push(format!("[Claude Code hooks in {shown} not read: {error}]"));
                return None;
            }
        };
        if matches!(source.scope, Scope::Project | Scope::Local) && carries_hooks(&text) {
            let name = source
                .path
                .strip_prefix(&self.project)
                .unwrap_or(&source.path);
            let name = name.to_string_lossy();
            let trust = gate.map(|gate| gate.trust_of(&self.project, &name, &text));
            if trust != Some(crate::ext::Trust::Granted) {
                self.notices.push(format!(
                    "[Claude Code hooks in {shown} are not run: the file is not trusted as written; run `yi trust` in {} to grant it]",
                    self.project.display()
                ));
                return None;
            }
        }
        match serde_json::from_str(&text) {
            Ok(value) => Some(value),
            Err(error) => {
                (self.notices).push(format!(
                    "[Claude Code hooks in {shown} are invalid: {error}]"
                ));
                None
            }
        }
    }

    fn parse(&mut self, source: &Source, value: &Value) {
        let shown = source.path.display();
        let Some(events) = value.get("hooks").and_then(Value::as_object) else {
            return;
        };
        for (name, groups) in events {
            let Some(event) = Event::parse(name) else {
                continue;
            };
            for group in groups.as_array().into_iter().flatten() {
                let matcher = group.get("matcher").and_then(Value::as_str).unwrap_or("");
                let matcher = match Matcher::parse(matcher) {
                    Ok(matcher) => matcher,
                    Err(error) => {
                        self.notices.push(format!(
                            "[Claude Code {name} hooks in {shown} are not run: matcher {matcher:?} is invalid: {error}]"
                        ));
                        continue;
                    }
                };
                for handler in group
                    .get("hooks")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    match handler_of(event, source, handler) {
                        Ok(handler) => self.handlers.push(Handler {
                            matcher: matcher.clone(),
                            ..handler
                        }),
                        Err(why) => self.notices.push(format!(
                            "[Claude Code {name} hook in {shown} skipped, so it never blocks: {why}]"
                        )),
                    }
                }
            }
        }
    }
}

fn handler_of(event: Event, source: &Source, value: &Value) -> Result<Handler, String> {
    let text = |key: &str| value.get(key).and_then(Value::as_str);
    match text("type") {
        Some("command") => {}
        other => {
            return Err(format!(
                "type {} is not supported, only command",
                other.unwrap_or("(missing)")
            ));
        }
    }
    let command = text("command").ok_or("command is missing")?;
    let condition = text("if").map(Condition::parse).transpose();
    let condition = condition.map_err(|why| format!("`if` is invalid: {why}"))?;
    if text("shell") == Some("powershell") {
        return Err("shell powershell is not supported".to_owned());
    }
    let args = match value.get("args") {
        None => None,
        Some(Value::Array(args)) => Some(
            (args.iter())
                .map(|arg| arg.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .ok_or("an args element is not a string")?,
        ),
        Some(_) => return Err("args is not an array".to_owned()),
    };
    let flag = |key: &str| value.get(key) == Some(&Value::Bool(true));
    let secs = value.get("timeout").and_then(Value::as_u64);
    Ok(Handler {
        event,
        matcher: Matcher::Any,
        condition,
        command: command.to_owned(),
        args,
        timeout: Duration::from_secs(secs.unwrap_or(DEFAULT_TIMEOUT_SECS)),
        block_on_failure: text("onFailure") == Some("block"),
        detached: flag("async") || flag("asyncRewake"),
        rewake: flag("asyncRewake"),
        plugin_root: source.plugin_root.clone(),
        managed: source.scope == Scope::Managed,
    })
}

pub fn carries_hooks(text: &str) -> bool {
    serde_json::from_str::<Value>(text)
        .is_ok_and(|value| value.get("hooks").is_some() || value.get("disableAllHooks").is_some())
}
