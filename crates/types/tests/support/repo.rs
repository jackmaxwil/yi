//! A minimal git repo for tests: `main` checked out, one commit, no dependence on
//! global git config. Shared by `#[path]` includes from other crates' test files.

use std::{error::Error, path::Path};

pub fn init_repo(repo: &Path) -> Result<(), Box<dyn Error>> {
    std::fs::create_dir_all(repo)?;
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.email", "lanes@test"][..],
        &["config", "user.name", "lanes"][..],
    ] {
        git(repo, args)?;
    }
    std::fs::write(repo.join("README.md"), "base\n")?;
    for args in [&["add", "README.md"][..], &["commit", "-qm", "base"][..]] {
        git(repo, args)?;
    }
    Ok(())
}

fn git(repo: &Path, args: &[&str]) -> Result<(), Box<dyn Error>> {
    #[expect(clippy::disallowed_methods, reason = "real git is the fixture builder")]
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()?;
    if !out.status.success() {
        return Err(format!("git {args:?}: {}", String::from_utf8_lossy(&out.stderr)).into());
    }
    Ok(())
}
