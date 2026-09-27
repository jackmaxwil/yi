//! What a lane's branch would land, as the console's Review pane asks the worker for it.
use std::error::Error;
use std::process::Command;

use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

fn git(root: &std::path::Path, args: &[&str]) -> TestResult {
    #[expect(
        clippy::disallowed_methods,
        reason = "the fixture is a real repository, built the way a lane's is"
    )]
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("git {args:?} failed").into())
    }
}

/// Dies without a base: the review had no view of what the branch would land. Dies too
/// with an uncommitted edit shown as landing, when `/land` pushes commits only.
#[test]
fn the_branch_diff_is_against_the_merge_base_with_main() -> TestResult {
    let dir = Scratch::new("yi-branch-diff")?;
    let root = dir.to_path_buf();
    let git = |args: &[&str]| git(&root, args);
    git(&["init", "-q", "-b", "main"])?;
    std::fs::write(root.join("kept.rs"), "one\n")?;
    git(&["add", "."])?;
    git(&["commit", "-q", "-m", "base"])?;
    git(&["checkout", "-q", "-b", "lane"])?;
    std::fs::write(root.join("kept.rs"), "one\ntwo\n")?;
    git(&["commit", "-q", "-am", "work"])?;
    std::fs::write(root.join("kept.rs"), "one\ntwo\nthree\n")?;
    std::fs::write(root.join("loose.rs"), "new\n")?;
    let branch = yi_runtime::environment::branch_diff(&root).ok_or("no base")?;
    assert_eq!(branch.files, vec![("kept.rs".to_owned(), 1, 0)]);
    assert_eq!(branch.untracked, 1);
    assert!(branch.patch.contains("+two"), "{}", branch.patch);
    assert!(!branch.patch.contains("+three"), "{}", branch.patch);
    Ok(())
}

/// Dies with a big branch cut mid-hunk by a 30 KB capture, its middle silently gone; a patch
/// over the cap ends on a whole file and says so, at the cap and one byte under it.
#[test]
fn a_big_branch_diff_is_whole_and_a_cut_one_ends_on_a_file_and_says_so() -> TestResult {
    let dir = Scratch::new("yi-branch-diff-big")?;
    let root = dir.to_path_buf();
    let git = |args: &[&str]| git(&root, args);
    git(&["init", "-q", "-b", "main"])?;
    std::fs::write(root.join("seed.rs"), "seed\n")?;
    git(&["add", "."])?;
    git(&["commit", "-q", "-m", "base"])?;
    git(&["checkout", "-q", "-b", "lane"])?;
    let body: String = (0..2_000)
        .map(|line| format!("let row_{line} = \"é{line}\";\n"))
        .collect();
    for name in ["a.rs", "b.rs", "c.rs"] {
        std::fs::write(root.join(name), &body)?;
    }
    git(&["add", "."])?;
    git(&["commit", "-q", "-m", "work"])?;
    let whole = yi_runtime::environment::branch_diff(&root).ok_or("no base")?;
    assert_eq!(whole.files.len(), 3);
    assert!(whole.patch.len() > 60_000, "{}", whole.patch.len());
    assert_eq!(whole.patch.matches("diff --git ").count(), 3);
    assert!(!whole.patch.contains("omitted") && !whole.patch.contains("[…"));
    let at_cap =
        yi_runtime::environment::branch_diff_capped(&root, whole.patch.len()).ok_or("no base")?;
    assert_eq!(at_cap.patch, whole.patch);
    let cut = yi_runtime::environment::branch_diff_capped(&root, whole.patch.len() - 1)
        .ok_or("no base")?;
    let (kept, notice) = cut.patch.rsplit_once("[… ").ok_or("no cut row")?;
    assert!(whole.patch.starts_with(kept), "the cut keeps a prefix");
    assert!(
        whole
            .patch
            .get(kept.len()..)
            .is_some_and(|rest| rest.starts_with("diff --git "))
    );
    assert!(
        notice.starts_with(&format!(
            "2 of 3 files shown: the review's patch cap is {} bytes",
            whole.patch.len() - 1
        )),
        "{notice}"
    );
    Ok(())
}
