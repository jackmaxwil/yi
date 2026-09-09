//! The root lane's hand-back: push, open the pull request, poll its gate.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use yi_types::acp::DaemonLedger;
use yi_types::event::AgentEvent;
use yi_types::lane::{JobState, Landing, LandingJob, PrNumber};
use yi_types::message::AgentMessage;
use yi_types::schedule::DeliveryMode;

use super::{ClaimBase, Holder, Lane, LaneError, Pool, SlotView, TreeState, git};

/// Invariant: a job name is forge-chosen text, bounded before a prompt or a latch key.
const JOB_NAME_MAX: usize = 64;
/// Invariant: the forge chooses how many checks a rollup has; the row and the prompt do not.
const JOBS_MAX: usize = 32;
const POLL_EVERY: std::time::Duration = std::time::Duration::from_secs(60);
const POLL_FOR: std::time::Duration = std::time::Duration::from_secs(2 * 60 * 60);
const FORGE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
const PUSH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);
/// Invariant: a conflict steer names files, bounded, inside a fence; never the whole diff.
const CONFLICT_FILES_MAX: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Forge {
    Forgejo,
    GitHub,
}

pub fn detect(repo: &Path) -> Result<(Forge, String), LaneError> {
    let url = git(repo, &["remote", "get-url", "origin"]).map_err(|_| LaneError::NoForge)?;
    let url = url.trim().to_owned();
    let forge = if url.contains("github.com") {
        Forge::GitHub
    } else {
        Forge::Forgejo
    };
    Ok((forge, url))
}

/// The origin's host, which `fgj api` needs spelled out; ssh or https, port dropped.
pub fn host_of(url: &str) -> Option<String> {
    let trimmed = url.trim();
    let rest = match trimmed.split_once("://") {
        Some((_, rest)) => rest,
        None => trimmed,
    };
    let rest = rest.rsplit_once('@').map_or(rest, |(_, rest)| rest);
    let host = rest.split(['/', ':']).next()?;
    (!host.is_empty()).then(|| host.to_owned())
}

/// `--repo` and `--hostname` for fgj: a lane is a worktree, whose `.git` is a file fgj
/// cannot read, so the target is spelled out from the origin on every call.
fn fgj_scope(url: &str) -> Result<[String; 2], LaneError> {
    let unreadable = || LaneError::Forge(format!("unreadable origin: {url}"));
    Ok([
        format!("--repo={}", owner_repo(url).ok_or_else(unreadable)?),
        format!("--hostname={}", host_of(url).ok_or_else(unreadable)?),
    ])
}

pub fn owner_repo(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches('/');
    let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    let path = match trimmed.split_once("://") {
        Some((_, rest)) => rest,
        None => trimmed.rsplit_once(':').map_or(trimmed, |(_, rest)| rest),
    };
    let mut parts = path.rsplit('/');
    let repo = parts.next()?;
    let owner = parts.next()?;
    (!owner.is_empty() && !repo.is_empty() && !owner.contains(':'))
        .then(|| format!("{owner}/{repo}"))
}

pub fn bounded_name(text: &str) -> String {
    text.chars().take(JOB_NAME_MAX).collect()
}

fn forge_call(cwd: &Path, program: &str, args: &[&str]) -> Result<String, LaneError> {
    super::capture(cwd, program, args, FORGE_TIMEOUT).map_err(LaneError::Forge)
}

pub fn pr_number(text: &str) -> Option<PrNumber> {
    for marker in ["/pulls/", "/pull/", "#"] {
        for (index, _) in text.match_indices(marker) {
            let digits: String = text
                .get(index.saturating_add(marker.len())..)?
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if let Ok(number) = digits.parse::<u32>() {
                return Some(PrNumber(number));
            }
        }
    }
    None
}

fn open_pr(forge: Forge, url: &str, lane: &Lane, title: &str) -> Result<PrNumber, LaneError> {
    let head = format!("--head={}", lane.branch());
    let title = format!("--title={title}");
    let output = match forge {
        Forge::Forgejo => {
            let [repo, host] = fgj_scope(url)?;
            forge_call(
                lane.path(),
                "fgj",
                &[
                    "pr",
                    "create",
                    &repo,
                    &host,
                    &head,
                    "--base=main",
                    &title,
                    "--body=",
                ],
            )?
        }
        Forge::GitHub => forge_call(
            lane.path(),
            "gh",
            &["pr", "create", &head, "--base=main", &title, "--body="],
        )?,
    };
    pr_number(&output)
        .ok_or_else(|| LaneError::Forge(format!("no pull request number in: {output}")))
}

/// The open PR whose head is `branch`: fgj rows carry `head.ref`, gh rows were filtered.
pub fn open_pr_in(text: &str, branch: &str) -> Option<PrNumber> {
    let Ok(Value::Array(rows)) = serde_json::from_str::<Value>(text) else {
        return None;
    };
    rows.iter()
        .find(|row| {
            row.pointer("/head/ref")
                .and_then(Value::as_str)
                .is_none_or(|head| head == branch)
        })
        .and_then(|row| row.get("number"))
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .map(PrNumber)
}

fn lookup_pr(forge: Forge, url: &str, lane: &Lane) -> Result<Option<PrNumber>, LaneError> {
    let branch = lane.branch().to_string();
    let text = match forge {
        Forge::Forgejo => {
            let [repo, host] = fgj_scope(url)?;
            forge_call(
                lane.path(),
                "fgj",
                &["pr", "list", &repo, &host, "--state=open", "--json"],
            )?
        }
        Forge::GitHub => forge_call(
            lane.path(),
            "gh",
            &[
                "pr",
                "list",
                "--state=open",
                &format!("--head={branch}"),
                "--json=number",
            ],
        )?,
    };
    Ok(open_pr_in(&text, &branch))
}

fn job_state(status: &str, conclusion: Option<&str>) -> JobState {
    match (status, conclusion) {
        ("pending" | "QUEUED" | "PENDING" | "WAITING" | "REQUESTED", _) => JobState::Queued,
        ("IN_PROGRESS" | "running", _) => JobState::Running,
        ("success", _) | (_, Some("SUCCESS" | "NEUTRAL" | "SKIPPED")) => JobState::Green,
        ("failure" | "error", _)
        | (_, Some("FAILURE" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED")) => JobState::Red,
        ("COMPLETED", None) => JobState::Green,
        (other, _) => JobState::Other(bounded_name(other)),
    }
}

pub fn parse_jobs(entries: &[Value]) -> Vec<LandingJob> {
    let mut jobs: Vec<LandingJob> = entries
        .iter()
        .take(JOBS_MAX)
        .filter_map(|entry| {
            let name = entry
                .get("name")
                .or_else(|| entry.get("context"))
                .and_then(Value::as_str)?;
            let status = entry
                .get("status")
                .or_else(|| entry.get("state"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let conclusion = entry.get("conclusion").and_then(Value::as_str);
            Some(LandingJob {
                name: bounded_name(name),
                state: job_state(status, conclusion),
            })
        })
        .collect();
    if let Some(more) = entries.len().checked_sub(JOBS_MAX).filter(|more| *more > 0) {
        jobs.push(LandingJob {
            name: format!("+{more} more"),
            state: JobState::Other("unlisted".to_owned()),
        });
    }
    jobs
}

fn poll_forge(forge: Forge, url: &str, lane: &Lane, pr: PrNumber) -> Result<Landing, LaneError> {
    let jobs = match forge {
        Forge::GitHub => {
            let number = pr.0.to_string();
            let text = forge_call(
                lane.path(),
                "gh",
                &["pr", "view", &number, "--json", "state,statusCheckRollup"],
            )?;
            let view: Value =
                serde_json::from_str(&text).map_err(|error| LaneError::Forge(error.to_string()))?;
            if view.get("state").and_then(Value::as_str) == Some("MERGED") {
                return Ok(Landing::Merged { pr });
            }
            view.get("statusCheckRollup")
                .and_then(Value::as_array)
                .map(|entries| parse_jobs(entries))
                .unwrap_or_default()
        }
        Forge::Forgejo => {
            let slug = owner_repo(url)
                .ok_or_else(|| LaneError::Forge(format!("unreadable origin: {url}")))?;
            let [_, host] = fgj_scope(url)?;
            let text = forge_call(
                lane.path(),
                "fgj",
                &["api", &host, &format!("repos/{slug}/pulls/{}", pr.0)],
            )?;
            let view: Value =
                serde_json::from_str(&text).map_err(|error| LaneError::Forge(error.to_string()))?;
            if view.get("merged").and_then(Value::as_bool) == Some(true) {
                return Ok(Landing::Merged { pr });
            }
            let sha = view
                .pointer("/head/sha")
                .and_then(Value::as_str)
                .ok_or_else(|| LaneError::Forge("pull request without a head sha".to_owned()))?;
            let text = forge_call(
                lane.path(),
                "fgj",
                &[
                    "api",
                    &host,
                    &format!("repos/{slug}/commits/{sha}/statuses"),
                ],
            )?;
            let statuses: Value =
                serde_json::from_str(&text).map_err(|error| LaneError::Forge(error.to_string()))?;
            statuses
                .as_array()
                .map(|entries| parse_jobs(entries))
                .unwrap_or_default()
        }
    };
    let behind = git(lane.path(), &["rev-list", "--count", "HEAD..origin/main"])
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
        .unwrap_or(0);
    Ok(Landing::Open { pr, jobs, behind })
}

fn merge_pr(forge: Forge, url: &str, lane: &Lane, pr: PrNumber) -> Result<String, LaneError> {
    let number = pr.0.to_string();
    match forge {
        Forge::Forgejo => {
            let [repo, host] = fgj_scope(url)?;
            forge_call(
                lane.path(),
                "fgj",
                &["pr", "merge", &repo, &host, &number, "--merge-method=merge"],
            )
        }
        Forge::GitHub => forge_call(lane.path(), "gh", &["pr", "merge", &number, "--merge"]),
    }
}

pub type SteerFn = dyn Fn(AgentMessage, DeliveryMode) + Send + Sync;

pub struct LaneHandle {
    lane: Mutex<Option<Lane>>,
    /// The pool the cwd's repository has, lane or not, so `/lanes` reads from the trunk too.
    pool: Option<Pool>,
    landing: Mutex<Landing>,
    land_command: Option<Vec<String>>,
    events: tokio::sync::broadcast::Sender<AgentEvent>,
    steer: Arc<SteerFn>,
    /// Invariant: one steer per red job name, never one per poll.
    latched: Mutex<BTreeSet<String>>,
    /// Invariant: one poller per handle; a second `/land` while one runs is refused.
    polling: AtomicBool,
}

impl LaneHandle {
    pub fn new(
        lane: Option<Lane>,
        pool: Option<Pool>,
        land_command: Option<Vec<String>>,
        events: tokio::sync::broadcast::Sender<AgentEvent>,
        steer: Arc<SteerFn>,
    ) -> Arc<Self> {
        let pool = pool.or_else(|| lane.as_ref().map(|lane| lane.pool().clone()));
        Arc::new(Self {
            lane: Mutex::new(lane),
            pool,
            landing: Mutex::new(Landing::Unlanded),
            land_command,
            events,
            steer,
            latched: Mutex::new(BTreeSet::new()),
            polling: AtomicBool::new(false),
        })
    }

    fn take_poller(&self) -> bool {
        self.polling
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    }

    fn release_poller(&self) {
        self.polling.store(false, Ordering::Release);
    }

    fn with_lane<R>(&self, f: impl FnOnce(&Lane) -> Result<R, LaneError>) -> Result<R, LaneError> {
        let guard = self
            .lane
            .lock()
            .map_err(|_| LaneError::Forge("lane state poisoned".to_owned()))?;
        match guard.as_ref() {
            Some(lane) => f(lane),
            None => Err(LaneError::Forge(
                "this session runs in the trunk checkout (--here); nothing to land".to_owned(),
            )),
        }
    }

    /// The quit line: slot, branch, path, and the pull request when there is one.
    pub fn describe(&self) -> Option<String> {
        let guard = self.lane.lock().ok()?;
        let lane = guard.as_ref()?;
        let landing = match self.landing() {
            Landing::Open { pr, .. } => format!(" · PR {pr} open"),
            Landing::Merged { pr } => format!(" · PR {pr} merged"),
            Landing::Unlanded | Landing::Pushed { .. } => String::new(),
        };
        Some(format!(
            "lane {} · {} · {}{landing}",
            lane.slot(),
            lane.branch(),
            tilde(lane.path())
        ))
    }

    pub fn slot(&self) -> Option<super::SlotIndex> {
        self.lane.lock().ok()?.as_ref().map(Lane::slot)
    }

    pub fn path(&self) -> Option<std::path::PathBuf> {
        self.lane
            .lock()
            .ok()?
            .as_ref()
            .map(|lane| lane.path().to_path_buf())
    }

    pub fn landing(&self) -> Landing {
        self.landing
            .lock()
            .map(|landing| landing.clone())
            .unwrap_or(Landing::Unlanded)
    }

    fn set_landing(&self, landing: Landing) {
        if let Ok(mut slot) = self.landing.lock() {
            *slot = landing.clone();
        }
        let _ = self.events.send(AgentEvent::LandingState { landing });
    }

    /// Runs in the background: a pre-push lane can take minutes.
    pub fn land(self: &Arc<Self>, title: &str) -> Result<String, LaneError> {
        let title = title.trim().to_owned();
        if title.is_empty() {
            return Err(LaneError::Forge("/land needs a title".to_owned()));
        }
        let branch = self.with_lane(|lane| Ok(lane.branch().to_string()))?;
        if !self.take_poller() {
            return Err(LaneError::Forge(
                "landing in progress; the status row follows it".to_owned(),
            ));
        }
        let handle = Arc::clone(self);
        std::thread::spawn(move || {
            if let Err(error) = handle.land_blocking(&title) {
                (handle.steer)(
                    crate::session::user_message(&format!("landing failed: {error}")),
                    DeliveryMode::Steer,
                );
            }
            handle.release_poller();
        });
        Ok(format!(
            "landing {branch}: pushing in the background; the status row follows it"
        ))
    }

    /// A resumed session finds its pull request on the forge instead of opening a second
    /// one: off the start path, on the land thread's shape, reaching the row as an event.
    pub fn reattach(self: &Arc<Self>) {
        if self.path().is_none() || !self.take_poller() {
            return;
        }
        let handle = Arc::clone(self);
        std::thread::spawn(move || {
            if matches!(handle.refresh(), Ok(Landing::Open { .. })) {
                handle.poll_until_merged();
            }
            handle.release_poller();
        });
    }

    /// Invariant: a merge, never a rebase on a branch that may be pushed. A conflict
    /// aborts, verifies the tree is clean again, and names the files; nothing is pushed.
    fn merge_main(&self) -> Result<(), LaneError> {
        self.with_lane(|lane| {
            let _ = git(lane.pool().repo(), &["fetch", "-q", "origin", "main"]);
            if git(lane.path(), &["merge", "--no-edit", "origin/main"]).is_ok() {
                return Ok(());
            }
            let files: Vec<String> = git(
                lane.path(),
                &["diff", "--name-only", "--diff-filter=U"],
            )
            .unwrap_or_default()
            .lines()
            .take(CONFLICT_FILES_MAX)
            .map(|line| crate::environment::sanitize(line.trim()))
            .collect();
            git(lane.path(), &["merge", "--abort"])?;
            if !git(lane.path(), &["status", "--porcelain"])?.trim().is_empty() {
                return Err(LaneError::Forge(format!(
                    "merge of origin/main left the tree dirty in {}; resolve by hand",
                    lane.path().display()
                )));
            }
            Err(LaneError::Forge(format!(
                "origin/main conflicts with this branch; nothing pushed. Conflicts:\n```\n{}\n```\nresolve in {}, commit, and /land again",
                files.join("\n"),
                lane.path().display()
            )))
        })
    }

    fn land_blocking(self: &Arc<Self>, title: &str) -> Result<(), LaneError> {
        if let Landing::Open { pr, jobs, .. } = self.landing()
            && !jobs.is_empty()
            && jobs.iter().all(|job| job.state == JobState::Green)
        {
            let (forge, url) = self.with_lane(|lane| detect(lane.pool().repo()))?;
            self.with_lane(|lane| merge_pr(forge, &url, lane, pr))?;
            self.set_landing(Landing::Merged { pr });
            return Ok(());
        }
        let pr = match &self.land_command {
            Some(argv) => self.with_lane(|lane| {
                let (program, args) = argv
                    .split_first()
                    .ok_or_else(|| LaneError::Forge("lanes.land is empty".to_owned()))?;
                let mut args: Vec<&str> = args.iter().map(String::as_str).collect();
                args.push(title);
                let output = super::capture(lane.path(), program, &args, PUSH_TIMEOUT)
                    .map_err(LaneError::Forge)?;
                Ok(pr_number(&output))
            })?,
            None => {
                let (forge, url) = self.with_lane(|lane| detect(lane.pool().repo()))?;
                self.merge_main()?;
                self.with_lane(|lane| {
                    let branch = lane.branch().to_string();
                    super::capture(
                        lane.path(),
                        "git",
                        &["push", "-q", "-u", "origin", &branch],
                        PUSH_TIMEOUT,
                    )
                    .map_err(LaneError::Forge)
                })?;
                let branch = self.with_lane(|lane| Ok(lane.branch().to_string()))?;
                self.set_landing(Landing::Pushed { branch });
                Some(self.with_lane(|lane| open_pr(forge, &url, lane, title))?)
            }
        };
        let Some(pr) = pr else {
            let branch = self.with_lane(|lane| Ok(lane.branch().to_string()))?;
            self.set_landing(Landing::Pushed { branch });
            return Ok(());
        };
        self.set_landing(Landing::Open {
            pr,
            jobs: Vec::new(),
            behind: 0,
        });
        self.poll_until_merged();
        Ok(())
    }

    fn poll_until_merged(&self) {
        let started = std::time::Instant::now();
        while started.elapsed() < POLL_FOR {
            std::thread::sleep(POLL_EVERY);
            match self.refresh() {
                Ok(Landing::Merged { .. }) => break,
                Ok(_) => {}
                Err(error) => {
                    (self.steer)(
                        crate::session::user_message(&format!("gate poll failed: {error}")),
                        DeliveryMode::Steer,
                    );
                    break;
                }
            }
        }
    }

    /// Unlanded asks the forge for an open pull request on this branch first; a forge
    /// that cannot answer leaves it unlanded rather than failing the read.
    pub fn refresh(&self) -> Result<Landing, LaneError> {
        let pr = match self.landing() {
            Landing::Open { pr, .. } => pr,
            Landing::Unlanded => {
                let found = self
                    .with_lane(|lane| detect(lane.pool().repo()))
                    .and_then(|(forge, url)| self.with_lane(|lane| lookup_pr(forge, &url, lane)));
                match found {
                    Ok(Some(pr)) => pr,
                    Ok(None) | Err(_) => return Ok(Landing::Unlanded),
                }
            }
            other => return Ok(other),
        };
        let (forge, url) = self.with_lane(|lane| detect(lane.pool().repo()))?;
        let landing = self.with_lane(|lane| poll_forge(forge, &url, lane, pr))?;
        if let Landing::Open { jobs, .. } = &landing
            && let Ok(mut latched) = self.latched.lock()
        {
            for job in jobs.iter().filter(|job| job.state == JobState::Red) {
                if latched.insert(job.name.clone()) {
                    (self.steer)(
                        crate::session::user_message(&format!(
                            "gate red on pull request {pr}: job\n```\n{}\n```\nread its log, fix, and /land again",
                            job.name
                        )),
                        DeliveryMode::Steer,
                    );
                }
            }
        }
        self.set_landing(landing.clone());
        Ok(landing)
    }

    pub fn lanes(&self) -> Result<String, LaneError> {
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| LaneError::Forge("not inside a git repository".to_owned()))?;
        pool.list().map(|views| format_lanes(&views, None))
    }

    pub fn discard(&self) -> Result<String, LaneError> {
        let lane = self
            .lane
            .lock()
            .map_err(|_| LaneError::Forge("lane state poisoned".to_owned()))?
            .take()
            .ok_or_else(|| LaneError::Forge("no lane to discard".to_owned()))?;
        let branch = lane.branch().to_string();
        lane.discard()?;
        self.set_landing(Landing::Unlanded);
        Ok(format!("discarded {branch}; the transcript is kept"))
    }

    /// Incident: `process::exit` skips destructors, so the CLI calls this first.
    pub fn release(&self) -> Result<(), LaneError> {
        let lane = self
            .lane
            .lock()
            .map_err(|_| LaneError::Forge("lane state poisoned".to_owned()))?
            .take();
        match lane {
            Some(lane) => lane.release(),
            None => Ok(()),
        }
    }

    pub fn bind_session(&self, session: &str) -> Result<(), LaneError> {
        let mut guard = self
            .lane
            .lock()
            .map_err(|_| LaneError::Forge("lane state poisoned".to_owned()))?;
        match guard.as_mut() {
            Some(lane) => lane.bind_session(session),
            None => Ok(()),
        }
    }
}

pub fn format_lanes(views: &[SlotView], ledger: Option<&DaemonLedger>) -> String {
    if views.is_empty() {
        return "no lanes yet".to_owned();
    }
    views
        .iter()
        .map(|view| lane_line(view, ledger))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `$HOME` as `~`, so a slot path reads at a glance.
pub fn tilde(path: &Path) -> String {
    let text = path.to_string_lossy();
    std::env::var_os("HOME")
        .and_then(|home| {
            let home = home.to_string_lossy();
            text.strip_prefix(home.as_ref())
                .map(|rest| format!("~{rest}"))
        })
        .unwrap_or_else(|| text.into_owned())
}

const NAME_MAX: usize = 32;

/// Text from the ledger or the forge, printable and bounded before it shares a line.
fn plain(text: &str, max: usize) -> String {
    text.chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .take(max)
        .collect()
}

fn who(holder: &Holder, ledger: Option<&DaemonLedger>) -> String {
    let entry = ledger.and_then(|ledger| ledger.sessions.get(&holder.session));
    match entry {
        Some(entry) => {
            let name = entry
                .name
                .as_deref()
                .map(|name| plain(name, NAME_MAX))
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "unnamed".to_owned());
            format!(
                "{} ({name}, {})",
                holder.session,
                tilde(Path::new(&entry.cwd))
            )
        }
        None => holder.session.clone(),
    }
}

fn age_label(age: std::time::Duration) -> String {
    let secs = age.as_secs();
    match secs {
        s if s < 3_600 => format!("{} min", s / 60),
        s if s < 48 * 3_600 => format!("{} h", s / 3_600),
        s => format!("{} d", s / 86_400),
    }
}

fn tree_label(tree: &TreeState) -> String {
    match tree {
        TreeState::Unread { step } => format!("tree unread ({step} timed out)"),
        TreeState::Known {
            modified,
            untracked,
            ahead,
            behind,
        } => {
            let mut parts = Vec::new();
            if *modified > 0 {
                parts.push(format!("{modified} modified"));
            }
            if *untracked > 0 {
                parts.push(format!("{untracked} untracked"));
            }
            if parts.is_empty() {
                parts.push("clean".to_owned());
            }
            parts.push(format!("{ahead} ahead, {behind} behind"));
            parts.join(", ")
        }
    }
}

/// What a reap costs, said before anyone types it.
fn verdict(tree: &TreeState) -> &'static str {
    match tree {
        TreeState::Unread { .. } => "reap keeps the branch; the tree is unread",
        tree if tree.is_empty() => "the next claim takes it",
        TreeState::Known {
            modified,
            untracked,
            ahead,
            ..
        } => match (*ahead > 0, modified.saturating_add(*untracked) > 0) {
            (true, true) => "reap keeps the branch; the uncommitted paths are lost",
            (true, false) => "reap keeps the branch",
            (false, true) => "reap deletes the merged branch; the uncommitted paths are lost",
            (false, false) => "the next claim takes it",
        },
    }
}

fn holder_line(holder: &Holder, ledger: Option<&DaemonLedger>) -> String {
    let mut text = format!(
        "{} on {} · {}",
        who(holder, ledger),
        holder
            .branch
            .as_ref()
            .map_or_else(|| "(detached)".to_owned(), ToString::to_string),
        tree_label(&holder.tree)
    );
    if let Some(age) = holder.age {
        text.push_str(&format!(" · {}", age_label(age)));
    }
    text
}

pub fn lane_line(view: &SlotView, ledger: Option<&DaemonLedger>) -> String {
    match view {
        SlotView::Idle {
            slot,
            path,
            base,
            warm,
        } => format!(
            "lane {slot}: idle{} at {} · {}",
            if *warm { ", warm" } else { "" },
            base.as_deref()
                .map_or("(unset)".to_owned(), |sha| sha.chars().take(12).collect()),
            tilde(path)
        ),
        SlotView::Held { slot, path, holder } => format!(
            "lane {slot}: held by {} · {}",
            holder_line(holder, ledger),
            tilde(path)
        ),
        SlotView::Orphan { slot, path, holder } => format!(
            "lane {slot}: left by {} · {} — {}",
            holder_line(holder, ledger),
            tilde(path),
            verdict(&holder.tree)
        ),
    }
}

pub fn claim_root(pool: &Pool, session: &str) -> Result<Lane, LaneError> {
    let lane = pool.claim(session, ClaimBase::Main)?;
    lane.sync()?;
    Ok(lane)
}
