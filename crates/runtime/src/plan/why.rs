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
    #[error("git {verb} failed: {detail}")]
    Git { verb: &'static str, detail: String },
}

fn git(cwd: &Path, verb: &'static str, args: &[&str]) -> Result<String, WhyError> {
    let mut process = yi_tools::command("git");
    process.arg("-C").arg(cwd).args(args);
    let never: yi_tools::CancelFlag = Arc::new(|| false);
    let capture = yi_tools::run_captured(process, None, &never, CAP)
        .map_err(|detail| WhyError::Git { verb, detail })?;
    if capture.exit_code == Some(0) {
        return Ok(capture.stdout);
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
    blamed
        .split_whitespace()
        .next()
        .filter(|first| first.len() == 40 && first.chars().all(|byte| byte.is_ascii_hexdigit()))
        .map(str::to_owned)
        .ok_or_else(|| WhyError::NotBlamed {
            path: path.to_owned(),
            line,
        })
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
        .and_then(|store| store.read(&plan_id).ok())
        .map(|file| file.plan);
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

/// The reverse index is the same data: every commit that carried this todo's
/// trailer, which is what makes a todo's row accumulate what it produced.
pub fn commits_for(cwd: &Path, address: &str) -> Result<Vec<(String, String)>, WhyError> {
    let needle = format!("{TRAILER}{address}");
    let found = git(
        cwd,
        "log",
        &[
            "log",
            "--format=%H %s",
            "--fixed-strings",
            "--grep",
            needle.as_str(),
        ],
    )?;
    Ok(found
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(commit, subject)| (commit.to_owned(), subject.to_owned()))
        .collect())
}
