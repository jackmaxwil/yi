use std::path::{Path, PathBuf};

use serde_json::{Value, json};

const BASE_POLICY: &str = include_str!("../../../vendor/seatbelt/seatbelt_base_policy.sbpl");

/// Only `/usr/bin/sandbox-exec`, never one found on PATH: an attacker who can
/// put a fake earlier on PATH would otherwise choose the sandbox.
pub const SEATBELT: &str = "/usr/bin/sandbox-exec";

/// What a contained command may touch: reads open bar the credential stores, writes confined
/// to roots a checkpoint can undo, and loopback the only network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sandbox {
    pub writable: Vec<PathBuf>,
    pub deny_read: Vec<PathBuf>,
    /// Paths inside a writable root that host code runs or reads back (D205).
    pub deny_write: Vec<PathBuf>,
    /// Stores only the host writes, the session corpus above all (its JSONL is the ledger kept
    /// rules replay from): denied to writes, save a writable root nested inside one.
    pub host_owned: Vec<PathBuf>,
}

/// Loopback for every profile (D329). Seatbelt's `localhost` is every address of this host and
/// the wildcard, so a listener can face the LAN; no rule narrows it (probed).
const NETWORK_POLICY: &str = "; loopback only, and no unix socket\n\
    (allow network-bind network-inbound (local ip \"localhost:*\"))\n\
    (allow network-outbound (remote ip \"localhost:*\"))\n";

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
            // The stores the read gate refuses, so a contained `cat` meets the same list (D323).
            deny_read: yi_permission::credential_stores(home),
            deny_write,
            host_owned: vec![home.join(".yi/sessions")],
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
        let owned = self.host_owned.iter().any(|store| {
            let (store, exempt) = (resolve_aliases(store), self.exempt(store));
            path.starts_with(&store) && !exempt.iter().any(|root| path.starts_with(root))
        });
        !granted
            && (!under(&self.writable)
                || under(&self.deny_write)
                || under(&self.deny_read)
                || owned)
    }

    /// The writable roots strictly inside a host-owned store: a child's own directory, the
    /// family board, a kernel's state.
    fn exempt(&self, store: &Path) -> Vec<PathBuf> {
        let store = resolve_aliases(store);
        (self.writable.iter())
            .map(|root| resolve_aliases(root))
            .filter(|root| root.starts_with(&store) && *root != store)
            .collect()
    }

    /// `-D` bindings keep paths out of the policy text. A denied path binds twice: as spelled,
    /// which a link itself is judged by, and resolved, since the kernel checks an open's target.
    pub fn params(&self) -> Vec<(String, PathBuf)> {
        self.params_for(&self.denied_parents(), None)
    }

    fn params_for(&self, parents: &[PathBuf], own: Option<&Path>) -> Vec<(String, PathBuf)> {
        let mut params = Vec::new();
        if let Some(own) = own {
            params.push(("OWN_CONNECTION".to_owned(), own.to_path_buf()));
            // Never `own` itself resolved: the kernel writes there and could plant a link.
            let parent = own.parent().map(yi_permission::resolve_links);
            let resolved = parent
                .zip(own.file_name())
                .map(|(dir, name)| dir.join(name));
            params.push((
                "OWN_CONNECTION_RESOLVED".to_owned(),
                resolved.unwrap_or_else(|| own.to_path_buf()),
            ));
        }
        for (index, path) in self.deny_write.iter().enumerate() {
            params.push((format!("DENY_WRITE_{index}"), resolve_aliases(path)));
            params.push((
                format!("DENY_WRITE_{index}_RESOLVED"),
                yi_permission::resolve_links(path),
            ));
        }
        for (index, root) in self.deny_read.iter().enumerate() {
            params.push((format!("DENY_READ_{index}"), root.clone()));
            params.push((
                format!("DENY_READ_{index}_RESOLVED"),
                yi_permission::resolve_links(root),
            ));
        }
        for (index, store) in self.host_owned.iter().enumerate() {
            params.push((format!("HOST_OWNED_{index}"), resolve_aliases(store)));
            params.push((
                format!("HOST_OWNED_{index}_RESOLVED"),
                yi_permission::resolve_links(store),
            ));
        }
        for (index, dir) in parents.iter().enumerate() {
            params.push((format!("DENIED_PARENT_{index}"), dir.clone()));
        }
        for (index, root) in self.writable.iter().enumerate() {
            params.push((format!("WRITABLE_ROOT_{index}"), resolve_aliases(root)));
        }
        params
    }

    pub fn policy(&self) -> String {
        self.policy_for(self.denied_parents().len(), false, true)
    }

    fn policy_for(&self, parents: usize, own: bool, network: bool) -> String {
        let mut sections = vec![BASE_POLICY.to_owned(), self.read_policy()];
        sections.push(self.write_policy(parents));
        if own {
            // Last, so it outranks the deny on the connection root: file rules are last-match.
            sections.push(
                "; this kernel's own connection file\n\
                 (allow file-read* file-write* (subpath (param \"OWN_CONNECTION\")) \
                 (subpath (param \"OWN_CONNECTION_RESOLVED\")))"
                    .to_owned(),
            );
        }
        if network {
            sections.push(NETWORK_POLICY.to_owned());
        }
        sections.join("\n")
    }

    /// The kernel's prefix: its profile plus read and write of `connection_dir`, which every
    /// other profile hides under the connection root (#599).
    pub fn kernel_prefix(&self, connection_dir: &Path) -> (String, Vec<String>) {
        (
            SEATBELT.to_owned(),
            self.seatbelt_args(Some(connection_dir), true),
        )
    }

    /// `-p <policy> -DKEY=value ... -- env -u …`: the rules and their bindings come from one
    /// reading of the filesystem, so a link swapped between the two cannot unbind a rule.
    fn seatbelt_args(&self, own: Option<&Path>, network: bool) -> Vec<String> {
        let parents = self.denied_parents();
        let mut wrapped = vec![
            "-p".to_owned(),
            self.policy_for(parents.len(), own.is_some(), network),
        ];
        for (key, value) in self.params_for(&parents, own) {
            wrapped.push(format!("-D{key}={}", value.to_string_lossy()));
        }
        wrapped.push("--".to_owned());
        wrapped.extend(scrubbed_env());
        wrapped
    }

    /// The directories between a writable root and a denied path: renaming one would carry the
    /// denied path out from under its own rule, which names it by where it was.
    fn denied_parents(&self) -> Vec<PathBuf> {
        let roots: Vec<PathBuf> = self
            .writable
            .iter()
            .map(|root| resolve_aliases(root))
            .collect();
        let mut parents: Vec<PathBuf> = (self.deny_write.iter())
            .chain(&self.deny_read)
            .chain(&self.host_owned)
            .flat_map(|path| [resolve_aliases(path), yi_permission::resolve_links(path)])
            .flat_map(|path| {
                path.ancestors()
                    .skip(1)
                    .map(Path::to_path_buf)
                    .collect::<Vec<_>>()
            })
            .filter(|dir| roots.iter().any(|root| dir.starts_with(root)))
            .collect();
        parents.sort();
        parents.dedup();
        parents
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
    fn write_policy(&self, parents: usize) -> String {
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
        // A path hidden from reads is not writable either: renamed, it would leave its rule behind.
        let denied = (0..self.deny_write.len())
            .map(|index| format!("DENY_WRITE_{index}"))
            .chain((0..self.deny_read.len()).map(|index| format!("DENY_READ_{index}")));
        for key in denied {
            anchors.push(format!(
                "(deny file-write* (subpath (param \"{key}\")) (subpath (param \"{key}_RESOLVED\")))"
            ));
        }
        for (index, store) in self.host_owned.iter().enumerate() {
            let exempt = self.exempt(store);
            let keep: String = (self.writable.iter().enumerate())
                .filter(|(_, root)| exempt.contains(&resolve_aliases(root)))
                .map(|(root, _)| {
                    format!(" (require-not (subpath (param \"WRITABLE_ROOT_{root}\")))")
                })
                .collect();
            for key in [
                format!("HOST_OWNED_{index}"),
                format!("HOST_OWNED_{index}_RESOLVED"),
            ] {
                anchors.push(format!(
                    "(deny file-write* (require-all (subpath (param \"{key}\")){keep}))"
                ));
            }
        }
        for index in 0..parents {
            anchors.push(format!(
                "(deny file-write-unlink (literal (param \"DENIED_PARENT_{index}\")))"
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
        self.wrap_with(true, program, args)
    }

    /// [`Self::wrap`] with no network at all, loopback included: the document converter parses
    /// untrusted files and needs none.
    pub fn wrap_offline(&self, program: &str, args: &[&str]) -> (String, Vec<String>) {
        self.wrap_with(false, program, args)
    }

    fn wrap_with(&self, network: bool, program: &str, args: &[&str]) -> (String, Vec<String>) {
        let _span = yi_types::trace::span("sandbox.wrap");
        let mut wrapped = self.seatbelt_args(None, network);
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

/// `env -u` for every inherited variable named like a secret (#906). A contained process has
/// no network past loopback to spend a key on, only output, files and local services.
fn scrubbed_env() -> Vec<String> {
    let mut names: Vec<String> = std::env::vars_os()
        // ponytail: a non-UTF-8 name is kept, `env -u` taking only what a String carries; one
        // holding `=` too, since `env -u` refuses it and would stop every contained spawn.
        .filter_map(|(name, _)| name.into_string().ok())
        .filter(|name| !name.contains('=') && is_secret_variable(name))
        .collect();
    names.sort();
    let unset = names.into_iter().flat_map(|name| ["-u".to_owned(), name]);
    std::iter::once("/usr/bin/env".to_owned())
        .chain(unset)
        .chain(std::iter::once(java_ipv4()))
        .collect()
}

/// A JVM's dual-stack socket dials 127.0.0.1 as `::ffff:127.0.0.1`, which Seatbelt refuses; IPv4
/// sockets keep Gradle, Maven and sbt on loopback. Appended to the value the run inherits.
fn java_ipv4() -> String {
    const IPV4: &str = "-Djava.net.preferIPv4Stack=true";
    match std::env::var("JAVA_TOOL_OPTIONS") {
        Ok(options) if !options.trim().is_empty() => format!("JAVA_TOOL_OPTIONS={options} {IPV4}"),
        _ => format!("JAVA_TOOL_OPTIONS={IPV4}"),
    }
}

/// Secrets named as nothing else is: a connection string, a password manager's unlocked session.
const SECRET_NAMES: [&str; 7] = [
    "BW_SESSION",
    "DATABASE_URL",
    "MYSQL_PWD",
    "NPM_CONFIG__AUTH",
    "REDIS_URL",
    "SENTRY_DSN",
    "SLACK_WEBHOOK_URL",
];

/// A heuristic over names, never values (D324): a `_`-separated word ending in a manifest mark or
/// `PASSWD`, `PAT`, `AUTH`, singular or plural, or a listed name. `SSH_AUTH_SOCK` and
/// `PASSWORD_STORE_DIR` go too: a contained run needs neither. It misses a secret under a plain
/// name (`FOO_URL=postgres://u:p@h`).
fn is_secret_variable(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    let marked = |word: &str| {
        let word = word.strip_suffix('S').unwrap_or(word);
        (yi_types::SECRET_NAME_MARKS.iter())
            .chain(&["PASSWD", "PAT", "AUTH"])
            .any(|mark| word.ends_with(mark))
    };
    SECRET_NAMES.contains(&upper.as_str())
        || upper.starts_with("OP_SESSION_")
        || upper.split('_').any(marked)
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
            // An empty or relative path would sit above every path it is compared with.
            return Path::new(path)
                .is_absolute()
                .then(|| Self::Path(PathBuf::from(path)));
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
        // The directory as spelled from `/` or from `~`, found anywhere in the text.
        let dir_text = dir.to_string_lossy();
        let from_tilde = (home.as_deref())
            .and_then(|home| Some(format!("~{}", dir_text.strip_prefix(home.to_str()?)?)));
        let named = [Some(dir_text.to_string()), from_tilde]
            .into_iter()
            .flatten()
            .any(|needle| {
                text.match_indices(needle.as_str()).any(|(at, _)| {
                    // A shell expands `~` only at a word's start, or after `=` or `:`, and up to
                    // a `/` or the word's end: `rm *~` and `s~a~b~` name no home path.
                    let before = text.get(..at).and_then(|head| head.chars().next_back());
                    let after = text
                        .get(at + needle.len()..)
                        .and_then(|tail| tail.chars().next());
                    let inside = |c: Option<char>, ends: &str| {
                        c.is_some_and(|c| !c.is_whitespace() && !ends.contains(c))
                    };
                    if needle.starts_with('~')
                        && (inside(before, "=:;|&()<>") || inside(after, "/;|&)<>"))
                    {
                        return false;
                    }
                    let rest = text.get(at..).unwrap_or_default();
                    let end = rest.find(|c: char| c.is_whitespace() || PATH_END.contains(&c));
                    denied(&absolute_path(
                        rest.get(..end.unwrap_or(rest.len())).unwrap_or(rest),
                        cwd,
                    ))
                })
            });
        // A read outside home never asks: `cat /etc/hosts` after a refused `/etc` write.
        written || (in_home(dir) && named)
    }
}

/// Whether a refusal in `dir` also asks for a call that only names a path there.
fn in_home(dir: &Path) -> bool {
    std::env::var_os("HOME").is_some_and(|home| dir.starts_with(home))
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
/// permitted` (Rust adds ` (os error 1)`), Python's `[Errno 1] …: '<path>'` or node's `EPERM`.
fn errno_path(line: &str) -> Option<&str> {
    let line = line.trim_end();
    let line = line.strip_suffix(" (os error 1)").unwrap_or(line);
    let raw = match line.strip_suffix(": Operation not permitted") {
        // `touch: /x`, or a quoted path: git's `'/x/.git'`, uv's `` `/x` ``.
        Some(head) => head
            .rsplit_once(": ")
            .map(|(_, path)| path)
            .filter(|path| !path.contains(char::is_whitespace))
            .or_else(|| {
                let word = head.rsplit(char::is_whitespace).next()?;
                let quote = word.chars().next().filter(|c| matches!(c, '\'' | '`'))?;
                (word.len() > 2 && word.ends_with(quote)).then_some(word)
            })?,
        None => match line.split_once("[Errno 1] Operation not permitted: ") {
            Some((_, path)) => path,
            // `Error: EPERM: operation not permitted, open '/x'`
            None => (line.split_once("EPERM: operation not permitted, ")?.1)
                .rsplit(char::is_whitespace)
                .next()?,
        },
    };
    let raw = raw.trim_matches(['\'', '"', '`']);
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

/// Seatbelt denies with a plain errno, or at the resolver. A denied write target counts at any
/// exit (`touch x | tail -1` is 0); else a non-zero exit, no known shell failure, naming a denial.
pub fn sandbox_refusal(
    sandbox: &Sandbox,
    cwd: &Path,
    exit_code: Option<i32>,
    output: &str,
    command: &str,
) -> Option<SandboxRefusal> {
    const QUICK_REJECT: [i32; 3] = [2, 126, 127];
    const DENIALS: [&str; 7] = [
        "operation not permitted",
        "permission denied",
        "read-only file system",
        "sandbox",
        "deny file-write",
        // curl, git and cargo: `Could not resolve host`, `Couldn't resolve host name`.
        "resolve host",
        // getaddrinfo's EAI_NONAME, as Python and ssh print it.
        "nodename nor servname",
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
    const CONTAINED: &str = "a contained run writes only the working tree, its git dirs and tmp, and has no network beyond 127.0.0.1 and ::1 (a dual-stack socket's `::ffff:127.0.0.1` is refused)";
    match refusal {
        SandboxRefusal::Path(path) => {
            let dir = path.parent().unwrap_or(path);
            let rule = if in_home(dir) {
                "naming a path"
            } else {
                "writing"
            };
            format!(
                "next: the sandbox refused writing `{}` ({CONTAINED}); the next call {rule} under `{}` outside those asks: approving widens that run by that directory, or runs it outside the sandbox where the directory is protected; nobody to answer refuses it",
                path.display(),
                dir.display()
            )
        }
        SandboxRefusal::Scopes(scopes) => {
            let needs = if scopes.len() == 1 { "needs" } else { "need" };
            let scopes: Vec<String> = scopes.iter().map(|scope| format!("`{scope}`")).collect();
            format!(
                "next: the sandbox refused this ({CONTAINED}); {} now {needs} permission: the next call using it asks, and approving runs that one call outside the sandbox; nobody to answer refuses it",
                scopes.join(", ")
            )
        }
    }
}
