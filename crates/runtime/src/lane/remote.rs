//! A contained git reaches origin through two stores the host runs: reads come from a mirror it
//! refreshes, pushes land in an outbox it empties and forwards after the spawn.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::hostgit::{git, git_capture, trunk};
use super::{BranchName, LaneError, Pool, Sha};
use crate::permission::{AskOutcome, PermissionAsk, PermissionBroker};

const OUTBOX_REFS_MAX: usize = 32;
const FORWARD_TIMEOUT: Duration = Duration::from_secs(120);
const REMOTE_TEXT_CHARS_MAX: usize = 2_000;
const CAPTURE_CAP: usize = 256 * 1024;

/// What a host forward came to. `Unknown` is its own state: a push past its deadline may have landed.
enum Forward {
    Pushed { old: Option<Sha> },
    Behind,
    Rejected(String),
    Unknown,
}

/// A ref the outbox holds, read from bytes a contained process wrote.
#[derive(Debug, PartialEq, Eq)]
pub enum Wanted {
    Branch(BranchName),
    Tag(String),
}

/// `None` for a name outside `heads/` and `tags/`, or a value that is no sha.
#[must_use]
pub fn outbox_ref(name: &[u8], value: &[u8]) -> Option<(Wanted, Sha)> {
    let name = std::str::from_utf8(name).ok()?;
    let sha = Sha::parse(std::str::from_utf8(value).ok()?)?;
    let wanted = match (name.strip_prefix("heads/"), name.strip_prefix("tags/")) {
        (Some(branch), _) => Wanted::Branch(BranchName::parse(branch).ok()?),
        (_, Some(tag)) => Wanted::Tag(BranchName::parse(tag).ok()?.to_string()),
        _ => return None,
    };
    Some((wanted, sha))
}

/// `https://user:token@host/…`. Such a URL would go on `argv` in the rewrite rule, so the repository
/// keeps today's ask instead.
fn carries_credentials(url: &str) -> bool {
    url.split_once("://")
        .filter(|(scheme, _)| scheme.starts_with("http"))
        .is_some_and(|(_, rest)| rest.split('/').next().unwrap_or_default().contains('@'))
}

pub struct GitLane {
    pool: Pool,
    mirror: PathBuf,
    outbox: PathBuf,
    assign: Vec<String>,
    forward_timeout: Duration,
}

// ponytail: one lane per tree for the process's life; a `git remote set-url` by hand needs a restart.
static LANES: Mutex<Option<HashMap<PathBuf, Option<Arc<GitLane>>>>> = Mutex::new(None);

impl GitLane {
    /// `None` with no origin, no pool or a credentialed origin: `git push` then fails contained
    /// and its refusal asks, as it did before there was a lane.
    pub(crate) fn for_tree(cwd: &Path) -> Option<Arc<Self>> {
        let mut lanes = LANES.lock().ok()?;
        let lanes = lanes.get_or_insert_with(HashMap::new);
        lanes
            .entry(cwd.to_path_buf())
            .or_insert_with(|| Self::open(cwd, FORWARD_TIMEOUT).map(Arc::new))
            .clone()
    }

    /// What a contained git gets where a repository has an origin and no lane: no transport but a
    /// local path. Without it, git reads a credential from the URL and lands on main through a
    /// seeded host, asked by nobody. Any remote counts, by whatever name; a git that cannot say
    /// whether one exists gets the same, never fail open.
    pub(crate) fn assign_for(lane: Option<&Self>, cwd: &Path) -> Vec<String> {
        static NO_LANE: Mutex<Option<HashMap<PathBuf, Vec<String>>>> = Mutex::new(None);
        if let Some(lane) = lane {
            return lane.assign.clone();
        }
        // Only a found remote is kept: a kernel restarts when its block changes, and a remote
        // added later must close the transport on the next call, not on the next process.
        let mut memo = NO_LANE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let memo = memo.get_or_insert_with(HashMap::new);
        if let Some(block) = memo.get(cwd) {
            return block.clone();
        }
        let remote = git(cwd, &["remote"]).map_or(true, |list| !list.trim().is_empty());
        if !remote {
            return Vec::new();
        }
        let block = config_env(&[
            ("maintenance.auto".to_owned(), "false".to_owned()),
            ("protocol.allow".to_owned(), "never".to_owned()),
            ("protocol.file.allow".to_owned(), "always".to_owned()),
        ]);
        memo.insert(cwd.to_path_buf(), block.clone());
        block
    }

    /// One repository's lane, its pushes cut at `forward_timeout`.
    #[must_use]
    pub fn open(cwd: &Path, forward_timeout: Duration) -> Option<Self> {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        let pool = Pool::open(&home, cwd, 1).ok()?;
        let dirs = yi_permission::git_dirs(cwd);
        let (admin, common) = (dirs.first()?.clone(), dirs.last()?.clone());
        let config = |key: &str| {
            git(pool.repo(), &["config", "--get", key])
                .ok()
                .map(|value| value.trim().to_owned())
        };
        let url = config("remote.origin.url").filter(|url| !carries_credentials(url))?;
        let pushurl = config("remote.origin.pushurl");
        if pushurl.as_deref().is_some_and(carries_credentials) {
            return None;
        }
        let mirror = pool.dir().join("mirror.git");
        let outbox = admin.join("yi-outbox");
        let objects = common.join("objects");
        init_store(pool.repo(), &mirror, &objects).ok()?;
        yi_permission::outbox::init_store(&outbox, &objects).ok()?;
        let mut rules = vec![
            ("maintenance.auto".to_owned(), "false".to_owned()),
            (
                "remote.origin.followRemoteHEAD".to_owned(),
                "never".to_owned(),
            ),
            (format!("url.{}.insteadOf", mirror.display()), url.clone()),
        ];
        rules.push(match pushurl {
            Some(pushurl) => (format!("url.{}.insteadOf", outbox.display()), pushurl),
            None => (format!("url.{}.pushInsteadOf", outbox.display()), url),
        });
        let assign = config_env(&rules);
        let lane = Self {
            pool,
            mirror,
            outbox,
            assign,
            forward_timeout,
        };
        let _an_empty_mirror_says_so_at_the_fetch = lane.sync_mirror();
        Some(lane)
    }

    /// `KEY=VALUE` for `env`'s argv, past its scrub: paths and an origin with no credentials.
    #[must_use]
    pub fn assign(&self) -> &[String] {
        &self.assign
    }

    fn sync_mirror(&self) -> Result<(), LaneError> {
        let repo = self.pool.repo().to_string_lossy().into_owned();
        let refspec = "+refs/remotes/origin/*:refs/heads/*";
        git(
            &self.mirror,
            &["fetch", "-q", "--prune", "--no-tags", &repo, refspec],
        )
        .map(drop)
    }

    /// Before a spawn whose text reads origin. A script that fetches gets what the last refresh
    /// saw: nothing here reads a script.
    pub(crate) fn refresh(&self, command: &str) -> Vec<String> {
        if !reads_a_remote(command) {
            return Vec::new();
        }
        let repo = self.pool.repo();
        let mut rows = Vec::new();
        if let Err(error) = git(repo, &["fetch", "-q", "--prune", "origin"]) {
            let text = fenced(&error.to_string());
            rows.push(format!(
                "[git: origin not refreshed, so this read what the last fetch saw: {text}]"
            ));
        }
        let _a_failed_set_head_keeps_the_old_trunk =
            git(repo, &["remote", "set-head", "origin", "--auto"]);
        if let Err(error) = self.sync_mirror() {
            rows.push(format!(
                "[git: mirror not refreshed: {}]",
                fenced(&error.to_string())
            ));
        }
        rows
    }

    /// After the spawn. Git never runs in the outbox, which a contained process wrote.
    pub fn sweep(&self, broker: &PermissionBroker) -> Vec<String> {
        let taken = yi_permission::outbox::take_refs(&self.outbox, OUTBOX_REFS_MAX);
        let mut rows = Vec::new();
        if taken.refused > 0 {
            rows.push(format!(
                "[not pushed: {} entries in the outbox were no ref (a link, a FIFO or a directory too deep)]",
                taken.refused
            ));
        }
        for (name, value) in &taken.refs {
            match outbox_ref(name, value) {
                Some((wanted, sha)) => rows.push(self.forward(&wanted, &sha, broker)),
                None => {
                    let shown: String = String::from_utf8_lossy(name)
                        .chars()
                        .filter(|ch| ch.is_ascii_alphanumeric() || "._/-".contains(*ch))
                        .take(64)
                        .collect();
                    rows.push(format!(
                        "[not pushed: `{shown}` is no branch or tag at a commit]"
                    ));
                }
            }
        }
        if taken.past_cap {
            rows.push(format!(
                "[{OUTBOX_REFS_MAX} refs forwarded, the rest wait in the outbox (outbox.refs {OUTBOX_REFS_MAX}); any next command forwards them]"
            ));
        }
        if taken.past_names {
            let cap = yi_permission::outbox::NAMES_PER_DIR_MAX;
            rows.push(format!(
                "[{cap} names read from one outbox directory that holds more, the rest wait (outbox.names {cap}); any next command reads them]"
            ));
        }
        rows
    }

    /// Off the executor: a cell's push takes up to its timeout per ref, and a held main waits on a
    /// person, whose answer a current-thread front end has to be free to deliver.
    pub async fn sweep_off_executor(self: Arc<Self>, broker: Arc<PermissionBroker>) -> Vec<String> {
        crate::lease::off_executor("outbox sweep", move || Ok(self.sweep(&broker)))
            .await
            .unwrap_or_else(|error| vec![format!("[outbox not swept: {error}]")])
    }

    fn forward(&self, wanted: &Wanted, sha: &Sha, broker: &PermissionBroker) -> String {
        let repo = self.pool.repo();
        let spec = format!("{}^{{commit}}", sha.as_str());
        if git(repo, &["cat-file", "-e", &spec]).is_err() {
            return format!(
                "[not pushed: {} is no commit in this repository]",
                sha.as_str()
            );
        }
        let (target, label) = match wanted {
            Wanted::Branch(branch) => (format!("refs/heads/{branch}"), branch.to_string()),
            Wanted::Tag(tag) => (format!("refs/tags/{tag}"), format!("tag {tag}")),
        };
        let held = match wanted {
            Wanted::Tag(_) => true,
            Wanted::Branch(branch) => {
                *branch == trunk(repo) || matches!(branch.as_str(), "main" | "master")
            }
        };
        let stat = if held {
            self.diffstat(wanted, sha)
        } else {
            String::new()
        };
        if held && !asked(&label, &stat, broker) {
            return format!(
                "[not pushed: {label} asks a person, who declined or is not here: {stat}]"
            );
        }
        match self.push(wanted, sha, &target) {
            Forward::Pushed { old } => {
                let from =
                    old.map_or_else(|| "new ".to_owned(), |old| format!("{}..", old.as_str()));
                format!(
                    "[pushed {label} to origin: {from}{}; Yi set origin/{label} and the upstream itself, so the client's errors writing them mean nothing]",
                    sha.as_str()
                )
            }
            Forward::Behind => format!(
                "[not pushed: origin/{label} has commits this lacks; merge, or git push --force asks]"
            ),
            Forward::Rejected(text) => format!("[not pushed: {text}]"),
            Forward::Unknown => format!(
                "[push of {label} unknown after {} s: it may have landed; git fetch and compare before pushing again]",
                self.forward_timeout.as_secs()
            ),
        }
    }

    fn diffstat(&self, wanted: &Wanted, sha: &Sha) -> String {
        let Wanted::Branch(branch) = wanted else {
            return "a tag".to_owned();
        };
        let range = format!("refs/remotes/origin/{branch}...{}", sha.as_str());
        let Ok(rows) = git(self.pool.repo(), &["diff", "--numstat", &range]) else {
            return "no diff against origin to show".to_owned();
        };
        let (mut added, mut removed, mut files) = (0u64, 0u64, 0u64);
        for row in rows.lines() {
            let mut cells = row.split('\t');
            let count =
                |cell: Option<&str>| cell.and_then(|cell| cell.parse::<u64>().ok()).unwrap_or(0);
            added = added.saturating_add(count(cells.next()));
            removed = removed.saturating_add(count(cells.next()));
            files = files.saturating_add(1);
        }
        format!("+{added} -{removed} in {files} files")
    }

    fn push(&self, wanted: &Wanted, sha: &Sha, target: &str) -> Forward {
        let repo = self.pool.repo();
        let tracking = match wanted {
            Wanted::Branch(branch) => format!("refs/remotes/origin/{branch}"),
            Wanted::Tag(_) => String::new(),
        };
        let old = git(repo, &["rev-parse", "--verify", "-q", &tracking])
            .ok()
            .and_then(|text| Sha::parse(&text));
        let deadline = Instant::now().checked_add(self.forward_timeout);
        let refspec = format!("{}:{target}", sha.as_str());
        let args = ["push", "-q", "origin", refspec.as_str()];
        match git_capture(repo, &args, deadline, CAPTURE_CAP) {
            Ok(_) => {
                if let Wanted::Branch(branch) = wanted {
                    let upstream = format!("origin/{branch}");
                    let local = branch.to_string();
                    let _no_local_branch_has_no_upstream =
                        git(repo, &["branch", "--set-upstream-to", &upstream, &local]);
                }
                let _the_next_fetch_syncs_it = self.sync_mirror();
                Forward::Pushed { old }
            }
            Err(LaneError::Git {
                exit_code: None, ..
            }) => {
                let _whatever_landed_is_read_at_the_next_fetch = self.sync_mirror();
                Forward::Unknown
            }
            Err(LaneError::Git { output, .. })
                if output.contains("non-fast-forward") || output.contains("fetch first") =>
            {
                Forward::Behind
            }
            Err(error) => Forward::Rejected(fenced(&error.to_string())),
        }
    }
}

fn asked(label: &str, stat: &str, broker: &PermissionBroker) -> bool {
    let description = format!("A contained command pushed to {label} on origin: {stat}");
    let ask = PermissionAsk {
        title: "git push",
        description: &description,
        patch: None,
        changes: &[],
        grants: &[],
        tool_call_id: None,
    };
    let (outcome, _classifier) = broker.confirm_judged(&ask, None);
    !matches!(outcome, AskOutcome::Reject)
}

/// A remote's text is the network's: redacted, fenced and cut where the model reads it.
fn fenced(text: &str) -> String {
    let total = text.chars().count();
    let kept: String = text.chars().take(REMOTE_TEXT_CHARS_MAX).collect();
    let cut = (total > REMOTE_TEXT_CHARS_MAX).then(|| {
        format!(
            "\n[… kept {REMOTE_TEXT_CHARS_MAX} of {total} chars (git.remote_text {REMOTE_TEXT_CHARS_MAX}); the same push in a terminal prints the rest]"
        )
    });
    let mut redacted = Vec::new();
    if let Some(mut redactor) = yi_tools::Redactor::new() {
        redactor.push(kept.as_bytes(), &mut redacted);
        redactor.finish(&mut redacted);
    }
    let fence =
        crate::fetch::fence_untrusted("git origin", &String::from_utf8_lossy(&redacted), None);
    format!("{fence}{}", cut.unwrap_or_default())
}

fn init_store(repo: &Path, store: &Path, objects: &Path) -> Result<(), LaneError> {
    if !store.join("HEAD").is_file() {
        let at = store.to_string_lossy().into_owned();
        git(repo, &["init", "-q", "--bare", &at])?;
    }
    write_alternates(store, objects)
}

fn config_env(rules: &[(String, String)]) -> Vec<String> {
    std::iter::once(format!("GIT_CONFIG_COUNT={}", rules.len()))
        .chain(rules.iter().enumerate().flat_map(|(at, (key, value))| {
            [
                format!("GIT_CONFIG_KEY_{at}={key}"),
                format!("GIT_CONFIG_VALUE_{at}={value}"),
            ]
        }))
        .collect()
}

fn write_alternates(store: &Path, objects: &Path) -> Result<(), LaneError> {
    let io = |error| super::io_error(store)(error);
    std::fs::create_dir_all(store.join("objects/info")).map_err(io)?;
    let alternates = store.join("objects/info/alternates");
    std::fs::write(alternates, format!("{}\n", objects.display())).map_err(io)
}

/// Invariant: a word of the command is `git` and another is the remote read, so a file named
/// `fetch.log` or a `grep pull` pays for no fetch it never asked for.
fn reads_a_remote(command: &str) -> bool {
    let words: Vec<&str> = command
        .split(|c: char| c.is_whitespace() || ";&|()<>!".contains(c))
        .collect();
    let git = words
        .iter()
        .any(|word| *word == "git" || word.ends_with("/git"));
    let read = words
        .iter()
        .any(|word| matches!(*word, "fetch" | "pull" | "ls-remote"));
    git && read
}

#[cfg(test)]
mod tests {
    use super::{GitLane, git};

    /// Dies with the origin-only key: a fork whose remote is `upstream` gets no transport either,
    /// and the block still permits a local path, which reaches no network.
    #[test]
    fn any_remote_closes_the_transport() -> Result<(), String> {
        let bare = std::env::temp_dir().join("yi-assign-no-remote");
        let forked = std::env::temp_dir().join("yi-assign-upstream");
        for dir in [&bare, &forked] {
            let _ = std::fs::remove_dir_all(dir);
            std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
            git(dir, &["init", "-q"]).map_err(|error| error.to_string())?;
        }
        git(
            &forked,
            &["remote", "add", "upstream", "https://example.invalid/r"],
        )
        .map_err(|error| error.to_string())?;
        assert_eq!(GitLane::assign_for(None, &bare), Vec::<String>::new());
        git(
            &bare,
            &["remote", "add", "later", "https://example.invalid/r"],
        )
        .map_err(|error| error.to_string())?;
        assert!(!GitLane::assign_for(None, &bare).is_empty());
        let assign = GitLane::assign_for(None, &forked);
        assert!(
            assign.contains(&"GIT_CONFIG_VALUE_1=never".to_owned()),
            "{assign:?}"
        );
        assert!(
            assign.contains(&"GIT_CONFIG_KEY_2=protocol.file.allow".to_owned()),
            "{assign:?}"
        );
        Ok(())
    }

    /// Dies with the substring test: a file whose name holds the verb costs no fetch.
    #[test]
    fn only_a_git_word_fetches() {
        assert!(super::reads_a_remote("git fetch origin"));
        assert!(super::reads_a_remote("cargo nextest && git pull --ff-only"));
        assert!(!super::reads_a_remote(
            "grep -n pull crates/runtime/src/lane/mod.rs"
        ));
        assert!(!super::reads_a_remote("tail -20 git fetch.log"));
        for punctuated in [
            "git fetch; git log",
            "(git fetch)",
            "!git fetch",
            "git fetch>log",
        ] {
            assert!(super::reads_a_remote(punctuated), "{punctuated}");
        }
    }
}
