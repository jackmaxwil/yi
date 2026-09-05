//! The root lane's hand-back: push, open the pull request, poll its gate.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use yi_types::event::AgentEvent;
use yi_types::lane::{JobState, Landing, LandingJob, PrNumber};
use yi_types::message::AgentMessage;
use yi_types::schedule::DeliveryMode;

use super::{ClaimBase, Lane, LaneError, Pool, SlotView, git};

/// Invariant: a job name is forge-chosen text, bounded before a prompt or a latch key.
const JOB_NAME_MAX: usize = 64;
const POLL_EVERY: std::time::Duration = std::time::Duration::from_secs(60);
const POLL_FOR: std::time::Duration = std::time::Duration::from_secs(2 * 60 * 60);
const FORGE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
const PUSH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);

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

fn open_pr(forge: Forge, lane: &Lane, title: &str) -> Result<PrNumber, LaneError> {
    let head = format!("--head={}", lane.branch());
    let title = format!("--title={title}");
    let output = match forge {
        Forge::Forgejo => forge_call(
            lane.path(),
            "tea",
            &[
                "pr",
                "create",
                &head,
                "--base=main",
                &title,
                "--description=",
            ],
        )?,
        Forge::GitHub => forge_call(
            lane.path(),
            "gh",
            &["pr", "create", &head, "--base=main", &title, "--body="],
        )?,
    };
    pr_number(&output)
        .ok_or_else(|| LaneError::Forge(format!("no pull request number in: {output}")))
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

fn parse_jobs(entries: &[Value]) -> Vec<LandingJob> {
    entries
        .iter()
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
        .collect()
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
            let text = forge_call(
                lane.path(),
                "tea",
                &["api", &format!("/repos/{slug}/pulls/{}", pr.0)],
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
                "tea",
                &["api", &format!("/repos/{slug}/commits/{sha}/statuses")],
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

fn merge_pr(forge: Forge, lane: &Lane, pr: PrNumber) -> Result<String, LaneError> {
    let number = pr.0.to_string();
    match forge {
        Forge::Forgejo => forge_call(lane.path(), "tea", &["pr", "merge", &number]),
        Forge::GitHub => forge_call(lane.path(), "gh", &["pr", "merge", &number, "--merge"]),
    }
}

pub type SteerFn = dyn Fn(AgentMessage, DeliveryMode) + Send + Sync;

pub struct LaneHandle {
    lane: Mutex<Option<Lane>>,
    landing: Mutex<Landing>,
    land_command: Option<Vec<String>>,
    events: tokio::sync::broadcast::Sender<AgentEvent>,
    steer: Arc<SteerFn>,
    /// Invariant: one steer per red job name, never one per poll.
    latched: Mutex<BTreeSet<String>>,
}

impl LaneHandle {
    pub fn new(
        lane: Option<Lane>,
        land_command: Option<Vec<String>>,
        events: tokio::sync::broadcast::Sender<AgentEvent>,
        steer: Arc<SteerFn>,
    ) -> Arc<Self> {
        Arc::new(Self {
            lane: Mutex::new(lane),
            landing: Mutex::new(Landing::Unlanded),
            land_command,
            events,
            steer,
            latched: Mutex::new(BTreeSet::new()),
        })
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

    pub fn describe(&self) -> Option<String> {
        let guard = self.lane.lock().ok()?;
        let lane = guard.as_ref()?;
        let base: String = lane.base().unwrap_or_default().chars().take(12).collect();
        Some(format!(
            "lane {} · branch {} off {base} · land with /land \"Title\"",
            lane.slot(),
            lane.branch()
        ))
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
        let handle = Arc::clone(self);
        std::thread::spawn(move || {
            if let Err(error) = handle.land_blocking(&title) {
                (handle.steer)(
                    crate::session::user_message(&format!("landing failed: {error}")),
                    DeliveryMode::Steer,
                );
            }
        });
        Ok(format!(
            "landing {branch}: pushing in the background; the status row follows it"
        ))
    }

    fn land_blocking(self: &Arc<Self>, title: &str) -> Result<(), LaneError> {
        if let Landing::Open { pr, jobs, .. } = self.landing()
            && !jobs.is_empty()
            && jobs.iter().all(|job| job.state == JobState::Green)
        {
            let (forge, _) = self.with_lane(|lane| detect(lane.pool().repo()))?;
            self.with_lane(|lane| merge_pr(forge, lane, pr))?;
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
                let (forge, _) = self.with_lane(|lane| detect(lane.pool().repo()))?;
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
                Some(self.with_lane(|lane| open_pr(forge, lane, title))?)
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
        Ok(())
    }

    pub fn refresh(&self) -> Result<Landing, LaneError> {
        let Landing::Open { pr, .. } = self.landing() else {
            return Ok(self.landing());
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

    /// Invariant: a merge, never a rebase on a branch that may be pushed.
    pub fn base(&self) -> Result<String, LaneError> {
        self.with_lane(|lane| {
            let _ = git(lane.pool().repo(), &["fetch", "-q", "origin", "main"]);
            git(lane.path(), &["merge", "--no-edit", "origin/main"])
        })
    }

    pub fn lanes(&self) -> Result<String, LaneError> {
        self.with_lane(|lane| lane.pool().list().map(|views| format_lanes(&views)))
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

pub fn format_lanes(views: &[SlotView]) -> String {
    if views.is_empty() {
        return "no lanes yet".to_owned();
    }
    views
        .iter()
        .map(|view| match view {
            SlotView::Idle { slot, base, warm } => format!(
                "lane {slot}: idle{} at {}",
                if *warm { ", warm" } else { "" },
                base.as_deref()
                    .map_or("(unset)".to_owned(), |sha| sha.chars().take(12).collect())
            ),
            SlotView::Held {
                slot,
                session,
                branch,
            } => format!(
                "lane {slot}: held by {session} on {}",
                branch.as_deref().unwrap_or("(detached)")
            ),
            SlotView::Orphan {
                slot,
                session,
                branch,
            } => format!(
                "lane {slot}: left by {session} on {} — `yi lanes reap {slot}`",
                branch.as_deref().unwrap_or("(detached)")
            ),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn claim_root(pool: &Pool, session: &str) -> Result<Lane, LaneError> {
    let lane = pool.claim(session, ClaimBase::Main)?;
    lane.sync()?;
    Ok(lane)
}
