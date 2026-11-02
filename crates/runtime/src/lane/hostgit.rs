//! Every git the host runs on a tree a contained process wrote: the one runner, the commit that
//! turns a child's work into a candidate, and which branch is trunk.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use yi_types::plan::acceptance::Quiescence;

use super::{BranchName, GIT_TIMEOUT_MS, Head, LaneError, Sha};

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

/// The one spawn a host git gets: the door's `Command`, its cwd and args, the caller's deadline and
/// the cap. Incident: a sandboxed refusal quoted 17 KB of wrapper argv before the line saying why.
pub(crate) fn capture_named(
    cwd: &Path,
    label: &str,
    program: &str,
    args: &[&str],
    deadline: std::time::Duration,
    cap: usize,
    sandbox: Option<&yi_tools::Sandbox>,
) -> Result<yi_tools::CommandCapture, String> {
    let capture =
        run(cwd, program, args, &[], deadline, cap).map_err(|error| format!("{label}: {error}"))?;
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

fn run(
    cwd: &Path,
    program: &str,
    args: &[&str],
    envs: &[(&str, &str)],
    deadline: std::time::Duration,
    cap: usize,
) -> Result<yi_tools::CommandCapture, String> {
    let _span = yi_types::trace::span("lane.capture").arg("program", program);
    let mut command = yi_tools::host_command(program, cwd).map_err(|error| error.to_string())?;
    command
        .current_dir(cwd)
        .args(args)
        .envs(envs.iter().copied());
    let deadline = std::time::Instant::now()
        .checked_add(deadline)
        .unwrap_or_else(std::time::Instant::now);
    let cancelled: yi_tools::CancelFlag = Arc::new(move || std::time::Instant::now() >= deadline);
    yi_tools::run_captured(command, None, &cancelled, cap)
}

pub(crate) fn git(cwd: &Path, args: &[&str]) -> Result<String, LaneError> {
    git_capped(cwd, args, None, PLUMBING_CAP)
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
    envs: &[(&str, &str)],
    deadline: Option<Instant>,
    cap: usize,
) -> Result<yi_tools::CommandCapture, LaneError> {
    past(deadline)?;
    let owned: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    let own = Instant::now()
        .checked_add(Duration::from_millis(GIT_TIMEOUT_MS))
        .unwrap_or_else(Instant::now);
    let until = deadline.map_or(own, |at| own.min(at));
    let capture = run(
        cwd,
        "git",
        args,
        envs,
        until.saturating_duration_since(Instant::now()),
        cap,
    )
    .map_err(|output| LaneError::Git {
        args: owned.clone(),
        exit_code: None,
        output,
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

/// `git` under the caller's deadline as well as the lane's own: a settle step cannot outlive
/// the run it belongs to (D177).
pub(super) fn git_by(
    cwd: &Path,
    args: &[&str],
    deadline: Option<Instant>,
) -> Result<String, LaneError> {
    git_capped(cwd, args, deadline, PLUMBING_CAP)
}

pub(super) fn git_capped(
    cwd: &Path,
    args: &[&str],
    deadline: Option<Instant>,
    cap: usize,
) -> Result<String, LaneError> {
    whole(git_capture(cwd, args, &[], deadline, cap)?, args, cap)
}

pub(super) fn past(deadline: Option<Instant>) -> Result<(), LaneError> {
    match deadline {
        Some(at) if Instant::now() >= at => Err(LaneError::Deadline),
        _ => Ok(()),
    }
}

pub(super) fn sha_of(text: &str) -> Result<Sha, LaneError> {
    Sha::parse(text).ok_or_else(|| LaneError::Git {
        args: vec!["rev-parse".to_owned()],
        exit_code: None,
        output: format!("not a commit id: {text:?}"),
    })
}

pub(super) fn quiet(quiescence: &Quiescence) -> Result<(), LaneError> {
    if quiescence.is_quiet() {
        return Ok(());
    }
    Err(LaneError::Busy {
        running: quiescence.running.clone(),
    })
}

/// The immutable output references a settled lane hands on: the candidate commit and the
/// branch that holds it, and the base the branch was cut from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub branch: BranchName,
    pub commit: Sha,
    pub base: Option<Sha>,
}

impl Candidate {
    pub fn as_reply(&self) -> Map<String, Value> {
        let mut reply = Map::new();
        reply.insert(
            "branch".to_owned(),
            Value::String(self.branch.as_str().to_owned()),
        );
        reply.insert(
            "candidate".to_owned(),
            Value::String(self.commit.as_str().to_owned()),
        );
        reply
    }
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

/// The branch this repository lands on.
pub fn trunk(repo: &Path) -> BranchName {
    let name = yi_permission::git_dirs(repo).pop().map_or_else(
        || "main".to_owned(),
        |common| yi_permission::trunk_of(&common),
    );
    BranchName::parse(&name).unwrap_or_else(|_| BranchName::main())
}

/// `-c` for what a repository's identity lacks: a host commit must not fail for want of a name.
pub(super) fn identity(tree: &Path) -> Vec<&'static str> {
    let has = |key| git(tree, &["config", key]).is_ok_and(|value| !value.trim().is_empty());
    let name = (!has("user.name")).then_some(["-c", "user.name=yi"]);
    let email = (!has("user.email")).then_some(["-c", "user.email=yi@localhost"]);
    name.into_iter().chain(email).flatten().collect()
}

/// Invariant: a tree's admin `HEAD` is contained-writable, so a planted one would walk the host's
/// next commit or merge onto trunk, a sibling's branch or another lane's kept one.
pub(super) fn own_head(tree: &Path, own: &BranchName) -> Result<(), LaneError> {
    let branch = match super::head(tree)? {
        Head::Branch(branch) => branch,
        Head::Detached(_) => return Ok(()),
    };
    if branch == *own {
        return Ok(());
    }
    let head = branch.as_str();
    let repo =
        super::canonical_repo(tree).ok_or_else(|| LaneError::NotARepo(tree.to_path_buf()))?;
    let here = tree.canonicalize().unwrap_or_else(|_| tree.to_path_buf());
    let listing = git(&repo, &["worktree", "list", "--porcelain"])?;
    let elsewhere = super::reap::parse(&listing).iter().any(|other| {
        other.branch.as_deref() == Some(head)
            && other.path.canonicalize().map_or(true, |path| path != here)
    });
    if head == trunk(&repo).as_str() || head.starts_with("yi/") || elsewhere {
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
        own_head(path, branch)?;
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
        let cut = super::git_capped(Path::new("."), &["--version"], None, 8);
        assert!(matches!(cut, Err(LaneError::Cut { cap: 8, .. })), "{cut:?}");
    }
}
