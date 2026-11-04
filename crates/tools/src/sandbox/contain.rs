//! Linux containment: `yi __contain` applies a Landlock ruleset and execs. A rule grants a whole
//! hierarchy of inodes that already exist, so a denial becomes the siblings of what it denies.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::Sandbox;

const READ_ROOTS: [&str; 10] = [
    "/usr", "/bin", "/sbin", "/lib", "/lib64", "/etc", "/opt", "/nix", "/dev", "/proc",
];
const WRITE_DEVICES: [&str; 6] = [
    "/dev/null",
    "/dev/zero",
    "/dev/urandom",
    "/dev/tty",
    "/dev/ptmx",
    "/dev/pts",
];
const GRANT_WALK_MAX: usize = 4_096;

static WRAPPER: OnceLock<PathBuf> = OnceLock::new();

/// The binary that holds `__contain`. Unset outside the real `yi`, so a test binary has no wrapper
/// unless a test names one.
pub fn set_wrapper(path: PathBuf) {
    let _first_wins = WRAPPER.set(path);
}

pub(super) fn wrapper() -> Option<&'static Path> {
    WRAPPER
        .get()
        .map(PathBuf::as_path)
        .filter(|_| cfg!(target_os = "linux"))
}

/// The kernel's answer, once: seccomp or an old kernel can refuse the ruleset calls.
pub(super) fn available() -> bool {
    static APPLIES: OnceLock<bool> = OnceLock::new();
    wrapper().is_some_and(|wrapper| {
        *APPLIES.get_or_init(|| {
            let probe = crate::process::command(wrapper)
                .args(["__contain", "--probe"])
                .output();
            probe.is_ok_and(|probe| probe.status.success())
        })
    })
}

/// `root` as rules: whole when nothing denied lies under it, else its children, so a `.ssh` below
/// costs listing the directory that holds it. Past the walk's limit it grants less, never more.
fn grants(root: &Path, denied: &[PathBuf], out: &mut Vec<PathBuf>) {
    let mut stack = vec![root.to_path_buf()];
    let mut walked = 0usize;
    while let Some(path) = stack.pop() {
        walked = walked.saturating_add(1);
        if walked > GRANT_WALK_MAX {
            return;
        }
        let real = path.canonicalize().unwrap_or_else(|_| path.clone());
        let inside = |at: &Path| denied.iter().any(|store| at.starts_with(store));
        if inside(&path) || inside(&real) {
            continue;
        }
        if denied
            .iter()
            .any(|store| store.starts_with(&path) || store.starts_with(&real))
        {
            stack.extend(
                std::fs::read_dir(&path)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|entry| entry.path()),
            );
        } else {
            out.push(path);
        }
    }
}

/// Toolchains and caches that live outside the listed roots and home.
fn env_roots() -> Vec<PathBuf> {
    let named = [
        "CARGO_HOME",
        "RUSTUP_HOME",
        "VIRTUAL_ENV",
        "UV_PYTHON_INSTALL_DIR",
    ];
    let single = named.iter().filter_map(std::env::var_os).map(PathBuf::from);
    let path = std::env::var_os("PATH");
    let on_path = path.iter().flat_map(std::env::split_paths);
    let exe = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    single
        .chain(on_path)
        .chain(exe)
        .filter(|dir| dir.is_absolute())
        .collect()
}

impl Sandbox {
    fn denied_reads(&self) -> Vec<PathBuf> {
        let resolved = self
            .deny_read
            .iter()
            .map(|path| yi_permission::resolve_links(path));
        self.deny_read.iter().cloned().chain(resolved).collect()
    }

    /// What `__contain` lets a process read. Reads open on macOS bar the stores are named here: a
    /// directory neither listed nor under home or a writable root reads as absent.
    #[must_use]
    pub fn read_grants(&self, own: Option<&Path>) -> Vec<PathBuf> {
        let denied = self.denied_reads();
        let home = crate::plugin::home();
        let roots = (READ_ROOTS.iter().map(PathBuf::from))
            .chain(home)
            .chain(env_roots())
            .chain(self.writable.iter().cloned());
        let mut out = Vec::new();
        for root in roots {
            grants(&root, &denied, &mut out);
        }
        out.extend(self.spared.iter().cloned());
        out.extend(own.map(Path::to_path_buf));
        out.sort();
        out.dedup();
        out
    }

    /// What `__contain` lets a process write. A leaf that does not exist has no inode to name, and
    /// git replaces a ref by rename: its parent is granted only where that parent is `yi`, so a lane
    /// keeps its branch and sibling `yi/*` refs, and `refs/stash` and `packed-refs.lock` refuse.
    #[must_use]
    pub fn write_grants(&self, own: Option<&Path>) -> Vec<PathBuf> {
        let denied = self.denied_reads();
        let mut out: Vec<PathBuf> = WRITE_DEVICES.iter().map(PathBuf::from).collect();
        for root in &self.writable {
            let parent = root
                .parent()
                .filter(|up| up.file_name().is_some_and(|name| name == "yi"));
            if let Some(parent) = parent {
                out.push(parent.to_path_buf());
            } else if root.exists() {
                grants(root, &denied, &mut out);
            }
        }
        out.extend(own.map(Path::to_path_buf));
        out.sort();
        out.dedup();
        out
    }

    /// Records what the host must find unchanged before its next git.
    pub fn seal_git_files(&self) {
        let files: Vec<PathBuf> = (self.deny_write.iter())
            .map(|path| match path.ends_with("refs/remotes/origin") {
                true => path.join("HEAD"),
                false => path.clone(),
            })
            .collect();
        let listed: Vec<PathBuf> = self
            .sealed
            .iter()
            .map(|sealed| sealed.denied.clone())
            .collect();
        super::seal::seal(&files, &listed);
    }

    /// `yi __contain --read … --write … -- env -u …`.
    pub(super) fn contain_args(&self, own: Option<&Path>, assign: &[String]) -> Vec<String> {
        self.seal_git_files();
        let flag = |name: &'static str, paths: Vec<PathBuf>| {
            paths
                .into_iter()
                .filter(|path| path.exists())
                .flat_map(move |path| [name.to_owned(), path.to_string_lossy().into_owned()])
        };
        std::iter::once("__contain".to_owned())
            .chain(flag("--read", self.read_grants(own)))
            .chain(flag("--write", self.write_grants(own)))
            .chain(std::iter::once("--".to_owned()))
            .chain(super::scrubbed_env())
            .chain(assign.iter().cloned())
            .collect()
    }
}
