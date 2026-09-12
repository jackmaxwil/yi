use std::path::{Component, Path, PathBuf};

/// Credential stores, protected recursively: destroying one private key inside
/// ~/.ssh is as damaging as destroying the directory.
const PROTECTED_CREDENTIAL_SUBPATHS: [&str; 5] = [".ssh", ".gnupg", ".aws", ".kube", ".docker"];

/// Home directories whose wholesale destruction is unacceptable but whose
/// individual files are legitimately edited; matched exactly.
const PROTECTED_HOME_SUBPATHS: [&str; 7] = [
    ".config",
    ".yi",
    ".claude",
    ".local",
    ".local/share",
    "Documents",
    "Desktop",
];

const PROTECTED_SYSTEM_PATHS: [&str; 20] = [
    "/",
    "/bin",
    "/boot",
    "/dev",
    "/etc",
    "/lib",
    "/lib64",
    "/opt",
    "/proc",
    "/root",
    "/sbin",
    "/srv",
    "/sys",
    "/usr",
    "/var",
    "/Applications",
    "/System",
    "/Library",
    "/Users",
    "/home",
];

/// System paths whose contents are as critical as the directory itself.
const SYSTEM_PATHS_PROTECTED_RECURSIVELY: [&str; 12] = [
    "/bin", "/boot", "/dev", "/etc", "/lib", "/lib64", "/proc", "/sbin", "/sys", "/usr",
    "/var/lib", "/System",
];

pub struct CatastrophicContext {
    pub home_dir: Option<PathBuf>,
    pub working_dir: Option<PathBuf>,
    /// The workspace `.git` directory (design M10 addition): denied in every
    /// mode including yolo — losing it loses the undo story for everything.
    pub workspace_git: Option<PathBuf>,
}

impl CatastrophicContext {
    pub fn detect(cwd: &Path) -> Self {
        Self {
            home_dir: home_dir(),
            working_dir: Some(cwd.to_path_buf()),
            workspace_git: Some(cwd.join(".git")),
        }
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn expand_home(path: &Path, home: Option<&Path>) -> PathBuf {
    match (home, path.strip_prefix("~")) {
        (Some(home), Ok(rest)) if rest.as_os_str().is_empty() => home.to_path_buf(),
        (Some(home), Ok(rest)) => home.join(rest),
        _ => path.to_path_buf(),
    }
}

/// `~` is HOME as a shell reads it, and the HOME `detect` reads: a tool opening
/// `<cwd>/~/notes.txt` would open a file the permission check never judged.
pub fn resolve_target(raw: &str, cwd: &Path) -> PathBuf {
    cwd.join(expand_home(Path::new(raw), home_dir().as_deref()))
}

/// Never touches the filesystem: canonicalize() fails on a file being created and a hostile
/// argument cannot slow a lexical pass. The write-time symlink recheck (T10) compensates.
pub fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        return PathBuf::from("/");
    }
    out
}

fn expand(raw: &str, context: &CatastrophicContext) -> PathBuf {
    let mut text = raw.to_owned();
    if let Some(home) = &context.home_dir {
        let home_str = home.to_string_lossy().into_owned();
        for var in ["${HOME}", "$HOME"] {
            text = text.replace(var, &home_str);
        }
    }
    resolve(Path::new(&text), context)
}

pub(crate) fn resolve(path: &Path, context: &CatastrophicContext) -> PathBuf {
    let path = expand_home(path, context.home_dir.as_deref());
    if path.is_absolute() {
        return lexical_normalize(&path);
    }
    match &context.working_dir {
        Some(cwd) => lexical_normalize(&cwd.join(path)),
        None => path,
    }
}

/// Whether destroying this path is categorically unacceptable. Checked before
/// decide() and denied in every mode including yolo (design M10, D15).
pub fn is_catastrophic(path: &Path, context: &CatastrophicContext) -> bool {
    let path = lexical_normalize(path);
    if PROTECTED_SYSTEM_PATHS
        .iter()
        .any(|protected| path == Path::new(protected))
    {
        return true;
    }
    if SYSTEM_PATHS_PROTECTED_RECURSIVELY
        .iter()
        .any(|protected| path.starts_with(protected))
    {
        return true;
    }
    if let Some(git) = &context.workspace_git {
        let git = lexical_normalize(git);
        if path == git || path.starts_with(&git) {
            return true;
        }
    }
    let Some(home) = &context.home_dir else {
        return false;
    };
    let home = lexical_normalize(home);
    if path == home {
        return true;
    }
    if PROTECTED_CREDENTIAL_SUBPATHS
        .iter()
        .any(|sub| path.starts_with(home.join(sub)))
    {
        return true;
    }
    PROTECTED_HOME_SUBPATHS
        .iter()
        .any(|sub| path == home.join(sub))
}

fn credential_stores(context: &CatastrophicContext) -> Vec<PathBuf> {
    context
        .home_dir
        .iter()
        .flat_map(|home| {
            PROTECTED_CREDENTIAL_SUBPATHS
                .iter()
                .map(|sub| lexical_normalize(&home.join(sub)))
        })
        .collect()
}

/// Directories that hold users' key stores whatever HOME is, refused to a read's walk.
const HOME_ROOTS: [&str; 4] = ["/", "/home", "/Users", "/root"];

/// `/proc` entries that link elsewhere (`/proc/self/root` is `/`), unseen by a lexical check.
const PROC_LINKS: [&str; 4] = ["root", "cwd", "fd", "map_files"];

/// What a read may not touch (D180): a key or the workspace `.git`, a directory a walk would
/// carry into a key store, and a device, which never ends (`/dev/zero`) or waits (`/dev/tty`).
pub(crate) fn read_is_catastrophic(path: &Path, context: &CatastrophicContext) -> bool {
    let path = lexical_normalize(path);
    if path.starts_with("/dev") || HOME_ROOTS.iter().any(|root| path == Path::new(root)) {
        return true;
    }
    if let Ok(rest) = path.strip_prefix("/proc")
        && rest
            .components()
            .any(|part| PROC_LINKS.iter().any(|link| part.as_os_str() == *link))
    {
        return true;
    }
    if let Some(git) = &context.workspace_git
        && path.starts_with(lexical_normalize(git))
    {
        return true;
    }
    credential_stores(context)
        .iter()
        .any(|store| path.starts_with(store) || store.starts_with(&path))
}

const DESTRUCTIVE_COMMANDS: [&str; 4] = ["rm", "rmdir", "shred", "unlink"];

/// A key read into the transcript has already left the machine, so credential stores are
/// read-gated too. Path-shaped arguments only: this reads a command, it does not run one.
pub fn command_reads_credentials(command: &str, context: &CatastrophicContext) -> Option<String> {
    let stores = credential_stores(context);
    command
        .split_whitespace()
        .skip(1)
        .filter(|token| !token.starts_with('-'))
        .map(|token| expand(token, context))
        .find(|path| stores.iter().any(|store| path.starts_with(store)))
        .map(|path| path.to_string_lossy().into_owned())
}

/// A destructive verb sends every path-shaped token through the denylist. No
/// shell-parsing cleverness (D15): this is a belt over the path-based checks.
pub fn command_targets_catastrophic(
    command: &str,
    context: &CatastrophicContext,
) -> Option<String> {
    let mut tokens = command.split_whitespace();
    let verb = tokens.next()?;
    let verb = verb.rsplit('/').next().unwrap_or(verb);
    let destructive = DESTRUCTIVE_COMMANDS.contains(&verb)
        || (verb == "sudo"
            && command
                .split_whitespace()
                .nth(1)
                .is_some_and(|second| DESTRUCTIVE_COMMANDS.contains(&second)));
    if !destructive {
        return None;
    }
    for token in command.split_whitespace().skip(1) {
        if token.starts_with('-') {
            continue;
        }
        // Incident: `rm -rf /` trimmed to an empty path that expanded to the working dir and
        // `rm -rf .git` was skipped for having no slash. Every non-flag argument counts now.
        let trimmed = token.trim_end_matches(['*', '/']);
        let bare = match (trimmed.is_empty(), token.starts_with('/')) {
            (false, _) => trimmed,
            (true, true) => "/",
            (true, false) => ".",
        };
        let expanded = expand(bare, context);
        if is_catastrophic(&expanded, context) {
            return Some(expanded.to_string_lossy().into_owned());
        }
    }
    None
}
