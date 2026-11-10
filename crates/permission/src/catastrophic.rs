use rustix::fs::{AtFlags, unlinkat};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Component, Path, PathBuf};

mod credential;
pub use credential::command_reads_credentials;

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

/// Origin's `HEAD` when set, else `master` when only that exists, else `main`: from the ref files,
/// since a claim and every turn ask. ponytail: a reftable repository reads as `main`.
pub fn trunk_of(common: &Path) -> String {
    let origin_head = read_pointer(&common.join("refs/remotes/origin/HEAD"));
    if let Some(name) = origin_head
        .as_deref()
        .and_then(|text| text.strip_prefix("ref: refs/remotes/origin/"))
    {
        return name.to_owned();
    }
    let packed = fs::read_to_string(common.join("packed-refs")).unwrap_or_default();
    let exists =
        |name: &str| common.join(name).is_file() || packed.lines().any(|line| line.ends_with(name));
    let main = exists("refs/heads/main") || exists("refs/remotes/origin/main");
    let master = exists("refs/heads/master") || exists("refs/remotes/origin/master");
    if master && !main { "master" } else { "main" }.to_owned()
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

pub fn cd_target(word: &str, after_cd: &mut bool) -> bool {
    let target = *after_cd && !word.starts_with('-');
    *after_cd = matches!(word, "cd" | "pushd") || *after_cd && word.starts_with('-');
    target
}

/// The next shell word, quotes dropped; none when it is empty or built by expansion.
pub(crate) fn word(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<String> {
    while chars.next_if(|next| next.is_whitespace()).is_some() {}
    let (mut text, mut quote) = (String::new(), None);
    while let Some(c) =
        chars.next_if(|c| quote.is_some() || !(c.is_whitespace() || ";&|()<>".contains(*c)))
    {
        match (quote, c) {
            (Some(open), c) if c == open => quote = None,
            (None, '\'' | '"') => quote = Some(c),
            _ => text.push(c),
        }
    }
    (!text.is_empty() && !text.contains(['$', '`'])).then_some(text)
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
    let folded_path = folded(&lexical_normalize(path));
    write_folded_is_catastrophic(&folded_path, context)
        || (SYSTEM_PATHS_PROTECTED_RECURSIVELY
            .iter()
            .any(|protected| folded_path.starts_with(folded(Path::new(protected)))))
}

/// Whether writing a file here is categorically unacceptable: a file under a system path is
/// overwritten as `tee` would, so only destroying one stays denied there.
pub fn write_is_catastrophic(path: &Path, context: &CatastrophicContext) -> bool {
    write_folded_is_catastrophic(&folded(&lexical_normalize(path)), context)
}

fn write_folded_is_catastrophic(path: &Path, context: &CatastrophicContext) -> bool {
    if PROTECTED_SYSTEM_PATHS
        .iter()
        .any(|protected| path == folded(Path::new(protected)))
    {
        return true;
    }
    if (context.workspace_git.iter())
        .chain(&context.host_owned)
        .any(|guarded| path.starts_with(folded(&lexical_normalize(guarded))))
    {
        return true;
    }
    let Some(home) = &context.home_dir else {
        return false;
    };
    let home = folded(&lexical_normalize(home));
    if path == home {
        return true;
    }
    if PROTECTED_CREDENTIAL_SUBPATHS
        .iter()
        .any(|sub| path.starts_with(folded(&home.join(sub))))
    {
        return true;
    }
    PROTECTED_HOME_SUBPATHS
        .iter()
        .any(|sub| path == folded(&home.join(sub)))
}

/// Invariant: a default macOS volume folds letter case, so `~/.SSH` names `~/.ssh`; folding
/// both sides of a comparison only ever denies more.
fn folded(path: &Path) -> PathBuf {
    match cfg!(target_os = "macos") {
        true => PathBuf::from(path.to_string_lossy().to_lowercase()),
        false => path.to_path_buf(),
    }
}

/// The credential stores under `home`, the one list the destroy gate, the read gate, the bash
/// read belt and the sandbox's deny-read all take (D323).
pub fn credential_stores(home: &Path) -> Vec<PathBuf> {
    PROTECTED_CREDENTIAL_SUBPATHS
        .iter()
        .map(|sub| lexical_normalize(&home.join(sub)))
        .collect()
}

/// Keys a contained `yi mcp` reads, so no sandbox profile hides them: the MCP config and session
/// store keep each stdio server's `env` (#906). The read gate and the bash read belt refuse them.
const CONTAINED_KEY_FILES: [&str; 2] = [".yi/mcp.json", ".yi/mcp/sessions.json"];

fn stores(context: &CatastrophicContext) -> Vec<PathBuf> {
    let Some(home) = context.home_dir.as_deref() else {
        return Vec::new();
    };
    let files = CONTAINED_KEY_FILES
        .iter()
        .map(|sub| lexical_normalize(&home.join(sub)));
    credential_stores(home).into_iter().chain(files).collect()
}

/// Directories that hold users' key stores whatever HOME is, refused to a read's walk.
const HOME_ROOTS: [&str; 4] = ["/", "/home", "/Users", "/root"];

/// `/proc` entries that link elsewhere (`/proc/self/root` is `/`), unseen by a lexical check.
const PROC_LINKS: [&str; 4] = ["root", "cwd", "fd", "map_files"];

/// What a read may not touch (D180, D323): a key, the workspace `.git` or a device, which never
/// ends (`/dev/zero`) or waits (`/dev/tty`), and a directory a walk would carry into a key
/// store; judged by file identity too, so letter case, a link, `/private` or a firmlink names no
/// way in; [`ReadGate::open`] judges the file it opened, so no swapped link either (#890). Store
/// roots are judged per gate; hard-link membership is snapshotted lazily per gate, while ancestor
/// identities load only for directory targets.
pub struct ReadGate {
    context: CatastrophicContext,
    stores: Vec<PathBuf>,
    /// Guarded with all under them: the key stores, `/dev` and the workspace `.git`.
    trees: Vec<PathBuf>,
    tree_ids: Vec<FileId>,
    /// The directory ancestors of stores, compared only for directory targets.
    exact: Vec<PathBuf>,
    /// Lazily initialized per gate; not cached on `ToolContext`.
    exact_ids: std::sync::OnceLock<Vec<FileId>>,
    /// Lazily scanned once per gate, never retained between tool calls.
    hardlink_ids: std::sync::OnceLock<Option<std::collections::HashSet<FileId>>>,
    /// Paths no wall covers, with all under them: a session's family spills and own transcript
    /// under the roots its wall holds (#971).
    spared: Vec<PathBuf>,
    hold: Option<OpenHold>,
}

pub type OpenHold = std::sync::Arc<dyn Fn() + Send + Sync>;

impl ReadGate {
    pub fn new(context: &CatastrophicContext) -> Self {
        let stores = stores(context);
        let mut trees = stores.clone();
        trees.push(PathBuf::from("/dev"));
        trees.extend(context.workspace_git.iter().cloned());
        let above = stores.iter().flat_map(|store| store.ancestors().skip(1));
        let mut exact: Vec<PathBuf> = (HOME_ROOTS.iter().map(Path::new))
            .chain(above)
            .map(Path::to_path_buf)
            .collect();
        exact.sort_unstable();
        exact.dedup();
        let tree_ids = identities(&trees);
        Self {
            context: context.clone(),
            stores,
            trees,
            tree_ids,
            exact,
            exact_ids: std::sync::OnceLock::new(),
            hardlink_ids: std::sync::OnceLock::new(),
            spared: Vec::new(),
            hold: None,
        }
    }

    /// A test seam: `hold` runs inside each [`Self::open`] and [`Self::open_write`] as `held`
    /// does in [`Self::open_after`], so a test forces a swap inside a tool. Production sets none.
    pub fn hold_opens(&mut self, hold: Option<OpenHold>) {
        self.hold = hold;
    }

    fn held(&self) {
        if let Some(hold) = &self.hold {
            hold();
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
        if self.denies_spelling(path) {
            return true;
        }
        let metadata = fs::metadata(path).ok();
        beneath_via(&self.trees, &self.tree_ids, path, LINK_HOPS)
            || denied_file(&self.tree_ids, metadata.as_ref())
            || (metadata.as_ref().is_some_and(|meta| meta.is_dir())
                && denied_file(self.exact_ids(), metadata.as_ref()))
            || self.hard_linked_file(metadata.as_ref())
    }

    fn exact_ids(&self) -> &[FileId] {
        self.exact_ids.get_or_init(|| identities(&self.exact))
    }

    fn hard_linked_file(&self, metadata: Option<&fs::Metadata>) -> bool {
        credential::gate_hard_linked_file_in_stores(&self.stores, metadata, &self.hardlink_ids)
    }

    /// A walk's entry, whose ancestors the walk judged on its way in: the identity `meta` read
    /// without following it. Its spelling adds nothing, since every guarded directory has one.
    pub fn denies_entry(&self, meta: Option<&fs::Metadata>) -> bool {
        denied_file(&self.tree_ids, meta)
            || (meta.is_some_and(fs::Metadata::is_dir) && denied_file(self.exact_ids(), meta))
            || self.hard_linked_file(meta)
    }

    /// Opens the resolved `path`, judged with `walls`, with no link left to follow: the file judged
    /// is the file opened, so a link swapped in after an earlier check reaches nothing (#890).
    pub fn open(&self, path: &Path, walls: &[PathBuf]) -> io::Result<File> {
        self.open_after(path, walls, &mut || self.held())
    }

    /// A test seam: [`Self::open`] with `held` run between the judgement and the act, the one
    /// window a racing process has, so a test forces the swap. Production calls [`Self::open`].
    pub fn open_after(
        &self,
        path: &Path,
        walls: &[PathBuf],
        held: &mut dyn FnMut(),
    ) -> io::Result<File> {
        let real = fs::canonicalize(path)?;
        self.opened(&real, walls, READS, OpenOptions::new().read(true), held)
    }

    /// [`Self::open`] to write, under the destroy gate too: untruncated, or created only where
    /// nothing stands, not even a dangling link; the caller truncates through the handle.
    pub fn open_write(&self, path: &Path, walls: &[PathBuf]) -> io::Result<File> {
        self.open_write_after(path, walls, &mut || self.held())
    }

    /// A test seam: [`Self::open_write`] with `held` run before each judged open, as in
    /// [`Self::open_after`]. Production calls [`Self::open_write`].
    pub fn open_write_after(
        &self,
        path: &Path,
        walls: &[PathBuf],
        held: &mut dyn FnMut(),
    ) -> io::Result<File> {
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
        match self.opened(&real, walls, WRITES, &options, held) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.opened(&real, walls, WRITES, options.create_new(true), held)
            }
            opened => opened,
        }
    }

    /// Unlinks `path`, the link itself when it is one, once judged as a write, inside the parent
    /// held open as judged, so a parent swapped to a link afterwards deletes nothing behind it.
    pub fn remove(&self, path: &Path, walls: &[PathBuf]) -> io::Result<()> {
        self.remove_after(path, walls, &mut || ())
    }

    /// A test seam: [`Self::remove`] with `held` run after the judgement and again once the
    /// parent is held open, the two windows a racing process has. Production calls remove.
    pub fn remove_after(
        &self,
        path: &Path,
        walls: &[PathBuf],
        held: &mut dyn FnMut(),
    ) -> io::Result<()> {
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return Err(io::ErrorKind::InvalidInput.into());
        };
        let parent = fs::canonicalize(parent)?;
        let real = parent.join(name);
        self.guard_resolved(&real, walls, is_catastrophic)?;
        held();
        let dir = judged_dir(&parent, &real)?;
        held();
        Ok(unlinkat(&dir, name, AtFlags::empty())?)
    }

    fn guard_resolved(&self, real: &Path, walls: &[PathBuf], protects: Protects) -> io::Result<()> {
        let spared = beneath(&self.spared, real);
        let denied =
            self.denies(real) || protects(real, &self.context) || (beneath(walls, real) && !spared);
        if denied { Err(refused(real)) } else { Ok(()) }
    }

    fn opened(
        &self,
        real: &Path,
        walls: &[PathBuf],
        protects: Protects,
        options: &OpenOptions,
        held: &mut dyn FnMut(),
    ) -> io::Result<File> {
        self.guard_resolved(real, walls, protects)?;
        held();
        let (_dir, at) = beside(real)?;
        let file = no_link(options)?.open(&at)?;
        if !opened_at(&file, real) {
            return Err(refused(real));
        }
        let metadata = file.metadata()?;
        if self.hard_linked_file(Some(&metadata)) {
            return Err(refused(real));
        }
        Ok(file)
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

type Protects = fn(&Path, &CatastrophicContext) -> bool;
const READS: Protects = |_, _| false;
const WRITES: Protects = write_is_catastrophic;

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
pub fn file_id(meta: &fs::Metadata) -> Option<FileId> {
    use std::os::unix::fs::MetadataExt;
    Some((meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
pub fn file_id(_meta: &fs::Metadata) -> Option<FileId> {
    None
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

/// Wrappers that run another command: argv0 alone lets `nice rm -rf .git` past the belt (D205).
const WRAPPERS: [&str; 14] = [
    "builtin", "command", "doas", "env", "exec", "ionice", "nice", "nohup", "stdbuf", "strace",
    "sudo", "time", "timeout", "xargs",
];

pub(crate) const SEPARATORS: [char; 7] = [';', '&', '|', '\n', '(', ')', '`'];

/// Reading past a wrapper, keyword, flag, assignment or number widens what belt and wall see.
pub fn wraps(word: &str) -> bool {
    WRAPPERS.contains(&word.rsplit('/').next().unwrap_or(word))
        || crate::safety::OPENERS.contains(&word)
        || crate::safety::HEADERS.contains(&word)
        || word.starts_with('-')
        || word.contains('=')
        || word.chars().all(|character| character.is_ascii_digit())
}

const DESTRUCTIVE_COMMANDS: [&str; 4] = ["rm", "rmdir", "shred", "unlink"];

fn runs_destructive(segment: &str) -> bool {
    segment
        .split_whitespace()
        .find(|token| !wraps(token))
        .is_some_and(|token| {
            DESTRUCTIVE_COMMANDS.contains(&token.rsplit('/').next().unwrap_or(token))
        })
}

/// A destructive verb in any segment sends every path-shaped token of the whole command through
/// the denylist, since a quoted `;` may cut a segment short. No shell parsing (D15).
pub fn command_targets_catastrophic(
    command: &str,
    context: &CatastrophicContext,
) -> Option<String> {
    if !command.split(SEPARATORS).any(runs_destructive) {
        return None;
    }
    for token in command.split(|c: char| c.is_whitespace() || SEPARATORS.contains(&c)) {
        let token = token.trim_matches(['"', '\'']);
        if token.is_empty() || token.starts_with('-') {
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
