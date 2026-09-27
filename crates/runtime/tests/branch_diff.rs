//! What a lane's branch would land, as the console's Review pane asks the worker for it.
use std::error::Error;
use std::process::Command;

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

/// Dies without a base: the review had no view of what the branch would land.
#[test]
fn the_branch_diff_is_against_the_merge_base_with_main() -> TestResult {
    let dir = Scratch::new("yi-branch-diff")?;
    let root = dir.to_path_buf();
    let git = |args: &[&str]| -> TestResult {
        #[expect(
            clippy::disallowed_methods,
            reason = "the fixture is a real repository, built the way a lane's is"
        )]
        let status = Command::new("git")
            .arg("-C")
            .arg(&root)
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
    };
    git(&["init", "-q", "-b", "main"])?;
    std::fs::write(root.join("kept.rs"), "one\n")?;
    git(&["add", "."])?;
    git(&["commit", "-q", "-m", "base"])?;
    git(&["checkout", "-q", "-b", "lane"])?;
    std::fs::write(root.join("kept.rs"), "one\ntwo\n")?;
    git(&["commit", "-q", "-am", "work"])?;
    std::fs::write(root.join("loose.rs"), "new\n")?;
    let branch = yi_runtime::environment::branch_diff(&root).ok_or("no base")?;
    assert_eq!(branch.files, vec![("kept.rs".to_owned(), 1, 0)]);
    assert_eq!(branch.untracked, 1);
    assert!(branch.patch.contains("+two"), "{}", branch.patch);
    Ok(())
}
