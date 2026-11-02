//! The kernel's filesystem noun: `tree.*` requests over a name a kernel gives, which only ever keys
//! the table. A kernel is a contained process, so every payload here is peer data.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use super::SubagentHost;
use crate::lane::fork::{Carry, ChildTree};
use crate::lane::warm::{self, Asked, Verdict};
use crate::lane::{Pool, Sha, tree as ops};
use crate::lease::off_executor;

const ROWS_SHOWN: usize = 8;
const LOG_DEFAULT: u64 = 20;
const LOG_MAX: u64 = 256;
const PATCH_CAP: usize = 16 << 10;
const NAME_MAX: usize = 64;
const CHECK_WAIT_DEFAULT: Duration = Duration::from_secs(30);
const CHECK_WAIT_MAX: Duration = Duration::from_secs(590);
const SELF: &str = "self";

type Free = HashMap<(PathBuf, String), ChildTree>;

/// Trees no child holds yet, by owning session's checkout and name.
fn free() -> &'static Mutex<Free> {
    static FREE: OnceLock<Mutex<Free>> = OnceLock::new();
    FREE.get_or_init(Mutex::default)
}

/// `f.bash`: a free tree this session forked, for the kernel's `exec.spawn`. `None` is a refusal.
pub(crate) fn free_tree_path(session_cwd: &Path, name: &str) -> Option<PathBuf> {
    let trees = free().lock().unwrap_or_else(PoisonError::into_inner);
    let key = (session_cwd.to_path_buf(), name.to_owned());
    trees.get(&key).map(|tree| tree.path().to_path_buf())
}

fn checked_name(name: &str) -> Result<&str, String> {
    let fine = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-';
    let good = !name.is_empty() && name.len() <= NAME_MAX && name.bytes().all(fine);
    good.then_some(name)
        .ok_or_else(|| format!("a tree's name is 1 to {NAME_MAX} of letters, digits, `_` and `-`"))
}

fn text<'a>(payload: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("tree request needs a \"{key}\" string"))
}

/// A list of names or paths from a kernel, bounded as `carry` is.
fn strings(payload: &Map<String, Value>, key: &str) -> Result<Vec<String>, String> {
    const MAX: usize = 256;
    let Some(value) = payload.get(key).filter(|value| !value.is_null()) else {
        return Ok(Vec::new());
    };
    let list = value
        .as_array()
        .ok_or_else(|| format!("{key} must be a list"))?;
    if list.len() > MAX {
        return Err(format!(
            "{key} names {} entries; the cap is {MAX}",
            list.len()
        ));
    }
    (list.iter())
        .map(|entry| {
            entry
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{key} entries must be strings"))
        })
        .collect()
}

/// What a tree is measured from: a fork's cut, or `HEAD` for the session's own.
fn base_of(path: &Path) -> Result<Sha, String> {
    let head = crate::lane::hostgit::git(path, &["rev-parse", "HEAD"])
        .map_err(|error| error.to_string())?;
    Sha::parse(&head).ok_or_else(|| format!("HEAD is no commit: {head}"))
}

impl SubagentHost {
    fn free_key(&self, name: &str) -> (PathBuf, String) {
        (self.options.cwd.clone(), name.to_owned())
    }

    fn tree_path(&self, name: &str) -> Result<PathBuf, String> {
        if name == SELF {
            return Ok(self.options.cwd.clone());
        }
        checked_name(name)?;
        let trees = free().lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(tree) = trees.get(&self.free_key(name)) {
            return Ok(tree.path().to_path_buf());
        }
        drop(trees);
        let child = self.cwd_of(name).filter(|path| *path != self.options.cwd);
        child.ok_or_else(|| format!("no tree named \"{name}\""))
    }

    /// Where `name` was cut: a free tree's base, a child's from its branch, the session's `HEAD`.
    fn tree_base(&self, name: &str, path: &Path) -> Result<Sha, String> {
        let trees = free().lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(tree) = trees.get(&self.free_key(name)) {
            return Ok(tree.base().clone());
        }
        drop(trees);
        let children = self
            .children
            .lock()
            .map_err(|_| "subagent state poisoned")?;
        let child = Self::key_of(&children, name)
            .ok()
            .and_then(|key| children.get(&key));
        let cut = child
            .and_then(|child| child.worktree.as_ref())
            .map(|tree| tree.base().clone());
        drop(children);
        cut.map_or_else(|| base_of(path), Ok)
    }

    /// `t.fork`: dirt and all, since a fork of a tree is that tree; `at` cuts at a tree id instead.
    fn tree_fork(&self, payload: &Map<String, Value>) -> Result<Map<String, Value>, String> {
        let from = payload.get("from").and_then(Value::as_str).unwrap_or(SELF);
        let source = self.tree_path(from)?;
        let name = match payload.get("name").and_then(Value::as_str) {
            Some(name) => checked_name(name)?.to_owned(),
            None => format!("t{}", super::random_suffix()?),
        };
        if name == SELF
            || free()
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains_key(&self.free_key(&name))
        {
            return Err(format!("a tree named \"{name}\" already exists"));
        }
        let at = payload.get("at").and_then(Value::as_str);
        let at = at
            .map(|at| Sha::parse(at).ok_or_else(|| format!("at must be a tree id, got {at}")))
            .transpose()?;
        let id = format!("fork-{}", super::random_suffix()?);
        let plan = Carry {
            extra: strings(payload, "carry")?,
            denied: yi_permission::credential_stores(&self.options.home),
            first_use: true,
            dirt: at.is_none(),
            at,
        };
        let pool = Pool::open(&self.options.home, &source, self.options.lane_slots)
            .map_err(|error| error.to_string())?;
        let (tree, rows) =
            ChildTree::fork(&pool, &source, &id, &plan).map_err(|error| error.to_string())?;
        let mut reply = Map::new();
        reply.insert("name".to_owned(), json!(name));
        reply.insert("base".to_owned(), json!(tree.base().as_str()));
        reply.insert("next".to_owned(), json!(rows.join("\n")));
        free()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(self.free_key(&name), tree);
        Ok(reply)
    }

    fn tree_head(&self, payload: &Map<String, Value>) -> Result<Map<String, Value>, String> {
        let path = self.tree_path(text(payload, "tree")?)?;
        let tree = warm::snapshot(&path).map_err(|error| error.to_string())?;
        let mut reply = Map::new();
        reply.insert("tree".to_owned(), json!(tree.as_str()));
        Ok(reply)
    }

    fn tree_log(&self, payload: &Map<String, Value>) -> Result<Map<String, Value>, String> {
        let name = text(payload, "tree")?;
        let path = self.tree_path(name)?;
        let limit = payload
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(LOG_DEFAULT)
            .min(LOG_MAX);
        let count = limit.to_string();
        let log = crate::lane::hostgit::git(&path, &["log", "-n", &count, "--format=%h %s"]);
        let mut rows: Vec<String> = log
            .map_err(|error| error.to_string())?
            .lines()
            .map(str::to_owned)
            .collect();
        if u64::try_from(rows.len()).is_ok_and(|shown| shown == limit) {
            rows.push(yi_tools::cut_row(
                rows.len(),
                rows.len().saturating_add(1),
                "commits or more",
                "limit",
                rows.len(),
                &format!("t.log(limit={})", limit.saturating_mul(2).min(LOG_MAX)),
            ));
        }
        rows.extend(crate::checkpoint::snapshots_off_row(&path));
        let mut reply = Map::new();
        reply.insert("rows".to_owned(), json!(rows));
        Ok(reply)
    }

    /// The file rows a `tree://<name>/` serves whole, one per line.
    pub(crate) fn tree_rows(&self, name: &str) -> Result<Vec<String>, String> {
        let path = self.tree_path(name)?;
        let base = self.tree_base(name, &path)?;
        ops::diff_rows(&path, &base).map_err(|error| error.to_string())
    }

    fn tree_diff(&self, payload: &Map<String, Value>) -> Result<Map<String, Value>, String> {
        let name = text(payload, "tree")?;
        let path = self.tree_path(name)?;
        let against = match payload.get("against").and_then(Value::as_str) {
            Some(at) => {
                Sha::parse(at).ok_or_else(|| format!("against must be a tree id, got {at}"))?
            }
            None => self.tree_base(name, &path)?,
        };
        let mut reply = Map::new();
        if let Some(file) = payload.get("path").and_then(Value::as_str) {
            let patch = ops::patch(&path, &against, file).map_err(|error| error.to_string())?;
            let (shown, cut) = cut_patch(patch, name, file);
            reply.insert("patch".to_owned(), json!(shown));
            if let Some(cut) = cut {
                reply.insert("cut".to_owned(), json!(cut));
            }
            return Ok(reply);
        }
        let rows = ops::diff_rows(&path, &against).map_err(|error| error.to_string())?;
        let total = rows.len();
        let offset: usize = rows
            .iter()
            .take(ROWS_SHOWN)
            .map(|row| row.len().saturating_add(1))
            .sum();
        let mut shown: Vec<String> = rows.into_iter().take(ROWS_SHOWN).collect();
        if total > ROWS_SHOWN {
            let next = format!("read(\"tree://{name}/\", offset={offset})");
            shown.push(yi_tools::cut_row(
                ROWS_SHOWN,
                total,
                "files",
                "tree.rows",
                ROWS_SHOWN,
                &next,
            ));
        }
        reply.insert("files".to_owned(), json!(shown));
        reply.insert(
            "summary".to_owned(),
            json!(match total {
                1 => "1 file differs".to_owned(),
                _ => format!("{total} files differ"),
            }),
        );
        // Invariant: null when the trial failed, so an unknown cannot read as no conflict.
        reply.insert(
            "would_conflict".to_owned(),
            json!(ops::would_conflict(
                &path,
                &self.options.cwd,
                &self.tree_base(name, &path)?
            )),
        );
        Ok(reply)
    }

    fn tree_check(&self, payload: &Map<String, Value>) -> Result<Map<String, Value>, String> {
        let name = text(payload, "tree")?;
        let path = self.tree_path(name)?;
        let command = text(payload, "cmd")?.to_owned();
        let wait = payload
            .get("wait_s")
            .and_then(Value::as_f64)
            .filter(|wait| wait.is_finite() && *wait >= 0.0);
        let wait = wait
            .map_or(CHECK_WAIT_DEFAULT, Duration::from_secs_f64)
            .min(CHECK_WAIT_MAX);
        if let Some(refused) = self.check_gate.get().and_then(|gates| gates.1(&command)) {
            return Err(format!("the check is refused: {refused}"));
        }
        let key = path.to_string_lossy().into_owned();
        let pool = Pool::open(&self.options.home, &path, self.options.lane_slots)
            .map_err(|error| error.to_string())?;
        let warm_pool = pool.warm().map_err(|error| error.to_string())?;
        let commit = warm::snapshot(&path).map_err(|error| error.to_string())?;
        let checker = self.checker();
        let until = self
            .deadline()
            .or_else(|| Instant::now().checked_add(warm::CHECK_QUEUE))
            .unwrap_or_else(Instant::now);
        let budget = Duration::from_millis(crate::goal::DEFAULT_CHECK_TIMEOUT_MS);
        let cmd = command.clone();
        let shown_as = path.clone();
        let asked = warm::ticket(&key, &command, wait, move || {
            warm::check_commit(
                &warm_pool, &commit, &shown_as, &cmd, until, budget, &checker,
            )
        });
        let mut reply = Map::new();
        match asked {
            Asked::Waiting { row } => {
                reply.insert("state".to_owned(), json!("running"));
                reply.insert("next".to_owned(), json!(row));
            }
            Asked::Finished(run) => {
                let (state, exit, why) = match &run.verdict {
                    Verdict::Green => ("green", Some(0), None),
                    Verdict::Red { exit } => ("red", *exit, None),
                    Verdict::Abstain { why } => ("abstain", None, Some(why.clone())),
                };
                reply.insert("state".to_owned(), json!(state));
                reply.insert("exit_code".to_owned(), json!(exit));
                reply.insert("output".to_owned(), json!(run.output()));
                reply.insert("waited_s".to_owned(), json!(run.waited.as_secs_f64()));
                reply.insert("ran_s".to_owned(), json!(run.ran.as_secs_f64()));
                if let Some(why) = why {
                    reply.insert("why".to_owned(), json!(why));
                }
            }
        }
        Ok(reply)
    }

    fn tree_merge(&self, payload: &Map<String, Value>) -> Result<Map<String, Value>, String> {
        let ours = self.tree_path(text(payload, "tree")?)?;
        let others = strings(payload, "others")?;
        let (mut merged, mut conflicts) = (Vec::new(), Vec::new());
        for other in &others {
            let path = self.tree_path(other)?;
            let base = self.tree_base(other, &path)?;
            let fold = ops::fold(&ours, &path, &base).map_err(|error| error.to_string())?;
            if fold.merged {
                merged.push(other.clone());
            }
            conflicts.extend(
                fold.conflicts
                    .into_iter()
                    .map(|file| format!("{other}: {file}")),
            );
        }
        let total = others.len();
        let mut reply = Map::new();
        reply.insert("merged".to_owned(), json!(merged));
        reply.insert(
            "summary".to_owned(),
            json!(format!("merged {} of {total}", merged.len())),
        );
        let shown: Vec<&String> = conflicts.iter().take(ROWS_SHOWN).collect();
        reply.insert("conflicts".to_owned(), json!(shown));
        if conflicts.len() > ROWS_SHOWN {
            let cut = yi_tools::cut_row(
                ROWS_SHOWN,
                conflicts.len(),
                "conflicted files",
                "tree.rows",
                ROWS_SHOWN,
                "t.diff(path=...) on each side",
            );
            reply.insert("cut".to_owned(), json!(cut));
        }
        if !conflicts.is_empty() {
            reply.insert("next".to_owned(), json!("a tree that conflicted moved nothing here: read both sides with t.diff, change one, and merge again"));
        }
        Ok(reply)
    }

    fn tree_rewind(&self, payload: &Map<String, Value>) -> Result<Map<String, Value>, String> {
        let path = self.tree_path(text(payload, "tree")?)?;
        let to = text(payload, "to")?;
        let to = Sha::parse(to).ok_or_else(|| format!("to must be a tree id, got {to}"))?;
        ops::rewind(&path, &to, &strings(payload, "paths")?).map_err(|error| error.to_string())?;
        Ok(Map::new())
    }

    /// A free tree's only. A child's is handed back through `merge_worktree` or `discard_worktree`.
    fn tree_discard(&self, payload: &Map<String, Value>) -> Result<Map<String, Value>, String> {
        let name = text(payload, "tree")?;
        checked_name(name)?;
        let taken = free()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.free_key(name));
        let tree = taken.ok_or_else(|| {
            format!(
                "no free tree named \"{name}\": a child's goes back through rlm.discard_worktree"
            )
        })?;
        let key = tree.path().to_string_lossy().into_owned();
        warm::forget_tickets(&key);
        let stopped = yi_tools::jobs::registry().kill_under(tree.path());
        let until = Instant::now().checked_add(Duration::from_secs(5));
        let busy = |path: &Path| {
            !crate::lane::settle::quiescence_of_jobs(path)
                .running
                .is_empty()
        };
        while busy(tree.path()) && until.is_some_and(|until| Instant::now() < until) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let quiescence = crate::lane::settle::quiescence_of_jobs(tree.path());
        let settled = tree
            .settle(&quiescence, self.deadline())
            .map_err(|(_kept, error)| error.to_string())?;
        settled.discard().map_err(|error| error.to_string())?;
        let mut reply = Map::new();
        if !stopped.is_empty() {
            reply.insert(
                "stopped".to_owned(),
                json!(format!(
                    "stopped {} jobs under the tree: {}; a process that left its job keeps running",
                    stopped.len(),
                    stopped.join(", ")
                )),
            );
        }
        Ok(reply)
    }

    /// `tree="name"` on `rlm.run`: the child takes that tree. A plain fork is `t.fork()`.
    pub(super) fn take_free_tree(&self, name: &str) -> Result<ChildTree, String> {
        checked_name(name)?;
        let mut trees = free().lock().unwrap_or_else(PoisonError::into_inner);
        trees
            .remove(&self.free_key(name))
            .ok_or_else(|| format!("no free tree named \"{name}\""))
    }

    pub(super) fn register_tree(self: &Arc<Self>, registry: &mut crate::kernel::HostRegistry) {
        type Verb = fn(&SubagentHost, &Map<String, Value>) -> Result<Map<String, Value>, String>;
        let verbs: [(&'static str, Verb); 8] = [
            ("tree.fork", Self::tree_fork),
            ("tree.head", Self::tree_head),
            ("tree.log", Self::tree_log),
            ("tree.diff", Self::tree_diff),
            ("tree.check", Self::tree_check),
            ("tree.merge", Self::tree_merge),
            ("tree.rewind", Self::tree_rewind),
            ("tree.discard", Self::tree_discard),
        ];
        for (method, verb) in verbs {
            let host = Arc::clone(self);
            registry.register(method, move |payload| {
                let host = Arc::clone(&host);
                Box::pin(off_executor(method, move || verb(&host, &payload)))
            });
        }
    }
}

/// A patch past its cap, cut on a character and named at the cut.
fn cut_patch(patch: String, name: &str, file: &str) -> (String, Option<String>) {
    if patch.len() <= PATCH_CAP {
        return (patch, None);
    }
    let total = patch.len();
    let mut at = PATCH_CAP;
    while !patch.is_char_boundary(at) {
        at = at.saturating_sub(1);
    }
    let shown: String = patch.get(..at).unwrap_or_default().to_owned();
    let next = format!("read(\"tree://{name}/{file}\")");
    (
        shown,
        Some(yi_tools::cut_row(
            at,
            total,
            "bytes of the patch",
            "tree.patch",
            PATCH_CAP,
            &next,
        )),
    )
}

impl crate::fetch::MemberTrees for SubagentHost {
    fn cwd_of(&self, agent: &str) -> Option<PathBuf> {
        SubagentHost::cwd_of(self, agent).or_else(|| free_tree_path(&self.options.cwd, agent))
    }

    fn diff_rows(&self, agent: &str) -> Result<Vec<String>, String> {
        self.tree_rows(agent)
    }
}
