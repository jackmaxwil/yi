use std::path::{Path, PathBuf};
use std::sync::Arc;

const GIT_TIMEOUT_MS: u64 = 120_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
}

fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = yi_tools::command("git");
    command.current_dir(cwd).args(args);
    let deadline = std::time::Instant::now()
        .checked_add(std::time::Duration::from_millis(GIT_TIMEOUT_MS))
        .unwrap_or_else(std::time::Instant::now);
    let cancelled: yi_tools::CancelFlag = Arc::new(move || std::time::Instant::now() >= deadline);
    let capture = yi_tools::run_captured(command, None, &cancelled, 30_000)
        .map_err(|error| format!("git {}: {error}", args.join(" ")))?;
    if capture.exit_code == Some(0) {
        return Ok(capture.stdout);
    }
    Err(format!(
        "git {} failed:\n{}{}",
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
