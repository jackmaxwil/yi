use std::path::{Path, PathBuf};

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
}

/// Credential stores, matching the paths the permission layer already refuses
/// to destroy or read.
const CREDENTIAL_DIRS: [&str; 5] = [".ssh", ".gnupg", ".aws", ".kube", ".docker"];

impl Sandbox {
    /// The worktree plus the scratch space a build needs. Anything else is a
    /// read, and the model is told when a write lands outside.
    pub fn for_workspace(cwd: &Path, home: &Path, session_dir: Option<&Path>) -> Self {
        let mut writable = vec![cwd.to_path_buf()];
        if let Some(dir) = session_dir {
            writable.push(dir.to_path_buf());
        }
        writable.extend(temp_roots());
        writable.sort();
        writable.dedup();
        Self {
            writable,
            deny_read: CREDENTIAL_DIRS.iter().map(|dir| home.join(dir)).collect(),
        }
    }

    pub fn available() -> bool {
        cfg!(target_os = "macos") && Path::new(SEATBELT).is_file()
    }

    /// `-D` bindings keep paths out of the policy text. A denied path binds twice, logical
    /// and resolved: macOS `/tmp` and `/var` are symlinks and the kernel checks the target.
    pub fn params(&self) -> Vec<(String, PathBuf)> {
        let mut params = Vec::new();
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
        sections.join("\n")
    }

    pub fn kernel_policy(&self) -> String {
        format!(
            "{}\n; Jupyter ZMQ: the kernel binds loopback and the host connects to it.\n\
             ; No outbound rule, so a cell reaches no local service either.\n\
             (allow network-inbound (local ip \"localhost:*\"))\n\
             (allow network-bind (local ip \"localhost:*\"))\n",
            self.policy()
        )
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
        format!(
            "; writes are confined to what a checkpoint can undo\n(allow file-write*\n  {}\n)\n{}",
            allows.join(" "),
            anchors.join("\n")
        )
    }

    /// `sandbox-exec -p <policy> -DKEY=value ... -- <program> <args>`.
    pub fn wrap(&self, program: &str, args: &[&str]) -> (String, Vec<String>) {
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

/// Seatbelt reports a denial as an ordinary errno, so the caller cannot know it was the
/// sandbox: a non-zero exit that is no known shell failure, plus output naming a denial.
pub fn denial_hint(exit_code: Option<i32>, output: &str) -> Option<String> {
    const QUICK_REJECT: [i32; 3] = [2, 126, 127];
    const KEYWORDS: [&str; 5] = [
        "operation not permitted",
        "permission denied",
        "read-only file system",
        "sandbox",
        "deny file-write",
    ];
    let code = exit_code?;
    if code == 0 || QUICK_REJECT.contains(&code) {
        return None;
    }
    let lower = output.to_lowercase();
    if !KEYWORDS.iter().any(|needle| lower.contains(needle)) {
        return None;
    }
    Some(
        "next: the sandbox refused this (writes stay in the working tree, egress is off); running the same command again asks the user instead of containing it"
            .to_owned(),
    )
}
