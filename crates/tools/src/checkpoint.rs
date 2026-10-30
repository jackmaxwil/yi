use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeId(String);

impl TreeId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Restored,
    Deleted,
    Kept,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub path: PathBuf,
    pub kind: ChangeKind,
}

#[derive(Debug, thiserror::Error)]
pub enum CheckpointError {
    #[error("git is unavailable: {0}")]
    GitMissing(String),
    #[error("git {command} failed: {message}")]
    Git { command: String, message: String },
}

/// A shadow gitdir beside the project, never its own `.git`: checkpoints must
/// not touch the user's branch, index or stash.
pub struct Checkpoints {
    git_dir: PathBuf,
    work_tree: PathBuf,
    serial: Arc<Mutex<()>>,
    staged: Vec<String>,
}

// Per gitdir, not per handle: git fails hard on index.lock, and one shadow has several openers.
fn serial_for(git_dir: &Path) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    let mut locks = LOCKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    Arc::clone(locks.entry(git_dir.to_path_buf()).or_default())
}

impl Checkpoints {
    pub fn open(checkpoint_root: &Path, project: &Path) -> Result<Self, CheckpointError> {
        let git_dir = checkpoint_root.join(project_key(project));
        std::fs::create_dir_all(&git_dir)
            .map_err(|error| CheckpointError::GitMissing(error.to_string()))?;
        let real = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let mut staged = ["add", "--all", "--", "."].map(str::to_owned).to_vec();
        // Incident: `yi` started in `~` staged the shadow into itself, each capture slower.
        if let Ok(inside) = real(checkpoint_root).strip_prefix(real(project)) {
            staged.push(format!(":(exclude,literal){}", inside.to_string_lossy()));
        }
        let checkpoints = Self {
            serial: serial_for(&git_dir),
            git_dir,
            work_tree: project.to_path_buf(),
            staged,
        };
        if !checkpoints.git_dir.join("HEAD").exists() {
            checkpoints.git(&["init", "--quiet"])?;
        }
        Ok(checkpoints)
    }

    /// Honours the project's own ignore rules, read from the work tree.
    pub fn capture(&self) -> Result<TreeId, CheckpointError> {
        self.stage()?;
        self.write_tree()
    }

    /// A capture with `excluded` (paths relative to the project) left out of the tree; the
    /// next `add --all` puts them back in the shadow index.
    pub fn capture_excluding(&self, excluded: &[&Path]) -> Result<TreeId, CheckpointError> {
        self.stage()?;
        for path in excluded {
            let path = path.to_string_lossy();
            self.git(&[
                "rm",
                "-r",
                "-q",
                "--cached",
                "--ignore-unmatch",
                "--",
                &path,
            ])?;
        }
        self.write_tree()
    }

    pub fn changed(&self, tree: &TreeId) -> Result<Vec<Change>, CheckpointError> {
        self.stage()?;
        let diff = self.git(&[
            "diff",
            "--no-renames",
            "--name-status",
            "-z",
            "--cached",
            tree.as_str(),
        ])?;
        Ok(parse_changes(&diff))
    }

    /// With `since` (the tree the turn left) only the paths that moved between the two trees
    /// move back, and one changed again since is kept; without it, every path since `tree`.
    pub fn restore(
        &self,
        tree: &TreeId,
        since: Option<&TreeId>,
    ) -> Result<Vec<Change>, CheckpointError> {
        let changes = match since {
            None => self.changed(tree)?,
            Some(since) => {
                let drifted: HashSet<PathBuf> =
                    self.changed(since)?.into_iter().map(|c| c.path).collect();
                let moved = self.git(&[
                    "diff",
                    "--no-renames",
                    "--name-status",
                    "-z",
                    tree.as_str(),
                    since.as_str(),
                ])?;
                parse_changes(&moved)
                    .into_iter()
                    // ponytail: skip, not merge; `git merge-file` three-way if a user asks.
                    .map(|change| match drifted.contains(&change.path) {
                        true => Change {
                            kind: ChangeKind::Kept,
                            ..change
                        },
                        false => change,
                    })
                    .collect()
            }
        };
        let mut pathspecs = Vec::new();
        for change in &changes {
            match change.kind {
                ChangeKind::Restored => {
                    pathspecs.extend_from_slice(b":(literal)");
                    pathspecs.extend_from_slice(change.path.to_string_lossy().as_bytes());
                    pathspecs.push(0);
                }
                ChangeKind::Deleted => {
                    let _absent_is_the_goal =
                        std::fs::remove_file(self.work_tree.join(&change.path));
                }
                ChangeKind::Kept => {}
            }
        }
        // Invariant: an empty pathspec makes `checkout <tree>` a switch, so none runs then.
        if !pathspecs.is_empty() {
            let checkout = [
                "checkout",
                tree.as_str(),
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ];
            self.git_fed(&checkout, Some(&pathspecs))?;
        }
        self.stage()?;
        Ok(changes)
    }

    pub fn diff(&self, from: &TreeId, to: &TreeId) -> Result<crate::GitPatch, CheckpointError> {
        let text = self.git(&["diff", from.as_str(), to.as_str()])?;
        Ok(crate::diff::GitPatch::from_text(text))
    }

    /// Invariant: `<tree>:<path>` is one revision argument, and it opens with
    /// the tree's hex, so no path can present itself to git as an option.
    pub fn show(&self, tree: &TreeId, path: &str) -> Result<String, CheckpointError> {
        self.git(&["show", &format!("{}:{path}", tree.as_str())])
    }

    /// Writes the tree into `into` as a work tree of its own, through a private index, so the
    /// shadow index and the project's checkout are untouched.
    pub fn materialize(&self, tree: &TreeId, into: &Path) -> Result<(), CheckpointError> {
        std::fs::create_dir_all(into)
            .map_err(|error| CheckpointError::GitMissing(error.to_string()))?;
        let index = into.with_extension("index");
        let _serialized = self
            .serial
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for args in [
            vec!["read-tree", tree.as_str()],
            vec!["checkout-index", "-a", "-f"],
        ] {
            let mut process = host_git();
            process
                .env("GIT_INDEX_FILE", &index)
                .arg("--git-dir")
                .arg(&self.git_dir)
                .arg("--work-tree")
                .arg(into)
                .args(&args)
                .current_dir(into);
            let output = process
                .output()
                .map_err(|error| CheckpointError::GitMissing(error.to_string()))?;
            if !output.status.success() {
                return Err(CheckpointError::Git {
                    command: args.first().copied().unwrap_or_default().to_owned(),
                    message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
                });
            }
        }
        let _index_is_scratch = std::fs::remove_file(&index);
        Ok(())
    }

    /// Incident: `write-tree` returns a cached tree without touching it, so an unchanged
    /// project's capture kept a months-old mtime and the 30-day prune took it minutes later.
    #[expect(
        clippy::disallowed_methods,
        reason = "the prune ages captures by object mtime"
    )]
    fn write_tree(&self) -> Result<TreeId, CheckpointError> {
        let tree = TreeId::new(self.git(&["write-tree"])?.trim());
        if let (Some(fan), Some(rest)) = (tree.as_str().get(..2), tree.as_str().get(2..)) {
            let object = self.git_dir.join("objects").join(fan).join(rest);
            let now = std::time::SystemTime::now();
            let _packed_or_gone =
                std::fs::File::open(object).and_then(|file| file.set_modified(now));
        }
        Ok(tree)
    }

    fn stage(&self) -> Result<String, CheckpointError> {
        self.git(&self.staged.iter().map(String::as_str).collect::<Vec<_>>())
    }

    fn git(&self, args: &[&str]) -> Result<String, CheckpointError> {
        self.git_fed(args, None)
    }

    fn git_fed(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<String, CheckpointError> {
        let verb = args.first().copied().unwrap_or_default();
        let _span = yi_types::trace::span("checkpoint.git").arg("verb", verb);
        let _serialized = self
            .serial
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut process = host_git();
        process
            .arg("--git-dir")
            .arg(&self.git_dir)
            .arg("--work-tree")
            .arg(&self.work_tree)
            .args(args)
            .current_dir(&self.work_tree);
        let never: crate::tool::CancelFlag = Arc::new(|| false);
        let capture =
            crate::process::run_captured(process, stdin.map(<[u8]>::to_vec), &never, usize::MAX)
                .map_err(CheckpointError::GitMissing)?;
        if capture.exit_code != Some(0) {
            return Err(CheckpointError::Git {
                command: verb.to_owned(),
                message: capture.stderr.trim().to_owned(),
            });
        }
        Ok(capture.stdout)
    }
}

/// A git Yi drives on its own repository: a caller's `GIT_INDEX_FILE`, `GIT_WORK_TREE` or
/// `GIT_DIR` never redirects it, and paths come back unquoted.
fn host_git() -> std::process::Command {
    let mut process = crate::process::keyless_command("git");
    for (variable, _) in std::env::vars_os() {
        if variable.to_string_lossy().starts_with("GIT_") {
            process.env_remove(variable);
        }
    }
    process.args(["-c", "core.quotepath=false"]);
    process
}

/// Read relative to the earlier tree from `-z` rows (status NUL path NUL), with renames split by
/// `--no-renames`: `A` is a path the turn added, everything else one to check out.
fn parse_changes(diff: &str) -> Vec<Change> {
    let mut fields = diff.split('\0');
    let mut changes = Vec::new();
    while let (Some(status), Some(path)) = (fields.next(), fields.next()) {
        let kind = match status.chars().next() {
            Some('A') => ChangeKind::Deleted,
            Some(_) => ChangeKind::Restored,
            None => continue,
        };
        changes.push(Change {
            path: PathBuf::from(path),
            kind,
        });
    }
    changes
}

/// The project a shadow dir captures, as `init --work-tree` wrote it into its config; read
/// without a git process because a sweep visits thousands. A quoted or escaped value is not.
pub fn work_tree(git_dir: &Path) -> Option<PathBuf> {
    let config = std::fs::read_to_string(git_dir.join("config")).ok()?;
    let value = config
        .lines()
        .find_map(|line| line.trim().strip_prefix("worktree = "))?;
    (!value.starts_with('"') && !value.contains('\\')).then(|| PathBuf::from(value))
}

/// Invariant: captures write no ref, so every object is unreachable and `prune` keeps what is
/// younger than `keep` plus whatever a younger tree still names.
pub fn prune(git_dir: &Path, keep: std::time::Duration) -> Result<(), CheckpointError> {
    let output = host_git()
        .arg("--git-dir")
        .arg(git_dir)
        .arg("prune")
        .arg(format!("--expire={}.seconds.ago", keep.as_secs()))
        .output()
        .map_err(|error| CheckpointError::GitMissing(error.to_string()))?;
    if output.status.success() {
        return Ok(());
    }
    Err(CheckpointError::Git {
        command: "prune".to_owned(),
        message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    })
}

/// Hashed so the directory name cannot collide with a path component.
fn project_key(project: &Path) -> String {
    let hash = xxhash_rust::xxh32::xxh32(project.to_string_lossy().as_bytes(), 0);
    format!("{hash:08x}")
}
