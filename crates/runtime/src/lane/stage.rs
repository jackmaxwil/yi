//! A kernel child's hand-back merged in the object store: no checkout, so no slot per merge. The
//! plan engine stays on [`super::settle::prepare`], since it verifies in that directory.

use std::path::Path;
use std::sync::OnceLock;
use std::time::Instant;

use super::hostgit::{git, git_by, identity, past, sha_of};
use super::settle::{Conflict, Generation, Prepared, Staging, prepare};
use super::{LaneError, Pool, Sha};

/// Incident, from git's release notes: `--attr-source` "used to segfault", fixed in 2.43. The fold
/// and the hand-back both pass it, so both take the old road below this one floor.
pub const ATTR_SOURCE_SINCE: (u32, u32) = (2, 43);

/// `git version 2.54.0 (Apple Git-157)` to `(2, 54)`.
#[must_use]
pub fn git_version(text: &str) -> Option<(u32, u32)> {
    let mut parts = text.trim().strip_prefix("git version ")?.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// Whether this git folds two snapshots: no checkout exists to fall back to below 2.43.
#[must_use]
pub fn folds(text: &str) -> bool {
    git_version(text).is_some_and(|at| at >= ATTR_SOURCE_SINCE)
}

fn native(parent: &Path) -> bool {
    static NEW_ENOUGH: OnceLock<bool> = OnceLock::new();
    *NEW_ENOUGH.get_or_init(|| {
        let version = git(parent, &["--version"]).ok();
        version
            .as_deref()
            .and_then(git_version)
            .is_some_and(|at| at >= ATTR_SOURCE_SINCE)
    })
}

/// A conflicted `merge-tree --write-tree -z`: the tree, then `mode oid stage\tpath` rows up to an
/// empty field, then messages. `None` on any other shape.
#[must_use]
pub fn merge_tree_rows(stdout: &str) -> Option<Vec<(String, u8)>> {
    let mut fields = stdout.split('\0');
    let _tree = fields.next().filter(|tree| !tree.is_empty())?;
    let mut rows = Vec::new();
    for field in fields.take_while(|field| !field.is_empty()) {
        let (meta, path) = field.split_once('\t')?;
        rows.push((path.to_owned(), meta.rsplit(' ').next()?.parse().ok()?));
    }
    Some(rows)
}

fn conflicts(rows: &[(String, u8)], base: &Sha) -> Vec<Conflict> {
    let mut paths: Vec<&str> = rows.iter().map(|(path, _)| path.as_str()).collect();
    paths.dedup();
    let has = |at: &str, stage: u8| rows.iter().any(|(path, row)| path == at && *row == stage);
    (paths.into_iter())
        .map(|path| Conflict {
            path: path.to_owned(),
            side: match (has(path, 2), has(path, 3)) {
                (true, true) => "both",
                (true, false) => "ours",
                (false, true) => "theirs",
                (false, false) => "unknown",
            }
            .to_owned(),
            base: base.as_str().to_owned(),
        })
        .collect()
}

/// `candidate` onto `generation`, the same answer [`prepare`] gives. Exit 1 is a conflict and any
/// other failure an error, so a broken git never reads as a clean merge or as a conflict.
pub fn merged(
    pool: &Pool,
    parent: &Path,
    session: &str,
    generation: &Generation,
    candidate: &Sha,
    deadline: Option<Instant>,
) -> Result<Prepared, LaneError> {
    past(deadline)?;
    if !native(parent) {
        return prepare(pool, session, generation, candidate, deadline);
    }
    let base = generation.base.as_str();
    let source = format!("--attr-source={base}");
    let args = [
        source.as_str(),
        "merge-tree",
        "--write-tree",
        "-z",
        base,
        candidate.as_str(),
    ];
    let tree = match git_by(parent, &args, deadline) {
        Ok(stdout) => stdout,
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
            let found = git_by(parent, &["merge-base", base, candidate.as_str()], deadline);
            let merge_base = found.ok().and_then(|text| Sha::parse(&text));
            let merge_base = merge_base.unwrap_or_else(|| generation.base.clone());
            return Ok(Prepared::Conflict {
                base: generation.base.clone(),
                conflicts: conflicts(&rows, &merge_base),
            });
        }
        Err(error) => return Err(error),
    };
    let tree = tree.split('\0').next().unwrap_or_default().trim();
    let message = format!(
        "Integrate {} onto {}",
        candidate.short(),
        generation.base.short()
    );
    let commit = [
        "commit-tree",
        tree,
        "-p",
        base,
        "-p",
        candidate.as_str(),
        "-m",
        &message,
    ];
    let integrated = git_by(
        parent,
        &[&identity(parent)[..], &commit[..]].concat(),
        deadline,
    )?;
    Ok(Prepared::Merged(Staging::laneless(
        generation.base.clone(),
        candidate.clone(),
        sha_of(&integrated)?,
    )))
}
