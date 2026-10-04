use rustix::fs::{AtFlags, unlinkat};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Component, Path, PathBuf};

/// Credential stores, protected recursively: destroying one private key inside
/// ~/.ssh is as damaging as destroying the directory. yi's own provider logins, OAuth profiles
/// and MCP OAuth tokens are keys too (#887), and so are the CLI logins egress would carry (#598),
/// and a kernel's connection file, whose key runs code in that kernel over loopback (#599).
const PROTECTED_CREDENTIAL_SUBPATHS: [&str; 19] = [
    ".ssh",
    ".gnupg",
    ".aws",
    ".kube",
    ".docker",
    ".yi/mcp/tokens",
    ".yi/providers/tokens",
    ".yi/oauth",
    ".yi/kernel-connections",
    ".config/gh",
    ".config/fgj",
    ".config/gcloud",
    ".netrc",
    ".git-credentials",
    ".npmrc",
    ".pypirc",
    ".cargo/credentials",
    ".cargo/credentials.toml",
    ".password-store",
];

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

#[derive(Clone)]
pub struct CatastrophicContext {
    pub home_dir: Option<PathBuf>,
    pub working_dir: Option<PathBuf>,
    /// The workspace `.git` directory (design §8 addition): denied in every
    /// mode including yolo — losing it loses the undo story for everything.
    pub workspace_git: Vec<PathBuf>,
    /// Stores only the host writes: the session corpus, whose JSONL kept rules replay from.
    pub host_owned: Vec<PathBuf>,
}

impl CatastrophicContext {
    pub fn detect(cwd: &Path) -> Self {
        Self {
            home_dir: home_dir(),
            working_dir: Some(cwd.to_path_buf()),
            workspace_git: workspace_git(cwd),
            host_owned: home_dir()
                .map(|home| home.join(".yi/sessions"))
                .into_iter()
                .collect(),
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
    resolve(Path::new(&with_home(raw, context)), context)
}

fn with_home(raw: &str, context: &CatastrophicContext) -> String {
    let mut text = raw.to_owned();
    if let Some(home) = &context.home_dir {
        let home_str = home.to_string_lossy().into_owned();
        for var in ["${HOME}", "$HOME"] {
            text = text.replace(var, &home_str);
        }
    }
    text
}

pub(crate) fn resolve(path: &Path, context: &CatastrophicContext) -> PathBuf {
    let path = absolute(path, context);
    match path.is_absolute() {
        true => lexical_normalize(&path),
        false => path,
    }
}

/// `~` and the working dir applied, `..` left to the kernel, which takes it after the link
/// before it as a tool's open does: `link/../x` names the link target's sibling.
pub(crate) fn absolute(path: &Path, context: &CatastrophicContext) -> PathBuf {
    let path = expand_home(path, context.home_dir.as_deref());
    match (&context.working_dir, path.is_absolute()) {
        (Some(cwd), false) => cwd.join(path),
        _ => path,
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
    if (context.workspace_git.iter())
        .chain(&context.host_owned)
        .any(|guarded| path.starts_with(lexical_normalize(guarded)))
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

/// The credential stores under `home`, the one list the destroy gate, the read gate, the bash
/// read belt and the sandbox's deny-read all take (D323).
pub fn credential_stores(home: &Path) -> Vec<PathBuf> {
    PROTECTED_CREDENTIAL_SUBPATHS
        .iter()
        .map(|sub| lexical_normalize(&home.join(sub)))
        .collect()
}

fn stores(context: &CatastrophicContext) -> Vec<PathBuf> {
    (context.home_dir.as_deref())
        .map(credential_stores)
        .unwrap_or_default()
}

/// Directories that hold users' key stores whatever HOME is, refused to a read's walk.
const HOME_ROOTS: [&str; 4] = ["/", "/home", "/Users", "/root"];

/// `/proc` entries that link elsewhere (`/proc/self/root` is `/`), unseen by a lexical check.
const PROC_LINKS: [&str; 4] = ["root", "cwd", "fd", "map_files"];

/// What a read may not touch (D180, D323): a key, the workspace `.git` or a device, which never
/// ends (`/dev/zero`) or waits (`/dev/tty`), and a directory a walk would carry into a key
/// store; judged by file identity too, so letter case, a link, `/private` or a firmlink names no
/// way in; [`ReadGate::open`] judges the file it opened, so no swapped link either (#890). Built
/// once per call or walk: each guarded directory costs a stat.
pub struct ReadGate {
    context: CatastrophicContext,
    stores: Vec<PathBuf>,
    /// Guarded with all under them: the key stores, `/dev` and the workspace `.git`.
    trees: Vec<PathBuf>,
    tree_ids: Vec<FileId>,
    /// The trees, the roots other users' homes sit under, and every directory above a store,
    /// each guarded as itself.
    ids: Vec<FileId>,
    /// Paths no wall covers, with all under them: a session's own spills and transcript under
    /// the roots its wall holds (D340, #971).
    spared: Vec<PathBuf>,
}

impl ReadGate {
    pub fn new(context: &CatastrophicContext) -> Self {
        let stores = stores(context);
        let mut trees = stores.clone();
        trees.push(PathBuf::from("/dev"));
        trees.extend(context.workspace_git.iter().cloned());
        let above = stores.iter().flat_map(|store| store.ancestors().skip(1));
        let exact: Vec<PathBuf> = (HOME_ROOTS.iter().map(Path::new))
            .chain(above)
            .map(Path::to_path_buf)
            .collect();
        let tree_ids = identities(&trees);
        let ids = [tree_ids.clone(), identities(&exact)].concat();
        Self {
            context: context.clone(),
            stores,
            trees,
            tree_ids,
            ids,
            spared: Vec::new(),
        }
    }

    /// This gate with each of `paths` and all under them outside every wall it is handed.
    #[must_use]
    pub fn except(mut self, paths: Vec<PathBuf>) -> Self {
        self.spared = paths;
        self
    }

    /// A named path: refused when it or an ancestor is guarded by spelling or by identity, a
    /// missing tail judged by its existing head and a dangling link by where it points.
    pub fn denies(&self, path: &Path) -> bool {
        self.denies_spelling(path)
            || beneath_via(&self.trees, &self.tree_ids, path, LINK_HOPS)
            || denied_file(&self.ids, fs::metadata(path).ok().as_ref())
    }

    /// A walk's entry, whose ancestors the walk judged on its way in: the identity `meta` read
    /// without following it. Its spelling adds nothing, since every guarded directory has one.
    pub fn denies_entry(&self, meta: Option<&fs::Metadata>) -> bool {
        denied_file(&self.ids, meta)
    }

    /// Opens the resolved `path`, judged with `walls`, with no link left to follow: the file judged
    /// is the file opened, so a link swapped in after an earlier check reaches nothing (#890).
    pub fn open(&self, path: &Path, walls: &[PathBuf]) -> io::Result<File> {
        let real = fs::canonicalize(path)?;
        self.opened(&real, walls, false, OpenOptions::new().read(true))
    }

    /// [`Self::open`] to write, under the destroy gate too: untruncated, or created only where
    /// nothing stands, not even a dangling link; the caller truncates through the handle.
    pub fn open_write(&self, path: &Path, walls: &[PathBuf]) -> io::Result<File> {
        let real = match fs::canonicalize(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
                    return Err(error);
                };
                fs::canonicalize(parent)?.join(name)
            }
            resolved => resolved?,
        };
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        match self.opened(&real, walls, true, &options) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.opened(&real, walls, true, options.create_new(true))
            }
            opened => opened,
        }
    }

    /// Unlinks `path`, the link itself when it is one, once judged as a write, inside the parent
    /// held open as judged, so a parent swapped to a link afterwards deletes nothing behind it.
    pub fn remove(&self, path: &Path, walls: &[PathBuf]) -> io::Result<()> {
        self.remove_after(path, walls, || ())
    }

    fn remove_after(&self, path: &Path, walls: &[PathBuf], held: impl FnOnce()) -> io::Result<()> {
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return Err(io::ErrorKind::InvalidInput.into());
        };
        let parent = fs::canonicalize(parent)?;
        let real = parent.join(name);
        self.guard_resolved(&real, walls, true)?;
        let dir = judged_dir(&parent, &real)?;
        held();
        Ok(unlinkat(&dir, name, AtFlags::empty())?)
    }

    fn guard_resolved(&self, real: &Path, walls: &[PathBuf], writes: bool) -> io::Result<()> {
        let spared = beneath(&self.spared, real);
        let denied = self.denies(real)
            || (writes && is_catastrophic(real, &self.context))
            || (beneath(walls, real) && !spared);
        if denied { Err(refused(real)) } else { Ok(()) }
    }

    fn opened(
        &self,
        real: &Path,
        walls: &[PathBuf],
        writes: bool,
        options: &OpenOptions,
    ) -> io::Result<File> {
        self.guard_resolved(real, walls, writes)?;
        let (_dir, at) = beside(real)?;
        let file = no_link(options)?.open(&at)?;
        opened_at(&file, real)
            .then_some(file)
            .ok_or_else(|| refused(real))
    }

    fn denies_spelling(&self, path: &Path) -> bool {
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
        if (self.context.workspace_git.iter()).any(|git| path.starts_with(lexical_normalize(git))) {
            return true;
        }
        (self.stores.iter()).any(|store| path.starts_with(store) || store.starts_with(&path))
    }
}

/// [`ReadGate::denies`] for one path.
pub fn read_is_catastrophic(path: &Path, context: &CatastrophicContext) -> bool {
    ReadGate::new(context).denies(path)
}

fn refused(real: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!(
            "{} is a protected path (a key store, the workspace .git, a device or a walled path); no call opens it",
            real.display()
        ),
    )
}

/// `O_NOFOLLOW_ANY` from `<sys/fcntl.h>`: the open fails if any component of the path is a
/// link, so a resolved path opens as judged or not at all.
#[cfg(target_os = "macos")]
fn no_link(options: &OpenOptions) -> io::Result<OpenOptions> {
    use std::os::unix::fs::OpenOptionsExt;
    static HONOURED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let flagged = |options: &OpenOptions| {
        let mut options = options.clone();
        options.custom_flags(0x2000_0000);
        options
    };
    // A kernel before macOS 11 ignores the bit, so `/etc`, a link to `/private/etc`, opens
    // instead of failing with ELOOP (62); every gated open is refused then, never raced.
    let honoured = *HONOURED.get_or_init(|| {
        let probe = flagged(OpenOptions::new().read(true)).open("/etc");
        probe.err().and_then(|error| error.raw_os_error()) == Some(62)
    });
    match honoured {
        true => Ok(flagged(options)),
        false => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this macOS ignores O_NOFOLLOW_ANY (it needs macOS 11), so no tool opens a file a swapped link could redirect",
        )),
    }
}

#[cfg(not(target_os = "macos"))]
fn no_link(options: &OpenOptions) -> io::Result<OpenOptions> {
    Ok(options.clone())
}

/// Linux has no such flag on `open`, so the kernel's own name for the descriptor must be the
/// path judged: a link anywhere on the way lands elsewhere and reads back a different name.
#[cfg(target_os = "linux")]
fn opened_at(file: &File, real: &Path) -> bool {
    use std::os::fd::AsRawFd;
    fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())).is_ok_and(|at| at == real)
}

#[cfg(not(target_os = "linux"))]
fn opened_at(_file: &File, _real: &Path) -> bool {
    true
}

/// `real`'s parent opened with no link on its path (macOS) or under the very name judged (Linux).
fn judged_dir(parent: &Path, real: &Path) -> io::Result<File> {
    let dir = no_link(OpenOptions::new().read(true))?.open(parent)?;
    opened_at(&dir, parent)
        .then_some(dir)
        .ok_or_else(|| refused(real))
}

/// Where to open `real` from: on Linux through its parent held open and checked, so a create
/// lands in the directory judged (std has no `openat`).
#[cfg(target_os = "linux")]
fn beside(real: &Path) -> io::Result<(Option<File>, PathBuf)> {
    use std::os::fd::AsRawFd;
    let (Some(parent), Some(name)) = (real.parent(), real.file_name()) else {
        return Ok((None, real.to_path_buf()));
    };
    let dir = judged_dir(parent, real)?;
    let at = Path::new(&format!("/proc/self/fd/{}", dir.as_raw_fd())).join(name);
    Ok((Some(dir), at))
}

#[cfg(not(target_os = "linux"))]
fn beside(real: &Path) -> io::Result<(Option<File>, PathBuf)> {
    Ok((None, real.to_path_buf()))
}

/// Invariant: beneath when it or an ancestor is one of `roots` by spelling or file identity, so
/// a symlink (dangling too), `..` or letter case cannot reach around it; hard links can.
pub fn beneath(roots: &[PathBuf], path: &Path) -> bool {
    !roots.is_empty() && beneath_via(roots, &identities(roots), path, LINK_HOPS)
}

/// As many links as the kernel follows before `ELOOP`.
const LINK_HOPS: u8 = 40;

/// A missing tail is judged by its existing head, and a dangling link just past that head by
/// where it points, so a refusal never tells a file behind a guard from a missing one.
fn beneath_via(roots: &[PathBuf], ids: &[FileId], path: &Path, hops: u8) -> bool {
    if lexically_beneath(roots, path) {
        return true;
    }
    let Some((head, real)) =
        (path.ancestors()).find_map(|head| Some((head, fs::canonicalize(head).ok()?)))
    else {
        return false;
    };
    if (real.ancestors()).any(|dir| denied_file(ids, fs::metadata(dir).ok().as_ref())) {
        return true;
    }
    let mut rest = path.strip_prefix(head).unwrap_or(path).components();
    let Some(Ok(target)) = rest.next().map(|next| fs::read_link(head.join(next))) else {
        return false;
    };
    let target = real.join(target).join(rest.as_path());
    hops > 0 && beneath_via(roots, ids, &target, hops.saturating_sub(1))
}

pub fn lexically_beneath(roots: &[PathBuf], path: &Path) -> bool {
    !roots.is_empty() && {
        let normalized = lexical_normalize(path);
        (roots.iter()).any(|root| normalized.starts_with(lexical_normalize(root)))
    }
}

/// A file's device and inode, which no spelling of its path changes.
pub type FileId = (u64, u64);

pub fn identities(paths: &[PathBuf]) -> Vec<FileId> {
    paths
        .iter()
        .filter_map(|path| file_id(&fs::metadata(path).ok()?))
        .collect()
}

pub fn denied_file(ids: &[FileId], meta: Option<&fs::Metadata>) -> bool {
    !ids.is_empty() && meta.and_then(file_id).is_some_and(|id| ids.contains(&id))
}

#[cfg(unix)]
fn file_id(meta: &fs::Metadata) -> Option<FileId> {
    use std::os::unix::fs::MetadataExt;
    Some((meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
fn file_id(_meta: &fs::Metadata) -> Option<FileId> {
    None
}

const DESTRUCTIVE_COMMANDS: [&str; 4] = ["rm", "rmdir", "shred", "unlink"];

/// A key read into the transcript has already left the machine, so credential stores are
/// read-gated too. Path-shaped arguments only: this reads a command, it does not run one. A path
/// is judged as the read gate judges it, by identity, and a directory in home above a store counts,
/// since a recursive read walks into it, as do the roots homes sit under; a glob counts when it
/// can name a store, a directory above one, or a path inside one (D324). Quotes and backslashes
/// are removed as the shell removes them; a path hidden in a variable or a substitution is not.
pub fn command_reads_credentials(command: &str, context: &CatastrophicContext) -> Option<String> {
    let stores = stores(context);
    let home = context.home_dir.as_deref().map(lexical_normalize);
    let above: Vec<PathBuf> = (stores.iter())
        .flat_map(|store| store.ancestors().skip(1))
        .filter(|dir| home.as_ref().is_some_and(|home| dir.starts_with(home)))
        .chain(HOME_ROOTS.iter().map(Path::new))
        .map(Path::to_path_buf)
        .collect();
    let (store_ids, above_ids) = (identities(&stores), identities(&above));
    let real_stores: Vec<PathBuf> = stores.iter().map(|store| resolve_links(store)).collect();
    command
        .split_whitespace()
        .skip(1)
        .map(|token| token.replace(['\'', '"', '\\'], ""))
        .filter(|token| !token.starts_with('-'))
        .map(|token| {
            let path = expand(&token, context);
            (token, path)
        })
        .find(|(token, path)| match token.contains(['*', '?', '[', '{']) {
            true => [path.clone(), resolve_links(path)].iter().any(|pattern| {
                (stores.iter().chain(&real_stores)).any(|store| glob_reaches(pattern, store))
            }),
            false => {
                // `link/../.netrc` is opened with `..` taken after the link, not popped before it.
                let opened =
                    resolve_links(&absolute(Path::new(&with_home(token, context)), context));
                beneath_via(&stores, &store_ids, path, LINK_HOPS)
                    || beneath_via(&stores, &store_ids, &opened, LINK_HOPS)
                    || above.contains(path)
                    || denied_file(&above_ids, fs::metadata(path).ok().as_ref())
            }
        })
        .map(|(_, path)| path.to_string_lossy().into_owned())
}

/// A path as the kernel resolves an open: every link and `..` taken, a missing tail as spelled.
pub fn resolve_links(path: &Path) -> PathBuf {
    let Some((head, real)) =
        (path.ancestors()).find_map(|head| Some((head, fs::canonicalize(head).ok()?)))
    else {
        return path.to_path_buf();
    };
    match path.strip_prefix(head) {
        Ok(tail) if !tail.as_os_str().is_empty() => lexical_normalize(&real.join(tail)),
        _ => real,
    }
}

/// Whether a shell glob can match `store`, a directory above it, or a path inside it: compared
/// component by component over the shorter of the two, ignoring letter case, a leading dot
/// matched only by what can open with a dot, as the shell matches it. A resolved pattern keeps its glob text.
fn glob_reaches(pattern: &Path, store: &Path) -> bool {
    let words = |path: &Path| -> Vec<Vec<char>> {
        (path.components())
            .map(|part| {
                part.as_os_str()
                    .to_string_lossy()
                    .to_lowercase()
                    .chars()
                    .collect()
            })
            .collect()
    };
    (words(pattern).iter().zip(&words(store))).all(|(glob, name)| {
        // A `{…}` group or a `[…]` class may open with the dot a plain glob never matches.
        let dotted = matches!(glob.first(), Some('.' | '{' | '['));
        let hidden = name.first() == Some(&'.') && !dotted;
        !hidden && wildcard(glob, name)
    })
}

/// `*` any run, `?` and a `[…]` class one character, a `{…}` group any run: a glob that can
/// match more than the shell would only asks more.
fn wildcard(glob: &[char], name: &[char]) -> bool {
    let rest = |after: usize| glob.get(after..).unwrap_or_default();
    let any_run = |after: usize| {
        (0..=name.len()).any(|at| wildcard(rest(after), name.get(at..).unwrap_or_default()))
    };
    match glob.first() {
        None => name.is_empty(),
        Some('*') => any_run(1),
        Some('{') => glob
            .iter()
            .position(|c| *c == '}')
            .is_some_and(|end| any_run(end + 1)),
        Some(first) => {
            let width = match first {
                '?' => 1,
                '[' => glob.iter().position(|c| *c == ']').map_or(1, |end| end + 1),
                _ => 1,
            };
            let same = matches!(first, '?' | '[') || name.first() == Some(first);
            same && !name.is_empty() && wildcard(rest(width), name.get(1..).unwrap_or_default())
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The swap lands between the judgement and the unlink every time, where the race found in
    /// review lands once in 100,000 tries: the delete must stay in the directory judged.
    #[test]
    fn a_parent_swapped_after_the_judgement_deletes_in_the_judged_dir() -> io::Result<()> {
        let root = std::env::temp_dir().join(format!("yi-gate-unlink-{}", std::process::id()));
        let (store, workspace) = (root.join("home/.ssh"), root.join("workspace"));
        let (d, stash) = (workspace.join("d"), workspace.join("dir.tmp"));
        fs::create_dir_all(&store)?;
        fs::create_dir_all(&d)?;
        fs::write(store.join("id_rsa"), "FAKE KEY MARKER\n")?;
        fs::write(d.join("id_rsa"), "ordinary\n")?;
        let gate = ReadGate::new(&CatastrophicContext {
            home_dir: Some(root.join("home")),
            working_dir: Some(workspace.clone()),
            workspace_git: Vec::new(),
            host_owned: Vec::new(),
        });
        let mut swapped = Ok(());
        let removed = gate.remove_after(&d.join("id_rsa"), &[], || {
            swapped = fs::rename(&d, &stash).and_then(|()| std::os::unix::fs::symlink(&store, &d));
        });
        let (key, ordinary) = (store.join("id_rsa").exists(), stash.join("id_rsa").exists());
        fs::remove_dir_all(&root)?;
        swapped?;
        assert!(removed.is_ok(), "{removed:?}");
        assert!(key, "the remove deleted the key behind the swapped parent");
        assert!(
            !ordinary,
            "the remove left the file in the directory it judged"
        );
        Ok(())
    }
}
