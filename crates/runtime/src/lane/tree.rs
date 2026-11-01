//! What the kernel's `Tree` does to a tree, in git: a snapshot names any state, so a diff, a merge
//! and a rewind all work on uncommitted files without touching the child's branch or staging.

use std::path::Path;

use super::hostgit::{git, git_by};
use super::stage::{git_version, merge_tree_rows};
use super::warm::snapshot;
use super::{LaneError, Sha};

/// `git merge-tree --merge-base` and `--attr-source` together: older gits have no road here, since a
/// fold over uncommitted trees has no checkout to fall back to.
const FOLD_SINCE: (u32, u32) = (2, 43);

/// `+added -deleted path`, as `git diff --numstat` counts them, from `against` to the tree's files.
pub fn diff_rows(tree: &Path, against: &Sha) -> Result<Vec<String>, LaneError> {
    let now = snapshot(tree)?;
    let numstat = git(tree, &["diff", "--numstat", against.as_str(), now.as_str()])?;
    Ok((numstat.lines())
        .filter_map(|row| {
            let mut parts = row.splitn(3, '\t');
            Some(format!(
                "+{} -{} {}",
                parts.next()?,
                parts.next()?,
                parts.next()?
            ))
        })
        .collect())
}

/// One file's patch from `against`, whole; the caller cuts it.
pub fn patch(tree: &Path, against: &Sha, path: &str) -> Result<String, LaneError> {
    let now = snapshot(tree)?;
    git(tree, &["diff", against.as_str(), now.as_str(), "--", path])
}

/// The paths a merge into `parent`'s `HEAD` would conflict on. `None` when the trial itself failed,
/// so an unknown cannot read as no conflict.
#[must_use]
pub fn would_conflict(tree: &Path, parent: &Path, base: &Sha) -> Option<Vec<String>> {
    let now = snapshot(tree).ok()?;
    let head = git(parent, &["rev-parse", "HEAD"]).ok()?;
    let merge_base = format!("--merge-base={}", base.as_str());
    let args = [
        "merge-tree",
        "--write-tree",
        "-z",
        &merge_base,
        head.trim(),
        now.as_str(),
    ];
    match git_by(parent, &args, None) {
        Ok(_) => Some(Vec::new()),
        Err(LaneError::Git {
            exit_code: Some(1),
            output,
            ..
        }) => {
            let mut paths: Vec<String> = merge_tree_rows(&output)?
                .into_iter()
                .map(|(path, _)| path)
                .collect();
            paths.dedup();
            Some(paths)
        }
        Err(_) => None,
    }
}

/// The tree's files become `to`'s, and the branch and index stay where they were. Incident: a file
/// new since `to` is in no index, so `read-tree` left it, and a rewind read as clean with it there.
pub fn rewind(tree: &Path, to: &Sha, paths: &[String]) -> Result<(), LaneError> {
    if let Some(escapes) = paths.iter().find(|path| !inside(path)) {
        return Err(LaneError::Forge(format!(
            "{escapes} is not a path inside the tree"
        )));
    }
    let now = snapshot(tree)?;
    let added = git(
        tree,
        &[
            "diff",
            "--name-only",
            "--no-renames",
            "--diff-filter=A",
            "-z",
            to.as_str(),
            now.as_str(),
        ],
    )?;
    let is_new = |path: &str| added.split('\0').any(|name| name == path);
    let chosen = |path: &str| paths.is_empty() || paths.iter().any(|named| named == path);
    for path in added
        .split('\0')
        .filter(|path| !path.is_empty() && chosen(path))
    {
        let at = tree.join(path);
        std::fs::remove_file(&at).map_err(super::io_error(&at))?;
    }
    if paths.is_empty() {
        git(tree, &["read-tree", "--reset", "-u", to.as_str()])?;
        return git(tree, &["reset", "-q", "--mixed"]).map(drop);
    }
    let kept: Vec<&str> = paths
        .iter()
        .map(String::as_str)
        .filter(|path| !is_new(path))
        .collect();
    if kept.is_empty() {
        return Ok(());
    }
    let source = format!("--source={}", to.as_str());
    let mut args = vec!["restore", source.as_str(), "--worktree", "--"];
    args.extend(kept);
    git(tree, &args).map(drop)
}

/// A kernel names these: relative, with no way up.
fn inside(path: &str) -> bool {
    use std::path::Component;
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
}

/// A file another side's merge conflicted on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fold {
    pub merged: bool,
    pub conflicts: Vec<String>,
}

/// `other`'s files merged into `ours`, in the store, over the two snapshots: no commit, and on a
/// conflict nothing moves. `other_base` is what `other` was cut from, so a sibling's shared history
/// counts once. Attributes are ours, as a checkout of ours would read them.
pub fn fold(ours: &Path, other: &Path, other_base: &Sha) -> Result<Fold, LaneError> {
    let version = git(ours, &["--version"])?;
    if git_version(&version).is_none_or(|at| at < FOLD_SINCE) {
        return Err(LaneError::Forge(format!(
            "merging trees needs git 2.43 and this one is {}",
            version.trim()
        )));
    }
    let (mine, theirs) = (snapshot(ours)?, snapshot(other)?);
    let source = format!("--attr-source={}", mine.as_str());
    let merge_base = format!("--merge-base={}", other_base.as_str());
    let args = [
        source.as_str(),
        "merge-tree",
        "--write-tree",
        "-z",
        &merge_base,
        mine.as_str(),
        theirs.as_str(),
    ];
    match git_by(ours, &args, None) {
        Ok(stdout) => {
            let tree = stdout
                .split('\0')
                .next()
                .unwrap_or_default()
                .trim()
                .to_owned();
            git(ours, &["read-tree", "--reset", "-u", &tree])?;
            git(ours, &["reset", "-q", "--mixed"])?;
            Ok(Fold {
                merged: true,
                conflicts: Vec::new(),
            })
        }
        Err(LaneError::Git {
            exit_code: Some(1),
            output,
            ..
        }) => {
            let rows = merge_tree_rows(&output).filter(|rows| !rows.is_empty());
            let Some(rows) = rows else {
                return Err(LaneError::Forge(format!(
                    "merge-tree said conflict and named none: {output}"
                )));
            };
            let mut conflicts: Vec<String> = rows.into_iter().map(|(path, _)| path).collect();
            conflicts.dedup();
            Ok(Fold {
                merged: false,
                conflicts,
            })
        }
        Err(error) => Err(error),
    }
}
