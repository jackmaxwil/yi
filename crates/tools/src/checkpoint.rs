use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use crate::process::command;

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
        let checkpoints = Self {
            serial: serial_for(&git_dir),
            git_dir,
            work_tree: project.to_path_buf(),
        };
        if !checkpoints.git_dir.join("HEAD").exists() {
            std::fs::create_dir_all(&checkpoints.git_dir)
                .map_err(|error| CheckpointError::GitMissing(error.to_string()))?;
            checkpoints.git(&["init", "--quiet"])?;
        }
        Ok(checkpoints)
    }

    /// Honours the project's own ignore rules, read from the work tree.
    pub fn capture(&self) -> Result<TreeId, CheckpointError> {
        self.git(&["add", "--all"])?;
        let tree = self.git(&["write-tree"])?;
        Ok(TreeId::new(tree.trim()))
    }

    /// A capture with `excluded` (paths relative to the project) left out of the tree; the
    /// next `add --all` puts them back in the shadow index.
    pub fn capture_excluding(&self, excluded: &[&Path]) -> Result<TreeId, CheckpointError> {
        self.git(&["add", "--all"])?;
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
        let tree = self.git(&["write-tree"])?;
        Ok(TreeId::new(tree.trim()))
    }

    pub fn changed(&self, tree: &TreeId) -> Result<Vec<Change>, CheckpointError> {
        self.git(&["add", "--all"])?;
        let diff = self.git(&[
            "diff",
            "--no-renames",
            "--name-status",
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
        for change in &changes {
            let path = change.path.to_string_lossy().into_owned();
            match change.kind {
                ChangeKind::Restored => {
                    self.git(&["checkout", tree.as_str(), "--", &path])?;
                }
                ChangeKind::Deleted => {
                    let _absent_is_the_goal = std::fs::remove_file(self.work_tree.join(&path));
                }
                ChangeKind::Kept => {}
            }
        }
        self.git(&["add", "--all"])?;
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
            let mut process = command("git");
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

    fn git(&self, args: &[&str]) -> Result<String, CheckpointError> {
        let _serialized = self
            .serial
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut process = command("git");
        process
            .arg("--git-dir")
            .arg(&self.git_dir)
            .arg("--work-tree")
            .arg(&self.work_tree)
            .args(args)
            .current_dir(&self.work_tree);
        let output = process
            .output()
            .map_err(|error| CheckpointError::GitMissing(error.to_string()))?;
        if !output.status.success() {
            return Err(CheckpointError::Git {
                command: args.first().copied().unwrap_or_default().to_owned(),
                message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

/// Read relative to the earlier tree, with renames split by `--no-renames` (an `R` row named
/// a path the tree lacks): `A` is a path the turn added, everything else one to check out.
fn parse_changes(diff: &str) -> Vec<Change> {
    let mut changes = Vec::new();
    for line in diff.lines() {
        let mut fields = line.split('\t');
        let Some(status) = fields.next() else {
            continue;
        };
        let Some(path) = fields.next_back() else {
            continue;
        };
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

/// Hashed so the directory name cannot collide with a path component.
fn project_key(project: &Path) -> String {
    let hash = xxhash_rust::xxh32::xxh32(project.to_string_lossy().as_bytes(), 0);
    format!("{hash:08x}")
}
