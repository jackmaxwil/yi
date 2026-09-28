use std::path::{Path, PathBuf};

use serde_json::{Value, json};

const BASE_POLICY: &str = include_str!("../../../vendor/seatbelt/seatbelt_base_policy.sbpl");

/// Only `/usr/bin/sandbox-exec`, never one found on PATH: an attacker who can
/// put a fake earlier on PATH would otherwise choose the sandbox.
pub const SEATBELT: &str = "/usr/bin/sandbox-exec";

/// What a contained command may touch: reads open bar the credential stores, writes confined
/// to roots a checkpoint can undo, and no network rule, so `(deny default)` covers egress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sandbox {
    pub writable: Vec<PathBuf>,
    pub deny_read: Vec<PathBuf>,
    /// Paths inside a writable root that host code runs or reads back (D205).
    pub deny_write: Vec<PathBuf>,
    /// Loopback bind and inbound: the kernel's Jupyter ZMQ needs them, and its `bash()` jobs
    /// run under the kernel's profile (D241). Never outbound, not even to localhost.
    pub loopback: bool,
}

/// Credential stores, matching the paths the permission layer already refuses
/// to destroy or read.
const CREDENTIAL_DIRS: [&str; 5] = [".ssh", ".gnupg", ".aws", ".kube", ".docker"];

/// A git dir's escape hatches: `hooks` and `config` run under the host's next git, and the
/// pointers are what the next sandbox policy is built from.
const HOST_RUN_BY_GIT: [&str; 4] = ["hooks", "config", "commondir", "gitdir"];

impl Sandbox {
    /// The worktree, its git dirs (a lane's index lives in the trunk's), and build scratch.
    pub fn for_workspace(cwd: &Path, home: &Path, session_dir: Option<&Path>) -> Self {
        let mut writable = vec![cwd.to_path_buf()];
        let git_dirs = yi_permission::git_dirs(cwd);
        let deny_write = git_dirs
            .iter()
            .flat_map(|dir| HOST_RUN_BY_GIT.iter().map(|leaf| dir.join(leaf)))
            .collect();
        writable.extend(git_dirs);
        if let Some(dir) = session_dir {
            writable.push(dir.to_path_buf());
        }
        writable.extend(temp_roots());
        writable.sort();
        writable.dedup();
        Self {
            writable,
            deny_read: CREDENTIAL_DIRS.iter().map(|dir| home.join(dir)).collect(),
            deny_write,
            loopback: false,
        }
    }

    pub fn available() -> bool {
        cfg!(target_os = "macos") && Path::new(SEATBELT).is_file()
    }

    /// Outside every writable root, or under a denied one; the devices the base policy grants
    /// (`/dev/null`, `/dev/ptmx`, the ttys) are not denied, and every other device is.
    pub fn denies_write(&self, path: &Path) -> bool {
        let path = resolve_aliases(path);
        let under = |roots: &[PathBuf]| {
            roots
                .iter()
                .any(|root| path.starts_with(resolve_aliases(root)))
        };
        let tty = path
            .to_str()
            .and_then(|path| path.strip_prefix("/dev/ttys"))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|byte| byte.is_ascii_digit()));
        let granted = tty || path == Path::new("/dev/null") || path == Path::new("/dev/ptmx");
        !granted && (!under(&self.writable) || under(&self.deny_write))
    }

    /// `-D` bindings keep paths out of the policy text. A denied path binds twice, logical
    /// and resolved: macOS `/tmp` and `/var` are symlinks and the kernel checks the target.
    pub fn params(&self) -> Vec<(String, PathBuf)> {
        let mut params = Vec::new();
        for (index, path) in self.deny_write.iter().enumerate() {
            params.push((format!("DENY_WRITE_{index}"), resolve_aliases(path)));
        }
        for (index, root) in self.deny_read.iter().enumerate() {
            params.push((format!("DENY_READ_{index}"), root.clone()));
            params.push((format!("DENY_READ_{index}_RESOLVED"), resolve_aliases(root)));
        }
        for (index, root) in self.writable.iter().enumerate() {
            params.push((format!("WRITABLE_ROOT_{index}"), resolve_aliases(root)));
        }
        params
    }

    pub fn policy(&self) -> String {
        let mut sections = vec![BASE_POLICY.to_owned(), self.read_policy()];
        sections.push(self.write_policy());
        if self.loopback {
            sections.push(
                "; Jupyter ZMQ: the kernel binds loopback and the host connects to it.\n\
                 ; No outbound rule, so a cell reaches no local service either.\n\
                 (allow network-inbound (local ip \"localhost:*\"))\n\
                 (allow network-bind (local ip \"localhost:*\"))\n"
                    .to_owned(),
            );
        }
        sections.join("\n")
    }

    pub fn kernel_policy(&self) -> String {
        Self {
            loopback: true,
            ..self.clone()
        }
        .policy()
    }

    pub fn kernel_prefix(&self) -> (String, Vec<String>) {
        let mut wrapped = vec!["-p".to_owned(), self.kernel_policy()];
        for (key, value) in self.params() {
            wrapped.push(format!("-D{key}={}", value.to_string_lossy()));
        }
        wrapped.push("--".to_owned());
        (SEATBELT.to_owned(), wrapped)
    }

    fn read_policy(&self) -> String {
        if self.deny_read.is_empty() {
            return "; reads are open\n(allow file-read*)".to_owned();
        }
        let denials: Vec<String> = (0..self.deny_read.len())
            .flat_map(|index| {
                [
                    format!("DENY_READ_{index}"),
                    format!("DENY_READ_{index}_RESOLVED"),
                ]
            })
            .flat_map(|key| {
                [
                    format!("(require-not (literal (param \"{key}\")))"),
                    format!("(require-not (subpath (param \"{key}\")))"),
                ]
            })
            .collect();
        format!(
            "; reads are open except the credential stores\n(allow file-read*\n  (require-all (subpath \"/\") {})\n)",
            denials.join(" ")
        )
    }

    /// Invariant: the unlink denial on each root stops a contained process
    /// replacing the boundary its own policy is written against.
    fn write_policy(&self) -> String {
        if self.writable.is_empty() {
            return "; no writable root\n".to_owned();
        }
        let mut allows = Vec::new();
        let mut anchors = Vec::new();
        for index in 0..self.writable.len() {
            let key = format!("WRITABLE_ROOT_{index}");
            allows.push(format!("(subpath (param \"{key}\"))"));
            anchors.push(format!(
                "(deny file-write-unlink (require-all (literal (param \"{key}\")) (vnode-type DIRECTORY)))"
            ));
        }
        for index in 0..self.deny_write.len() {
            anchors.push(format!(
                "(deny file-write* (subpath (param \"DENY_WRITE_{index}\")))"
            ));
        }
        format!(
            "; writes are confined to what a checkpoint can undo\n(allow file-write*\n  {}\n)\n{}",
            allows.join(" "),
            anchors.join("\n")
        )
    }

    /// `sandbox-exec -p <policy> -DKEY=value ... -- <program> <args>`.
    pub fn wrap(&self, program: &str, args: &[&str]) -> (String, Vec<String>) {
        let _span = yi_types::trace::span("sandbox.wrap");
        let mut wrapped = vec!["-p".to_owned(), self.policy()];
        for (key, value) in self.params() {
            wrapped.push(format!("-D{key}={}", value.to_string_lossy()));
        }
        wrapped.push("--".to_owned());
        wrapped.push(program.to_owned());
        wrapped.extend(args.iter().map(|arg| (*arg).to_owned()));
        (SEATBELT.to_owned(), wrapped)
    }
}

/// Resolve the top-level alias only: deeper components can be replaced by a process already
/// inside the sandbox, so following them would turn a path check into a fresh grant.
fn resolve_aliases(path: &Path) -> PathBuf {
    let Some(top) = path.ancestors().find(|ancestor| {
        ancestor
            .parent()
            .is_some_and(|parent| parent.parent().is_none())
    }) else {
        return path.to_path_buf();
    };
    let is_symlink =
        std::fs::symlink_metadata(top).is_ok_and(|metadata| metadata.file_type().is_symlink());
    if !is_symlink {
        return path.to_path_buf();
    }
    match (top.canonicalize(), path.strip_prefix(top)) {
        (Ok(canonical), Ok(suffix)) => canonical.join(suffix),
        _ => path.to_path_buf(),
    }
}

fn temp_roots() -> Vec<PathBuf> {
    let mut roots = vec![std::env::temp_dir()];
    // macOS hands out a per-user TMPDIR under /var/folders and resolves /tmp
    // through /private; a build that writes to either must still run.
    if cfg!(target_os = "macos") {
        roots.push(PathBuf::from("/private/tmp"));
        roots.push(PathBuf::from("/private/var/folders"));
    } else {
        roots.push(PathBuf::from("/tmp"));
    }
    roots
}

/// Why a contained command failed, as the hint words it and the broker remembers it. The bash
/// tool finds it once, from the raw output, and hands it to the broker in its result details.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum SandboxRefusal {
    /// A write this profile denies.
    Path(PathBuf),
    /// No denied path found, so likely the network: the programs that ran.
    Scopes(Vec<String>),
}

impl SandboxRefusal {
    /// The bash result's `sandboxRefusal` detail: `{"path": …}` or `{"scopes": […]}`.
    pub fn to_json(&self) -> Value {
        match self {
            Self::Path(path) => json!({ "path": path }),
            Self::Scopes(scopes) => json!({ "scopes": scopes }),
        }
    }

    pub fn from_json(value: &Value) -> Option<Self> {
        if let Some(path) = value.get("path").and_then(Value::as_str) {
            return Some(Self::Path(PathBuf::from(path)));
        }
        let scopes = value.get("scopes")?.as_array()?;
        let scopes = scopes.iter().filter_map(Value::as_str).map(str::to_owned);
        Some(Self::Scopes(scopes.collect()))
    }

    /// Whether `command` retries this refusal: it writes a denied path under the refused one's
    /// directory, or names one there in any spelling when that directory is in home.
    pub fn retried_by(&self, sandbox: &Sandbox, cwd: &Path, command: &str) -> bool {
        let dir = match self {
            Self::Path(refused) => refused.parent().unwrap_or(refused),
            Self::Scopes(refused) => {
                return yi_permission::refused_scopes(command)
                    .iter()
                    .any(|scope| refused.contains(scope));
            }
        };
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let text = with_home_expanded(command, home.as_deref());
        let denied = |path: &Path| path.starts_with(dir) && sandbox.denies_write(path);
        let written = yi_permission::write_targets(&text)
            .iter()
            .any(|raw| denied(&absolute_path(raw, cwd)));
        // A read outside home never asks: `cat /etc/hosts` after a refused `/etc` write.
        let in_home = home.as_deref().is_some_and(|home| dir.starts_with(home));
        // The directory as spelled from `/` or from `~`, found anywhere in the text.
        let dir_text = dir.to_string_lossy();
        let from_tilde = (home.as_deref())
            .and_then(|home| Some(format!("~{}", dir_text.strip_prefix(home.to_str()?)?)));
        let named = [Some(dir_text.to_string()), from_tilde]
            .into_iter()
            .flatten()
            .any(|needle| {
                text.match_indices(needle.as_str()).any(|(at, _)| {
                    let rest = text.get(at..).unwrap_or_default();
                    let end = rest.find(|c: char| c.is_whitespace() || PATH_END.contains(&c));
                    denied(&absolute_path(
                        rest.get(..end.unwrap_or(rest.len())).unwrap_or(rest),
                        cwd,
                    ))
                })
            });
        written || (in_home && named)
    }
}

/// What ends a path spelled inside a command: a quote, a shell operator, a comma.
const PATH_END: [char; 12] = ['\'', '"', '`', ';', '|', '&', '<', '>', '(', ')', ',', '$'];

/// `command` with `${HOME}` and `$HOME` written as `home`; `resolve_target` reads `~` itself.
fn with_home_expanded(command: &str, home: Option<&Path>) -> String {
    let home = home.map_or_else(|| "$HOME".into(), Path::to_string_lossy);
    command.replace("${HOME}", &home).replace("$HOME", &home)
}

fn absolute_path(raw: &str, cwd: &Path) -> PathBuf {
    yi_permission::lexical_normalize(&yi_permission::resolve_target(raw, cwd))
}

/// The path of a line in the errno shape a refused write prints, `<prog>: <path>: Operation not
/// permitted` or Python's `[Errno 1] Operation not permitted: '<path>'`, unquoted.
fn errno_path(line: &str) -> Option<&str> {
    let line = line.trim_end();
    let raw = match line.strip_suffix(": Operation not permitted") {
        // `touch: /x`, or git's `fatal: could not create … '/x/.git'`, quoted.
        Some(head) => head
            .rsplit_once(": ")
            .map(|(_, path)| path)
            .filter(|path| !path.contains(char::is_whitespace))
            .or_else(|| {
                let word = head.rsplit(char::is_whitespace).next()?;
                (word.len() > 2 && word.starts_with('\'') && word.ends_with('\'')).then_some(word)
            })?,
        None => line.split_once("[Errno 1] Operation not permitted: ")?.1,
    };
    let raw = raw.trim_matches(['\'', '"']);
    (!raw.is_empty() && !raw.contains(char::is_whitespace)).then_some(raw)
}

/// The first write target this profile denies, once an output line has the errno shape; else,
/// when the command failed, the first denied path such a line names.
fn refused_path(
    sandbox: &Sandbox,
    cwd: &Path,
    exit_code: Option<i32>,
    output: &str,
    command: &str,
) -> Option<PathBuf> {
    let named: Vec<&str> = output.lines().filter_map(errno_path).collect();
    if named.is_empty() {
        return None;
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let targets = yi_permission::write_targets(&with_home_expanded(command, home.as_deref()));
    let failed = exit_code != Some(0);
    targets
        .iter()
        .map(String::as_str)
        .chain(named.into_iter().filter(|_| failed))
        .map(|raw| absolute_path(raw, cwd))
        .find(|path| sandbox.denies_write(path))
}

/// Seatbelt denies with a plain errno. A denied write target counts at any exit (`touch x |
/// tail -1` is 0); without one, a non-zero exit that is no known shell failure, naming a denial.
pub fn sandbox_refusal(
    sandbox: &Sandbox,
    cwd: &Path,
    exit_code: Option<i32>,
    output: &str,
    command: &str,
) -> Option<SandboxRefusal> {
    const QUICK_REJECT: [i32; 3] = [2, 126, 127];
    const DENIALS: [&str; 5] = [
        "operation not permitted",
        "permission denied",
        "read-only file system",
        "sandbox",
        "deny file-write",
    ];
    if let Some(path) = refused_path(sandbox, cwd, exit_code, output, command) {
        return Some(SandboxRefusal::Path(path));
    }
    let code = exit_code?;
    let lower = output.to_lowercase();
    let named = DENIALS.iter().any(|needle| lower.contains(needle));
    (code != 0 && !QUICK_REJECT.contains(&code) && named)
        .then(|| SandboxRefusal::Scopes(yi_permission::refused_scopes(command)))
}

pub fn denial_hint(refusal: &SandboxRefusal) -> String {
    const CONTAINED: &str =
        "a contained run writes only the working tree, its git dirs and tmp, and has no network";
    match refusal {
        SandboxRefusal::Path(path) => format!(
            "next: the sandbox refused writing `{}` ({CONTAINED}); the next call writing under `{}` outside those asks instead of running contained, and is refused where nobody can answer",
            path.display(),
            path.parent().unwrap_or(path).display()
        ),
        SandboxRefusal::Scopes(scopes) => {
            let needs = if scopes.len() == 1 { "needs" } else { "need" };
            let scopes: Vec<String> = scopes.iter().map(|scope| format!("`{scope}`")).collect();
            format!(
                "next: the sandbox refused this ({CONTAINED}); {} now {needs} permission: the next call using it asks instead of running contained, and is refused where nobody can answer",
                scopes.join(", ")
            )
        }
    }
}
