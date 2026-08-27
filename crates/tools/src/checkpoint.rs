use std::path::{Path, PathBuf};
use std::sync::Mutex;

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
    // One shadow index, so two concurrent captures would race on index.lock.
    serial: Mutex<()>,
}

impl Checkpoints {
    pub fn open(checkpoint_root: &Path, project: &Path) -> Result<Self, CheckpointError> {
        let git_dir = checkpoint_root.join(project_key(project));
        let checkpoints = Self {
            git_dir,
            work_tree: project.to_path_buf(),
            serial: Mutex::new(()),
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

    pub fn changed(&self, tree: &TreeId) -> Result<Vec<Change>, CheckpointError> {
        self.git(&["add", "--all"])?;
        let diff = self.git(&["diff", "--name-status", "--cached", tree.as_str()])?;
        Ok(parse_changes(&diff))
    }

    /// Restores what changed or vanished, removes what the turn created.
    pub fn restore(&self, tree: &TreeId) -> Result<Vec<Change>, CheckpointError> {
        let changes = self.changed(tree)?;
        for change in &changes {
            let path = change.path.to_string_lossy().into_owned();
            match change.kind {
                ChangeKind::Restored => {
                    self.git(&["checkout", tree.as_str(), "--", &path])?;
                }
                ChangeKind::Deleted => {
                    let _absent_is_the_goal = std::fs::remove_file(self.work_tree.join(&path));
                }
            }
        }
        self.git(&["add", "--all"])?;
        Ok(changes)
    }

    pub fn diff(&self, from: &TreeId, to: &TreeId) -> Result<crate::GitPatch, CheckpointError> {
        let text = self.git(&["diff", from.as_str(), to.as_str()])?;
        Ok(crate::diff::GitPatch::from_text(text))
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

/// `--cached <tree>` reads "index relative to tree": `A` is a path the turn
/// added, `D` one it removed, everything else a content change.
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
