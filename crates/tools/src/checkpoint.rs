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
    #[error("git {command} past its deadline")]
    Deadline { command: String },
}

/// A shadow gitdir beside the project, never its own `.git`, so the user's branch, index and stash
/// stay untouched. One per repository: its worktrees share the objects, each with its own index.
pub struct Checkpoints {
    git_dir: PathBuf,
    work_tree: PathBuf,
    serial: Arc<Mutex<()>>,
    staged: Vec<String>,
    /// `None` is a store from before one was shared, whose own `index` is this tree's.
    index: Option<PathBuf>,
    /// The per-path store this tree used before, which still holds its old sessions' trees.
    legacy: Option<PathBuf>,
    /// Set only on the handle [`Self::capture_within`] makes.
    until: Option<std::time::Instant>,
}

const SERIAL_LOCKS_MAX: usize = 1_024;

// Per gitdir, not per handle: git fails hard on index.lock, and one shadow has several openers.
fn serial_for(git_dir: &Path) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    let mut locks = LOCKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    // One index per tree, so a family's locks would otherwise outlive it.
    if locks.len() >= SERIAL_LOCKS_MAX {
        locks.retain(|_, lock| Arc::strong_count(lock) > 1);
    }
    Arc::clone(locks.entry(git_dir.to_path_buf()).or_default())
}
/// Not project content: the home holding the store, or, in a home that is itself the work tree,
/// the store and the caches spared beside it. Incident: a probe home there stored its venv again.
fn not_project_content(checkpoint_root: &Path, project: &Path) -> Vec<PathBuf> {
    let subtree = checkpoint_root
        .parent()
        .filter(|dir| dir.file_name().is_some_and(|name| name == ".yi"))
        .and_then(Path::parent);
    match subtree {
        Some(home) if real(home) == real(project) => [".yi", ".cache", ".local", ".cargo"]
            .into_iter()
            .map(|spared| home.join(spared))
            .collect(),
        Some(home) => vec![home.to_path_buf()],
        None => vec![checkpoint_root.to_path_buf()],
    }
}

fn real(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn staged_for(checkpoint_root: &Path, project: &Path) -> Vec<String> {
    let mut staged = ["add", "--all", "--", "."].map(str::to_owned).to_vec();
    // Incident: `yi` started in `~` staged the shadow into itself, each capture slower.
    for root in not_project_content(checkpoint_root, project) {
        if let Ok(inside) = real(&root).strip_prefix(real(project)) {
            staged.push(format!(":(exclude,literal){}", inside.to_string_lossy()));
        }
    }
    staged
}

impl Checkpoints {
    pub fn open(checkpoint_root: &Path, project: &Path) -> Result<Self, CheckpointError> {
        let common = yi_permission::git_dirs(project).pop();
        let keyed = common.as_deref().unwrap_or(project);
        let git_dir = checkpoint_root.join(project_key(keyed));
        // `core.worktree` names the repository's main tree, which is what retention asks about.
        let main_tree = (common.as_deref())
            .and_then(Path::parent)
            .unwrap_or(project);
        std::fs::create_dir_all(&git_dir)
            .map_err(|error| CheckpointError::GitMissing(error.to_string()))?;
        let old = checkpoint_root.join(project_key(project));
        let index = git_dir.join(format!("index-{}", project_key(project)));
        let checkpoints = Self {
            serial: serial_for(&index),
            legacy: (old != git_dir && old.join("HEAD").exists()).then_some(old),
            index: Some(index),
            work_tree: project.to_path_buf(),
            staged: staged_for(checkpoint_root, project),
            git_dir,
            until: None,
        };
        // Incident shape: first opens of one new store raced on `config.lock`, and the shadow
        // stayed `None` for the session.
        let creating = serial_for(&checkpoints.git_dir);
        let _creating = creating.lock().unwrap_or_else(PoisonError::into_inner);
        if !checkpoints.git_dir.join("HEAD").exists() {
            checkpoints.init(main_tree)?;
        }
        checkpoints.sync_exclude(common.as_deref());
        Ok(checkpoints)
    }

    /// The store that holds `tree`: this repository's, else the per-path one an older session
    /// wrote, which then serves the whole operation.
    pub fn open_holding(
        checkpoint_root: &Path,
        project: &Path,
        tree: &TreeId,
    ) -> Result<Self, CheckpointError> {
        let shared = Self::open(checkpoint_root, project)?;
        match &shared.legacy {
            Some(old) if !shared.holds(tree) => Ok(Self {
                serial: serial_for(old),
                git_dir: old.clone(),
                work_tree: project.to_path_buf(),
                staged: staged_for(checkpoint_root, project),
                index: None,
                legacy: None,
                until: None,
            }),
            _ => Ok(shared),
        }
    }

    #[must_use]
    pub fn holds(&self, tree: &TreeId) -> bool {
        self.git(&["cat-file", "-e", tree.as_str()]).is_ok()
    }

    fn init(&self, main_tree: &Path) -> Result<(), CheckpointError> {
        let output = host_git(main_tree)?
            .arg("--git-dir")
            .arg(&self.git_dir)
            .arg("--work-tree")
            .arg(main_tree)
            .args(["init", "--quiet"])
            .current_dir(main_tree)
            .output()
            .map_err(|error| CheckpointError::GitMissing(error.to_string()))?;
        if output.status.success() {
            return Ok(());
        }
        Err(CheckpointError::Git {
            command: "init".to_owned(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }

    /// The project's `info/exclude` is a repository's own ignore list, and a shadow has its own
    /// `info`. Copied at open only when the bytes differ, so an edit reaches the next session.
    fn sync_exclude(&self, common: Option<&Path>) {
        let source = common.map(|common| common.join("info/exclude"));
        let wanted = source.and_then(|source| std::fs::read(source).ok());
        let target = self.git_dir.join("info/exclude");
        if let Some(wanted) =
            wanted.filter(|bytes| std::fs::read(&target).ok().as_ref() != Some(bytes))
        {
            let _no_list_only_narrows_nothing = std::fs::create_dir_all(self.git_dir.join("info"))
                .and_then(|()| std::fs::write(target, wanted));
        }
    }

    /// Honours the project's own ignore rules, read from the work tree.
    pub fn capture(&self) -> Result<TreeId, CheckpointError> {
        self.stage()?;
        self.write_tree()
    }

    /// [`Self::capture`] that kills its git past `within`, then clears the lock it left. A second
    /// process capturing this tree sees a failed capture, never a corrupt index.
    pub fn capture_within(&self, within: std::time::Duration) -> Result<TreeId, CheckpointError> {
        let Some(until) = std::time::Instant::now().checked_add(within) else {
            return self.capture();
        };
        let bound = Self {
            git_dir: self.git_dir.clone(),
            work_tree: self.work_tree.clone(),
            serial: Arc::clone(&self.serial),
            staged: self.staged.clone(),
            index: self.index.clone(),
            legacy: None,
            until: Some(until),
        };
        bound.capture()
    }

    /// A handle whose git uses an index of its own, so a background capture never queues a turn's
    /// behind it; a store from before one was shared keeps the single index it has.
    #[must_use]
    pub fn own_index(&self) -> Self {
        let index = self
            .index
            .as_ref()
            .map(|index| index.with_extension("call"));
        Self {
            serial: index
                .as_ref()
                .map_or_else(|| Arc::clone(&self.serial), |index| serial_for(index)),
            git_dir: self.git_dir.clone(),
            work_tree: self.work_tree.clone(),
            staged: self.staged.clone(),
            index,
            legacy: None,
            until: None,
        }
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
            let mut process = host_git(&self.work_tree)?;
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
        let mut process = host_git(&self.work_tree)?;
        process
            .arg("--git-dir")
            .arg(&self.git_dir)
            .arg("--work-tree")
            .arg(&self.work_tree)
            .args(args)
            .current_dir(&self.work_tree);
        // Invariant: in the store's root, so its `.lock` still moves the dir mtime retention reads.
        if let Some(index) = &self.index {
            process.env("GIT_INDEX_FILE", index);
        }
        let index = (self.index.clone()).unwrap_or_else(|| self.git_dir.join("index"));
        let lock = lock_for(&index);
        let not_ours = lock.exists();
        let until = self.until;
        let past: crate::tool::CancelFlag =
            Arc::new(move || until.is_some_and(|until| std::time::Instant::now() >= until));
        let capture =
            crate::process::run_captured(process, stdin.map(<[u8]>::to_vec), &past, usize::MAX)
                .map_err(CheckpointError::GitMissing)?;
        if capture.cancelled {
            // Invariant: git's lock is the index path with `.lock` appended, and a lock there before the spawn is another's.
            if !not_ours {
                let _no_lock_is_fine = std::fs::remove_file(&lock);
            }
            return Err(CheckpointError::Deadline {
                command: verb.to_owned(),
            });
        }
        if capture.exit_code != Some(0) {
            return Err(CheckpointError::Git {
                command: verb.to_owned(),
                message: capture.stderr.trim().to_owned(),
            });
        }
        Ok(capture.stdout)
    }
}

/// The lock git takes for `index`: `.lock` appended to its name, never a replaced extension.
fn lock_for(index: &Path) -> PathBuf {
    let mut name = index.as_os_str().to_os_string();
    name.push(".lock");
    PathBuf::from(name)
}
fn host_git(cwd: &Path) -> Result<std::process::Command, CheckpointError> {
    let mut process = crate::process::host_command("git", cwd)
        .map_err(|error| CheckpointError::GitMissing(error.to_string()))?;
    process.args(["-c", "core.quotepath=false"]);
    Ok(process)
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

/// A worktree's index left behind once the tree is gone: only a cache of what the next capture
/// would stat again, so it goes with no effect but one slow capture.
pub fn drop_old_indexes(git_dir: &Path, older_than: std::time::Duration) {
    let entries = std::fs::read_dir(git_dir).into_iter().flatten().flatten();
    for entry in entries.filter(|entry| entry.file_name().to_string_lossy().starts_with("index-")) {
        let age = entry.metadata().and_then(|meta| meta.modified()).ok();
        if age
            .and_then(|at| at.elapsed().ok())
            .is_some_and(|age| age > older_than)
        {
            let _gone_already_is_fine = std::fs::remove_file(entry.path());
        }
    }
}

/// Invariant: captures write no ref, so every object is unreachable and `prune` keeps what is
/// younger than `keep` plus whatever a younger tree still names.
pub fn prune(git_dir: &Path, keep: std::time::Duration) -> Result<(), CheckpointError> {
    let output = host_git(git_dir)?
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
