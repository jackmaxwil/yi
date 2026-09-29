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
    /// The workspace `.git` directory (design §8 addition): denied in every
    /// mode including yolo — losing it loses the undo story for everything.
    pub workspace_git: Vec<PathBuf>,
}

impl CatastrophicContext {
    pub fn detect(cwd: &Path) -> Self {
        Self {
            home_dir: home_dir(),
            working_dir: Some(cwd.to_path_buf()),
            workspace_git: workspace_git(cwd),
        }
    }
}

/// A linked worktree's real git dirs sit outside it; `rm` may name them as typed or resolved.
fn workspace_git(cwd: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![cwd.join(".git")];
    for dir in git_dirs(cwd) {
        if let Ok(resolved) = dir.canonicalize() {
            dirs.push(resolved);
        }
        dirs.push(dir);
    }
    dirs.dedup();
    dirs
}

const POINTER_MAX_BYTES: u64 = 4096;

fn read_pointer(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(POINTER_MAX_BYTES).read_to_string(&mut text))
        .ok()?;
    Some(text.trim().to_owned())
}

/// `[git dir]`, or `[worktree git dir, common dir]` for a linked worktree; file reads only.
pub fn git_dirs(cwd: &Path) -> Vec<PathBuf> {
    let Some(dot_git) = cwd
        .ancestors()
        .map(|dir| dir.join(".git"))
        .find(|git| git.exists())
    else {
        return Vec::new();
    };
    if dot_git.is_dir() {
        return vec![lexical_normalize(&dot_git)];
    }
    let Some(target) = read_pointer(&dot_git)
        .as_deref()
        .and_then(|text| text.strip_prefix("gitdir: "))
        .map(PathBuf::from)
    else {
        return Vec::new();
    };
    let base = dot_git.parent().unwrap_or(cwd);
    let Some(gitdir) = git_dir_at(&base.join(target)) else {
        return Vec::new();
    };
    let common = read_pointer(&gitdir.join("commondir"))
        .and_then(|common| git_dir_at(&gitdir.join(common)))
        .unwrap_or_else(|| gitdir.clone());
    let mut dirs = vec![gitdir];
    if !dirs.contains(&common) {
        dirs.push(common);
    }
    dirs
}

/// Checked, never believed: without this `gitdir: /` reads as a writable root (D205).
fn git_dir_at(path: &Path) -> Option<PathBuf> {
    let path = lexical_normalize(path);
    path.join("HEAD").is_file().then_some(path)
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
/// argument cannot slow a lexical pass. The write-time symlink recheck (§7.2) compensates.
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
/// decide() and denied in every mode including yolo (design §8, D15).
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
    if context
        .workspace_git
        .iter()
        .any(|git| path.starts_with(lexical_normalize(git)))
    {
        return true;
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
pub fn read_is_catastrophic(path: &Path, context: &CatastrophicContext) -> bool {
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
    if context
        .workspace_git
        .iter()
        .any(|git| path.starts_with(lexical_normalize(git)))
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

/// Wrappers that run another command: argv0 alone lets `nice rm -rf .git` past the belt (D205).
const WRAPPERS: [&str; 11] = [
    "command", "doas", "env", "ionice", "nice", "nohup", "stdbuf", "sudo", "time", "timeout",
    "xargs",
];

/// Reading past a wrapper, flag, assignment or number only widens what the belt and wall see.
pub fn wraps(word: &str) -> bool {
    WRAPPERS.contains(&word.rsplit('/').next().unwrap_or(word))
        || word.starts_with('-')
        || word.contains('=')
        || word.chars().all(|character| character.is_ascii_digit())
}

fn runs_destructive(command: &str) -> bool {
    command
        .split_whitespace()
        .find(|token| !wraps(token))
        .is_some_and(|token| {
            DESTRUCTIVE_COMMANDS.contains(&token.rsplit('/').next().unwrap_or(token))
        })
}

/// A destructive verb sends every path-shaped token through the denylist. No
/// shell-parsing cleverness (D15): this is a belt over the path-based checks.
pub fn command_targets_catastrophic(
    command: &str,
    context: &CatastrophicContext,
) -> Option<String> {
    if !runs_destructive(command) {
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
