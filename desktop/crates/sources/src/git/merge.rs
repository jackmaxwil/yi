//! Bringing a Yi checkout's branch into the project: what a merge would take, and the merge.

use std::path::{Path, PathBuf};

use yd_core::row::Row;

use super::{LANE, base, git, parse_status};

/// What a merge of the checkout at `root` would bring into the project's own folder.
#[derive(Debug, PartialEq, Eq)]
struct Plan {
    home: PathBuf,
    branch: String,
    base: String,
    commits: usize,
    files: usize,
}

/// The last merge asked for: how many were asked, and what came of the newest.
#[derive(Debug, Default)]
pub(crate) struct Said {
    done: u64,
    words: String,
}

/// Why a checkout whose work is all uncommitted has nothing to merge, in a person's words.
const UNCOMMITTED: &str =
    "The agent's changes are not committed yet. Ask it to commit, then merge.";

/// The folder of the repository's main checkout: the first `git worktree list` names.
fn home(root: &Path) -> Option<PathBuf> {
    let out = git(root, &["worktree", "list", "--porcelain", "-z"], b"").ok()?;
    let text = String::from_utf8_lossy(&out);
    let first = text.split('\0').next()?.strip_prefix("worktree ")?;
    Some(PathBuf::from(first))
}

/// The checkout holds work not yet committed, new files included.
fn uncommitted(root: &Path) -> bool {
    let status = count(root, &["status", "--porcelain=v2", "-z"]);
    status.is_ok_and(|status| !parse_status(&status).1.is_empty())
}

/// `n` and the noun, which takes an `s` past one.
fn some(n: usize, noun: &str) -> String {
    let s = if n == 1 { "" } else { "s" };
    format!("{n} {noun}{s}")
}

fn count(root: &Path, args: &[&str]) -> Result<String, String> {
    git(root, args, b"")
        .map(|out| String::from_utf8_lossy(&out).into_owned())
        .map_err(|why| why.to_string())
}

/// The merge of `branch` into `base`, or why the project folder must not take it now.
fn plan(root: &Path, branch: &str, base: &str) -> Result<Plan, String> {
    let home = home(root).ok_or("the project folder of this checkout was not found")?;
    let status = [
        "status",
        "--porcelain=v2",
        "--branch",
        "-z",
        "--untracked-files=no",
    ];
    let status = count(&home, &status)?;
    let (head, changed) = parse_status(&status);
    let on = head.words("branch").unwrap_or_default();
    if on != base {
        return Err(format!(
            "The project folder is on {on}, not {base}: nothing was merged."
        ));
    }
    if !changed.is_empty() {
        let n = some(changed.len(), "uncommitted change");
        return Err(format!(
            "The project folder has {n}: commit or stash them first."
        ));
    }
    let commits = count(
        root,
        &["rev-list", "--count", &[base, "..", branch].concat()],
    )?;
    let commits = commits.trim().parse::<usize>().unwrap_or(0);
    if commits == 0 && uncommitted(root) {
        return Err(UNCOMMITTED.to_owned());
    }
    if commits == 0 {
        return Err(format!(
            "{branch} has no commit that {base} lacks: nothing to merge."
        ));
    }
    let files = count(
        root,
        &["diff", "--name-only", &[base, "...", branch].concat()],
    )?;
    Ok(Plan {
        home,
        branch: branch.to_owned(),
        base: base.to_owned(),
        commits,
        files: files.lines().count(),
    })
}

/// The `merge` row of a Yi checkout: what a merge would take (`branch`, `base`, `home`,
/// `commits`, `files`) or why not (`refused`), and the last merge's `said` and `done`.
pub(crate) fn row(root: &Path, repo: Option<&Row>, said: &Said) -> Option<Row> {
    let branch = repo?.words("branch").filter(|b| b.starts_with(LANE))?;
    let base = repo?.words("base")?;
    let row = Row::new("merge")
        .text("branch", branch)
        .text("base", base)
        .text("said", said.words.as_str())
        .text("done", said.done.to_string());
    Some(match plan(root, branch, base) {
        Ok(plan) => row
            .text("home", plan.home.to_string_lossy())
            .text("commits", plan.commits.to_string())
            .text("files", plan.files.to_string()),
        Err(why) => {
            let asks = why == UNCOMMITTED;
            row.text("refused", why)
                .text("uncommitted", if asks { "1" } else { "0" })
        }
    })
}

/// Merges the checkout's branch into the project folder with `git merge --no-ff`, checked
/// again first; a conflict is left as git left it. Never a push, never another folder.
pub(crate) fn merge(root: &Path, said: &mut Said) {
    said.done = said.done.saturating_add(1);
    said.words = match run(root) {
        Ok(words) | Err(words) => words,
    };
}

fn run(root: &Path) -> Result<String, String> {
    let branch = count(root, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let branch = branch.trim();
    let base = base(root).ok_or("this repository has no base branch")?;
    if !branch.starts_with(LANE) {
        return Err(format!(
            "{branch} is not a Yi checkout's branch: nothing was merged."
        ));
    }
    let plan = plan(root, branch, &base)?;
    let merged = git(
        &plan.home,
        &["merge", "--no-ff", "--no-edit", &plan.branch],
        b"",
    );
    let (n, home) = (some(plan.files, "file"), plan.home.display());
    let Err(why) = merged else {
        return Ok(format!("Merged {branch} into {base} in {home}: {n}."));
    };
    let unmerged = count(&plan.home, &["diff", "--name-only", "--diff-filter=U"]);
    let unmerged = unmerged.unwrap_or_default();
    let names: Vec<&str> = unmerged.lines().collect();
    if names.is_empty() {
        return Err(format!(
            "git could not merge {branch}: {why}. {home} is as git left it."
        ));
    }
    let names = names.join(", ");
    Err(format!(
        "The merge stopped on a conflict in {names}; {home} is left as git left it."
    ))
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;
    use crate::fs::tests::{scratch, sh};

    fn commit(at: &Path, words: &str) -> Result<(), Box<dyn Error>> {
        sh(at, &["add", "-A"])?;
        sh(at, &["commit", "-qm", words])
    }

    /// A project on `main` with one file, and Yi's checkout of it on `yi/0f3e`.
    fn project(name: &str) -> Result<(PathBuf, PathBuf), Box<dyn Error>> {
        let home = scratch(name)?;
        sh(&home, &["init", "-q", "-b", "main"])?;
        sh(&home, &["config", "user.name", "t"])?;
        sh(&home, &["config", "user.email", "t@t"])?;
        std::fs::write(home.join("a.rs"), "one\n")?;
        commit(&home, "c")?;
        let lane = scratch(&[name, "-lane"].concat())?.join("0");
        let text = lane.to_str().ok_or("a path that is not text")?;
        sh(&home, &["worktree", "add", "-q", "-b", "yi/0f3e", text])?;
        Ok((home, lane))
    }

    fn said(lane: &Path) -> String {
        let mut said = Said::default();
        merge(lane, &mut said);
        said.words
    }

    fn on_main(home: &Path, path: &str) -> Result<String, Box<dyn Error>> {
        Ok(count(home, &["show", &["main:", path].concat()])?)
    }

    #[test]
    #[cfg_attr(
        not(target_os = "macos"),
        ignore = "needs git 2.36 for `worktree list -z`, and the forge image has 2.34 (#2341)"
    )]
    fn a_checkouts_commits_merge_into_the_project_folder_once_asked() -> Result<(), Box<dyn Error>>
    {
        let (home, lane) = project("merge")?;
        std::fs::write(lane.join("by.rs"), "fn by() {}\n")?;
        commit(&lane, "e")?;
        let words = said(&lane);
        assert!(words.starts_with("Merged yi/0f3e into main in "), "{words}");
        assert!(words.ends_with(": 1 file."), "{words}");
        assert_eq!(on_main(&home, "by.rs")?, "fn by() {}\n");
        let parents = count(&home, &["rev-list", "--parents", "-n", "1", "main"])?;
        assert_eq!(
            parents.split_whitespace().count(),
            3,
            "a merge commit: {parents}"
        );
        let again = said(&lane);
        assert_eq!(
            again,
            "yi/0f3e has no commit that main lacks: nothing to merge."
        );
        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(target_os = "macos"),
        ignore = "needs git 2.36 for `worktree list -z`, and the forge image has 2.34 (#2341)"
    )]
    fn a_project_folder_with_edits_or_on_another_branch_takes_no_merge()
    -> Result<(), Box<dyn Error>> {
        let (home, lane) = project("merge-dirty")?;
        std::fs::write(lane.join("by.rs"), "fn by() {}\n")?;
        commit(&lane, "e")?;
        std::fs::write(home.join("a.rs"), "edited by hand\n")?;
        let words = said(&lane);
        assert_eq!(
            words,
            "The project folder has 1 uncommitted change: commit or stash them first."
        );
        assert!(on_main(&home, "by.rs").is_err(), "nothing merged");
        sh(&home, &["checkout", "-q", "a.rs"])?;
        sh(&home, &["checkout", "-q", "-b", "spike"])?;
        let words = said(&lane);
        assert_eq!(
            words,
            "The project folder is on spike, not main: nothing was merged."
        );
        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(target_os = "macos"),
        ignore = "needs git 2.36 for `worktree list -z`, and the forge image has 2.34 (#2341)"
    )]
    fn a_conflict_is_left_as_git_left_it_and_says_where() -> Result<(), Box<dyn Error>> {
        let (home, lane) = project("merge-conflict")?;
        std::fs::write(lane.join("a.rs"), "lane\n")?;
        commit(&lane, "e")?;
        std::fs::write(home.join("a.rs"), "home\n")?;
        commit(&home, "h")?;
        let words = said(&lane);
        assert!(
            words.starts_with("The merge stopped on a conflict in a.rs; "),
            "{words}"
        );
        assert!(
            home.join(".git/MERGE_HEAD").exists(),
            "git's own state is kept"
        );
        Ok(())
    }

    /// Dogfood: "has no commit that main lacks: nothing to merge" while Changes listed the
    /// agent's files, which it had not committed.
    #[test]
    #[cfg_attr(
        not(target_os = "macos"),
        ignore = "needs git 2.36 for `worktree list -z`, and the forge image has 2.34 (#2341)"
    )]
    fn uncommitted_work_in_the_checkout_is_named_as_the_reason() -> Result<(), Box<dyn Error>> {
        let (_, lane) = project("merge-uncommitted")?;
        std::fs::write(lane.join("by.rs"), "fn by() {}\n")?;
        let rows = super::super::snapshot(&lane)?;
        let row = row(&lane, rows.first(), &Said::default()).ok_or("no merge row")?;
        assert_eq!(row.words("refused"), Some(UNCOMMITTED));
        assert_eq!(
            row.words("uncommitted"),
            Some("1"),
            "the pane offers to ask"
        );
        assert_eq!(said(&lane), UNCOMMITTED);
        Ok(())
    }

    #[test]
    #[cfg_attr(
        not(target_os = "macos"),
        ignore = "needs git 2.36 for `worktree list -z`, and the forge image has 2.34 (#2341)"
    )]
    fn the_row_says_what_a_merge_would_take_before_it_is_asked() -> Result<(), Box<dyn Error>> {
        let (_, lane) = project("merge-row")?;
        std::fs::write(lane.join("by.rs"), "fn by() {}\n")?;
        std::fs::write(lane.join("a.rs"), "uno\n")?;
        commit(&lane, "e")?;
        let rows = super::super::snapshot(&lane)?;
        let row = row(&lane, rows.first(), &Said::default()).ok_or("no merge row")?;
        let cell = |name| row.words(name);
        assert_eq!(
            (cell("branch"), cell("base")),
            (Some("yi/0f3e"), Some("main"))
        );
        assert_eq!((cell("commits"), cell("files")), (Some("1"), Some("2")));
        assert_eq!(cell("refused"), None);
        Ok(())
    }
}
