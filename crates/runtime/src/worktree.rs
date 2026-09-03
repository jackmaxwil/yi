use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::subagent::{ChildStatus, SubagentHost};

const GIT_TIMEOUT_MS: u64 = 120_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
}

fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    capture(
        cwd,
        "git",
        args,
        std::time::Duration::from_millis(GIT_TIMEOUT_MS),
    )
}

pub(crate) fn capture(
    cwd: &Path,
    program: &str,
    args: &[&str],
    deadline: std::time::Duration,
) -> Result<String, String> {
    let mut command = yi_tools::command(program);
    command.current_dir(cwd).args(args);
    let deadline = std::time::Instant::now()
        .checked_add(deadline)
        .unwrap_or_else(std::time::Instant::now);
    let cancelled: yi_tools::CancelFlag = Arc::new(move || std::time::Instant::now() >= deadline);
    let capture = yi_tools::run_captured(command, None, &cancelled, 30_000)
        .map_err(|error| format!("{program} {}: {error}", args.join(" ")))?;
    if capture.exit_code == Some(0) {
        return Ok(capture.stdout);
    }
    Err(format!(
        "{program} {} failed:\n{}{}",
        args.join(" "),
        capture.stdout,
        capture.stderr
    ))
}

/// Design B11: one branch off the parent's HEAD per isolated child.
pub fn create(repo: &Path, artifacts: &Path, child_id: &str) -> Result<Worktree, String> {
    let branch = format!("yi/{child_id}");
    let path = artifacts.join(format!("wt-{child_id}"));
    let path_text = path.to_string_lossy().into_owned();
    git(
        repo,
        &["worktree", "add", "-b", &branch, &path_text, "HEAD"],
    )?;
    Ok(Worktree { path, branch })
}

/// A child leaves its work uncommitted, so the branch is empty until this
/// commits it; without that step a merge would silently bring nothing over.
pub fn merge(repo: &Path, tree: &Worktree, label: &str) -> Result<String, String> {
    let status = git(&tree.path, &["status", "--porcelain"])?;
    if !status.trim().is_empty() {
        git(&tree.path, &["add", "-A"])?;
        git(
            &tree.path,
            &["commit", "-m", &format!("subagent {label} work")],
        )?;
    }
    git(repo, &["merge", "--no-ff", "--no-edit", &tree.branch])
}

pub fn discard(repo: &Path, tree: &Worktree) -> Result<(), String> {
    let path_text = tree.path.to_string_lossy().into_owned();
    git(repo, &["worktree", "remove", "--force", &path_text])?;
    git(repo, &["branch", "-D", &tree.branch])?;
    Ok(())
}

impl SubagentHost {
    /// B11 hand-back: the child's branch is committed, then merged.
    pub fn merge_worktree(&self, target: &str) -> Result<Map<String, Value>, String> {
        let (tree, name) = self.take_settled_worktree(target)?;
        let output = crate::worktree::merge(&self.options.cwd, &tree, &name)?;
        crate::worktree::discard(&self.options.cwd, &tree)?;
        let mut reply = Map::new();
        reply.insert("merged".to_owned(), Value::Bool(true));
        reply.insert("branch".to_owned(), Value::String(tree.branch));
        reply.insert("output".to_owned(), Value::String(output));
        Ok(reply)
    }

    pub fn discard_worktree(&self, target: &str) -> Result<Map<String, Value>, String> {
        let (tree, _) = self.take_settled_worktree(target)?;
        crate::worktree::discard(&self.options.cwd, &tree)?;
        let mut reply = Map::new();
        reply.insert("discarded".to_owned(), Value::Bool(true));
        reply.insert("branch".to_owned(), Value::String(tree.branch));
        Ok(reply)
    }

    /// Invariant: taken off the record, so a tree is never handed back twice.
    fn take_settled_worktree(
        &self,
        target: &str,
    ) -> Result<(crate::worktree::Worktree, String), String> {
        let mut children = self
            .children
            .lock()
            .map_err(|_| "subagent state poisoned")?;
        let key = Self::key_of(&children, target)?;
        let record = children
            .get_mut(&key)
            .ok_or_else(|| format!("No RLM child matches \"{target}\""))?;
        if record.status == ChildStatus::Running {
            return Err(format!(
                "child \"{target}\" is still running; wait for it before touching its worktree"
            ));
        }
        let tree = record
            .worktree
            .take()
            .ok_or_else(|| format!("child \"{target}\" has no worktree (isolation was none)"))?;
        Ok((tree, record.session_name.clone()))
    }
}
