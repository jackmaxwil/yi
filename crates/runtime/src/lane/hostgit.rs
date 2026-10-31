//! Every git the host runs on a tree a contained process wrote: the one runner, the commit that
//! turns a child's work into a candidate, and which branch is trunk.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use yi_types::plan::acceptance::Quiescence;

use super::settle::{Candidate, git_by, quiet, sha_of};
use super::{BranchName, GIT_TIMEOUT_MS, LaneError, Sha};

/// Plumbing a caller parses is read whole: past this the runner errors rather than cut its middle.
pub(crate) const PLUMBING_CAP: usize = 8 << 20;

pub(crate) fn capture(
    cwd: &Path,
    program: &str,
    args: &[&str],
    deadline: std::time::Duration,
) -> Result<String, String> {
    let label = format!("{program} {}", args.join(" "));
    capture_named(
        cwd,
        &label,
        program,
        args,
        deadline,
        yi_tools::OUTPUT_CAP,
        None,
    )
    .map(|capture| capture.stdout)
}

/// Incident: the lane sync ran under the sandbox wrapper and its refusal quoted the wrapper's
/// whole argv, 17 KB of profile before the one line saying why; `label` is what the caller meant.
pub(crate) fn capture_named(
    cwd: &Path,
    label: &str,
    program: &str,
    args: &[&str],
    deadline: std::time::Duration,
    cap: usize,
    sandbox: Option<&yi_tools::Sandbox>,
) -> Result<yi_tools::CommandCapture, String> {
    let _span = yi_types::trace::span("lane.capture").arg("program", program);
    let mut command =
        yi_tools::host_command(program, cwd).map_err(|error| format!("{label}: {error}"))?;
    command.current_dir(cwd).args(args);
    let deadline = std::time::Instant::now()
        .checked_add(deadline)
        .unwrap_or_else(std::time::Instant::now);
    let cancelled: yi_tools::CancelFlag = Arc::new(move || std::time::Instant::now() >= deadline);
    let capture = yi_tools::run_captured(command, None, &cancelled, cap)
        .map_err(|error| format!("{label}: {error}"))?;
    if capture.exit_code == Some(0) {
        return Ok(capture);
    }
    let output = format!("{}{}", capture.stdout, capture.stderr);
    let denied = sandbox
        .and_then(|sandbox| {
            yi_tools::sandbox_refusal(sandbox, cwd, capture.exit_code, &output, label)
        })
        .map(|refusal| format!("\n{}\n", yi_tools::denial_hint(&refusal)))
        .unwrap_or_default();
    Err(format!("{label} failed:\n{output}{denied}"))
}

pub(crate) fn git(cwd: &Path, args: &[&str]) -> Result<String, LaneError> {
    whole(git_capture(cwd, args, PLUMBING_CAP)?, args, PLUMBING_CAP)
}

/// Incident: a long `status -z` lost its middle, a dirty file with it, and a restore overwrote
/// that file. A caller that parses plumbing reads all of it or gets this error.
pub(super) fn whole(
    capture: yi_tools::CommandCapture,
    args: &[&str],
    cap: usize,
) -> Result<String, LaneError> {
    match capture.cut {
        None => Ok(capture.stdout),
        Some((kept, total)) => Err(LaneError::Cut {
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            kept,
            total,
            cap,
        }),
    }
}

pub(crate) fn git_capture(
    cwd: &Path,
    args: &[&str],
    cap: usize,
) -> Result<yi_tools::CommandCapture, LaneError> {
    let _span = yi_types::trace::span("lane.git").arg("args", args.join(" "));
    let owned: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    let mut command = yi_tools::host_command("git", cwd).map_err(|error| LaneError::Git {
        args: owned.clone(),
        exit_code: None,
        output: error.to_string(),
    })?;
    command.current_dir(cwd).args(args);
    let deadline = std::time::Instant::now()
        .checked_add(std::time::Duration::from_millis(GIT_TIMEOUT_MS))
        .unwrap_or_else(std::time::Instant::now);
    let cancelled: yi_tools::CancelFlag = Arc::new(move || std::time::Instant::now() >= deadline);
    let capture =
        yi_tools::run_captured(command, None, &cancelled, cap).map_err(|error| LaneError::Git {
            args: owned.clone(),
            exit_code: None,
            output: error,
        })?;
    if capture.exit_code == Some(0) {
        return Ok(capture);
    }
    Err(LaneError::Git {
        args: owned,
        exit_code: capture.exit_code,
        output: format!("{}{}", capture.stdout, capture.stderr),
    })
}

/// Build output a child leaves untracked. Tracked changes under these names still commit, and only
/// `dist` and `build` at the top count: `src/build/` is somebody's source.
const NOT_WORK: [&str; 8] = [
    ":(exclude,glob)**/__pycache__/**",
    ":(exclude,glob)**/*.pyc",
    ":(exclude,glob)**/.pytest_cache/**",
    ":(exclude,glob)**/target/**",
    ":(exclude,glob)**/node_modules/**",
    ":(exclude,glob)**/.venv/**",
    ":(exclude,glob)dist/**",
    ":(exclude,glob)build/**",
];

/// Origin's `HEAD` when set, else `master` when only that exists, else `main`: from the ref files,
/// since a claim and every turn ask. ponytail: a reftable repository reads as `main`.
pub fn trunk(repo: &Path) -> BranchName {
    let Some(common) = yi_permission::git_dirs(repo).pop() else {
        return BranchName("main".to_owned());
    };
    let origin_head = std::fs::read_to_string(common.join("refs/remotes/origin/HEAD"));
    let named = origin_head.ok().and_then(|text| {
        let name = text
            .trim()
            .strip_prefix("ref: refs/remotes/origin/")?
            .to_owned();
        BranchName::parse(&name).ok()
    });
    if let Some(named) = named {
        return named;
    }
    let packed = std::fs::read_to_string(common.join("packed-refs")).unwrap_or_default();
    let exists =
        |name: &str| common.join(name).is_file() || packed.lines().any(|line| line.ends_with(name));
    let main = exists("refs/heads/main") || exists("refs/remotes/origin/main");
    let master = exists("refs/heads/master") || exists("refs/remotes/origin/master");
    BranchName(if master && !main { "master" } else { "main" }.to_owned())
}

/// `-c` for a repository with no identity: a host commit must not fail for want of a name.
pub(super) fn identity(tree: &Path) -> Vec<&'static str> {
    let named = git(tree, &["config", "user.email"]).is_ok_and(|email| !email.trim().is_empty());
    match named {
        true => Vec::new(),
        false => vec!["-c", "user.name=yi", "-c", "user.email=yi@localhost"],
    }
}

/// Invariant: a tree's admin `HEAD` is contained-writable, so a planted one would walk the host's
/// next commit or merge onto trunk or a sibling's branch; a detached `HEAD` commits to no branch.
pub(super) fn own_head(tree: &Path) -> Result<(), LaneError> {
    let head = match git(tree, &["symbolic-ref", "-q", "--short", "HEAD"]) {
        Ok(head) => head,
        Err(LaneError::Git {
            exit_code: Some(1), ..
        }) => return Ok(()),
        Err(error) => return Err(error),
    };
    let head = head.trim();
    let repo =
        super::canonical_repo(tree).ok_or_else(|| LaneError::NotARepo(tree.to_path_buf()))?;
    let here = tree.canonicalize().unwrap_or_else(|_| tree.to_path_buf());
    let listing = git(&repo, &["worktree", "list", "--porcelain"])?;
    let wanted = format!("branch refs/heads/{head}");
    let elsewhere = listing.split("\n\n").any(|block| {
        let path = block
            .lines()
            .find_map(|line| line.strip_prefix("worktree "));
        let other = path.is_some_and(|path| {
            Path::new(path)
                .canonicalize()
                .map_or(true, |path| path != here)
        });
        other && block.lines().any(|line| line == wanted)
    });
    if head == trunk(&repo).as_str() || elsewhere {
        return Err(LaneError::ForeignHead {
            head: head.to_owned(),
        });
    }
    Ok(())
}

/// Incident: uncommitted child work merged as nothing, so it is committed first. Free of the
/// `Lane`, so a host reads it with the path cloned and no lock held across the git calls.
pub fn candidate_of(
    path: &Path,
    session: &str,
    branch: &BranchName,
    base: Option<Sha>,
    quiescence: &Quiescence,
    deadline: Option<Instant>,
) -> Result<Candidate, LaneError> {
    quiet(quiescence)?;
    let status = git_by(path, &super::STATUS, deadline)?;
    if !status.trim().is_empty() {
        own_head(path)?;
        git_by(path, &["add", "-u"], deadline)?;
        git_by(
            path,
            &[&["add", "-A", "--", "."][..], &NOT_WORK].concat(),
            deadline,
        )?;
        let staged = git_by(path, &["diff", "--cached", "--name-only"], deadline)?;
        let message = format!("Save the work of session {session}");
        if !staged.trim().is_empty() {
            let commit = ["commit", "-q", "-m", &message];
            git_by(path, &[&identity(path)[..], &commit[..]].concat(), deadline)?;
        }
    }
    let commit = sha_of(&git_by(path, &["rev-parse", "HEAD"], deadline)?)?;
    Ok(Candidate {
        branch: branch.clone(),
        commit,
        base,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_past_the_cap_is_an_error_naming_it() {
        let cut = super::super::settle::git_capped(Path::new("."), &["--version"], None, 8);
        assert!(matches!(cut, Err(LaneError::Cut { cap: 8, .. })), "{cut:?}");
    }
}
