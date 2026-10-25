use std::path::Path;
use std::sync::Arc;

use yi_types::plan::doc::{Plan, PlanId, TodoLabel};
use yi_types::plan::ids::TodoAddr;

use super::store::PlanStore;

/// The trailer D99 writes, read back: `git blame` to commit to todo to goal
/// resolves "why does this line exist" with no inference.
pub const TRAILER: &str = "Plan: plan://";

const CAP: usize = 64 * 1024;

/// What one line's blame resolved to. Absent halves are absent, never guessed:
/// a commit with no trailer answers to no todo, and says so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub commit: String,
    pub subject: String,
    pub plan: Option<PlanId>,
    pub todo: Option<String>,
    pub goal: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum WhyError {
    #[error("git blame found nothing for {path}:{line}")]
    NotBlamed { path: String, line: u32 },
    #[error("{path}:{line} is not committed yet, so no commit answers for it")]
    Uncommitted { path: String, line: u32 },
    #[error("git {verb} failed: {detail}")]
    Git { verb: &'static str, detail: String },
}

fn git(cwd: &Path, verb: &'static str, args: &[&str]) -> Result<String, WhyError> {
    run(cwd, verb, args).map(|capture| capture.stdout)
}

fn run(
    cwd: &Path,
    verb: &'static str,
    args: &[&str],
) -> Result<yi_tools::CommandCapture, WhyError> {
    let mut process = yi_tools::command("git");
    process.arg("-C").arg(cwd).args(args);
    let never: yi_tools::CancelFlag = Arc::new(|| false);
    let capture = yi_tools::run_captured(process, None, &never, CAP)
        .map_err(|detail| WhyError::Git { verb, detail })?;
    if capture.exit_code == Some(0) {
        return Ok(capture);
    }
    Err(WhyError::Git {
        verb,
        detail: capture.stderr.trim().to_owned(),
    })
}

fn blame(cwd: &Path, path: &str, line: u32) -> Result<String, WhyError> {
    let range = format!("{line},{line}");
    let blamed = git(
        cwd,
        "blame",
        &["blame", "-L", &range, "--porcelain", "--", path],
    )?;
    let commit = blamed
        .split_whitespace()
        .next()
        .filter(|first| is_commit(first))
        .ok_or_else(|| WhyError::NotBlamed {
            path: path.to_owned(),
            line,
        })?;
    // Invariant: git blames a line of the work tree that no commit holds on the all-zero id.
    if commit.bytes().all(|byte| byte == b'0') {
        return Err(WhyError::Uncommitted {
            path: path.to_owned(),
            line,
        });
    }
    Ok(commit.to_owned())
}

fn is_commit(id: &str) -> bool {
    id.len() == 40 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn trailer_of(body: &str) -> Option<(PlanId, String)> {
    let address = body
        .lines()
        .find_map(|line| line.trim().strip_prefix(TRAILER))?
        .trim();
    let (plan, todo) = address.split_once('/')?;
    PlanId::new(plan).ok().map(|plan| (plan, todo.to_owned()))
}

/// The slug is what a `plan://` address carries, so the label is recovered by
/// re-slugging each todo rather than by inverting the slug.
fn label_of(plan: &Plan, slug: &str) -> Option<TodoLabel> {
    plan.todos.iter().find_map(|todo| {
        TodoAddr {
            plan: plan.id.clone(),
            todo: todo.label.clone(),
        }
        .to_url()
        .ok()
        .filter(|url| url.path().rsplit('/').next() == Some(slug))
        .map(|_| todo.label.clone())
    })
}

pub fn answer(cwd: &Path, plans_dir: &Path, path: &str, line: u32) -> Result<Answer, WhyError> {
    let commit = blame(cwd, path, line)?;
    let body = git(cwd, "show", &["show", "-s", "--format=%B", &commit])?;
    let subject = body.lines().next().unwrap_or_default().trim().to_owned();
    let Some((plan_id, slug)) = trailer_of(&body) else {
        return Ok(Answer {
            commit,
            subject,
            plan: None,
            todo: None,
            goal: None,
        });
    };
    let opened = PlanStore::open(plans_dir.to_path_buf())
        .ok()
        .and_then(|store| store.read(&plan_id).ok());
    let (todo, goal) = match &opened {
        Some(plan) => (
            Some(
                label_of(plan, &slug)
                    .map_or_else(|| slug.clone(), |label| label.as_str().to_owned()),
            ),
            Some(plan.goal.as_str().to_owned()),
        ),
        None => (Some(slug), None),
    };
    Ok(Answer {
        commit,
        subject,
        plan: Some(plan_id),
        todo,
        goal,
    })
}

/// Basic regexp named, so a user's `grep.patternType` never reads the trailer pattern.
const LOG: [&str; 4] = ["log", "--format=%H %s", "--basic-regexp", "--grep"];

/// The reverse index is the same data: every commit whose trailer names this todo exactly, so
/// `p/todo` never lists `p/todo-2`'s; a log past [`CAP`] ends with the row naming the cut.
pub fn commits_for(cwd: &Path, address: &str) -> Result<Commits, WhyError> {
    let escaped: String = (address.chars())
        .flat_map(|c| (".[]*^$\\".contains(c).then_some('\\').into_iter()).chain([c]))
        .collect();
    let pattern = format!("^[[:space:]]*{TRAILER}{escaped}[[:space:]]*$");
    let capture = run(cwd, "log", &[&LOG[..], &[pattern.as_str()]].concat())?;
    let found = (capture.stdout.lines())
        .filter_map(|line| line.split_once(' ').filter(|(commit, _)| is_commit(commit)))
        .map(|(commit, subject)| (commit.to_owned(), subject.to_owned()))
        .collect();
    let rest = format!("git log --format='%H %s' --basic-regexp --grep '{pattern}'");
    let cut = (capture.cut)
        .map(|(kept, all)| yi_tools::cut(kept, all, "bytes of git log", "CAP", CAP, &rest));
    Ok((found, cut))
}

/// Each commit and its subject, and the cut row when the log passed [`CAP`].
pub type Commits = (Vec<(String, String)>, Option<String>);
