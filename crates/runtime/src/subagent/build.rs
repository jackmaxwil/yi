use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use yi_types::message::{AgentMessage, Content};

use super::{ChildActivity, ChildBuild, Standing, SubagentHost};
use crate::mailbox::ParentLink;
use crate::plan::ops::{ENGINE_AGENT, OWNER_AGENT};
use crate::provider::resolve_model;
use yi_types::model::{Effort, Model};

/// What a build is made from, read off the spawn's kwargs: the model, the effort, the wall.
pub(crate) type Cast = (
    Model,
    Effort,
    crate::wall::Wall,
    Option<super::reader::Reader>,
);

/// Dropped, it frees the name and the slot a build reserved, whether the build landed or not.
pub(super) struct Reservation<'a> {
    host: &'a SubagentHost,
    name: String,
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        if let Ok(mut children) = self.host.children.lock() {
            children.release_build(&self.name);
        }
    }
}

impl SubagentHost {
    /// A child's tree branches from the parent's own HEAD, so its merge lands back in the parent's
    /// checkout rather than on `main`. No slot and no permit: family caps bound forks.
    pub(super) fn fork_child_tree(
        &self,
        child_id: &str,
        kwargs: &Map<String, Value>,
        contained_in_image: bool,
    ) -> Result<(crate::lane::fork::ChildTree, Vec<String>), String> {
        let pool = crate::lane::Pool::open(
            &self.options.home,
            &self.options.cwd,
            self.options.lane_slots,
        )
        .map_err(|error| error.to_string())?;
        if let Some(name) = kwargs.get("tree").and_then(Value::as_str) {
            return self.take_free_tree(name).map(|tree| (tree, Vec::new()));
        }
        let denied = yi_permission::credential_stores(&self.options.home);
        let plan = crate::lane::fork::Carry {
            extra: parse_carry(kwargs)?,
            denied,
            first_use: !contained_in_image,
            ..crate::lane::fork::Carry::default()
        };
        crate::lane::fork::ChildTree::fork(&pool, &self.options.cwd, child_id, &plan)
            .map_err(|error| error.to_string())
    }

    /// Invariant: the roster lock is never held across a build, so `states` and `list` answer
    /// while a factory runs; the name and the slot are reserved under it instead.
    pub(super) fn reserve(
        &self,
        session_name: &str,
        ask: &crate::lease::Ask,
        standing: &Standing,
    ) -> Result<(Reservation<'_>, yi_types::lease::Lease), String> {
        let mut children = self
            .children
            .lock()
            .map_err(|_| "subagent state poisoned")?;
        // A service is never reaped while it serves, so it neither fills the worker cap nor is
        // refused by it.
        let building_readers = children
            .building
            .iter()
            .filter(|(.., reader)| *reader)
            .count();
        let workers = children
            .values()
            .filter(|record| matches!(record.standing, Standing::Worker))
            .count()
            .saturating_add(children.building.len().saturating_sub(building_readers));
        let reserved = [OWNER_AGENT, ENGINE_AGENT, "host"];
        let refusal = if reserved.contains(&session_name) {
            Some(format!(
                "\"{session_name}\" is reserved: it names the plan's owner, engine or host; pick another name"
            ))
        } else if matches!(standing, Standing::Worker) && workers >= self.options.max_children {
            Some(format!(
                "RLM child limit reached ({} children retained); rlm.delete_subagent a finished child first",
                self.options.max_children
            ))
        } else if children
            .values()
            .map(|record| &record.session_name)
            .chain(children.building.iter().map(|(name, ..)| name))
            .any(|name| name == session_name)
        {
            Some(format!(
                "Agent session name \"{session_name}\" is already taken at depth {}",
                self.options.depth.saturating_add(1)
            ))
        } else {
            None
        };
        let lease = match refusal {
            None => self.draw(&children, session_name, ask),
            Some(refusal) => Err(refusal),
        }?;
        let tokens = lease.tokens.unwrap_or(0);
        let reader = matches!(standing, Standing::Reader);
        children
            .building
            .push((session_name.to_owned(), tokens, reader));
        let reservation = Reservation {
            host: self,
            name: session_name.to_owned(),
        };
        Ok((reservation, lease))
    }

    pub(super) fn cast(
        &self,
        kwargs: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Cast, String> {
        let thinking = super::optional_string(kwargs, "thinking")?
            .map(|level| level.parse::<Effort>())
            .transpose()
            .map_err(|error| error.to_string())?;
        let (parent_model, parent_effort) = (self.options.defaults)();
        let model = match super::optional_string(kwargs, "model")? {
            None => parent_model,
            Some(selector) => {
                let (provider, id) = selector.split_once('/').ok_or_else(|| {
                    format!("model selector must be provider/model, got {selector}")
                })?;
                resolve_model(provider, id)
                    .ok_or_else(|| format!("no model matches selector {selector}"))?
            }
        };
        // Before a lane, a container or a lease is taken: the child would stream with no key.
        if !self.options.provider.has_credential(&model.provider) {
            return Err(yi_ai::auth::missing_message(&model.provider));
        }
        let wall = self.wall_for(kwargs)?;
        let reader = super::reader::parse(kwargs)?;
        Ok((model, thinking.unwrap_or(parent_effort), wall, reader))
    }

    /// Invariant: a child that runs unrecorded leaves nothing to read when it fails, so a
    /// transcript it cannot open refuses the build. `kept` is a respawned service's own.
    pub(crate) fn build(
        self: &std::sync::Arc<Self>,
        (model, thinking, wall, reader): Cast,
        name: &str,
        session_dir: &Path,
        cwd: Option<&Path>,
        lease: &yi_types::lease::Lease,
        kept: Option<yi_session::SharedSession>,
    ) -> Result<crate::session::AgentSession, String> {
        let clock = lease
            .deadline_ms
            .map(|ends| std::time::Duration::from_millis(ends.saturating_sub(lease.granted_at)));
        let child = (self.options.factory)(ChildBuild {
            model,
            thinking: Some(thinking),
            session_dir,
            cwd,
            link: ParentLink {
                child_name: name.to_owned(),
                host: std::sync::Arc::downgrade(self),
            },
            wall,
            deadline: clock,
            tokens: lease.tokens,
            reader,
        })?;
        let bill = Some(std::sync::Arc::clone(&self.options.attribute));
        child.provider().settle_into(child.settle_hook(bill));
        if let Some(clock) = clock {
            child.set_deadline(clock);
        }
        let root = cwd.unwrap_or(self.options.cwd.as_path());
        let store = match kept {
            Some(store) => Ok(store),
            None => {
                let parent = self.parent_id();
                yi_session::create_flat_session(
                    session_dir.to_path_buf(),
                    root.to_string_lossy(),
                    parent,
                )
            }
        };
        store
            .and_then(|store| child.attach_store(store))
            .map_err(|error| error.to_string())?;
        Ok(child)
    }

    /// A lagged watch missed events: counters and activity are read again from the session, and
    /// true means a run is live whose start event may be lost, so no reader would conclude it.
    pub(crate) fn refold(&self, child_id: &str) -> bool {
        let mut running = false;
        if let Ok(mut children) = self.children.lock()
            && let Some(record) = children.get_mut(child_id)
        {
            // ponytail: `messages` trails a live turn, so counters only ever move up here.
            let messages = record.session.messages();
            let mut fresh = (0_u64, 0_u64, 0.0_f64);
            for message in record.billable(&messages) {
                if let AgentMessage::Assistant { usage, content, .. } = message {
                    let calls = content
                        .iter()
                        .filter(|block| matches!(block, Content::ToolCall { .. }))
                        .count();
                    fresh.0 = fresh.0.saturating_add(u64::try_from(calls).unwrap_or(0));
                    fresh.1 = fresh
                        .1
                        .saturating_add(u64::try_from(usage.total_tokens).unwrap_or(0));
                    fresh.2 += usage.cost.total.as_f64().unwrap_or(0.0);
                }
            }
            record.tool_use_count = record.tool_use_count.max(fresh.0);
            record.token_count = record.token_count.max(fresh.1);
            record.cost = record.cost.max(fresh.2);
            let live = record.session.status() == crate::session::Status::Running;
            running = live || (record.exit.is_none() && !record.concluding);
            record.activity = if live {
                ChildActivity::Writing
            } else {
                ChildActivity::Waiting
            };
        }
        self.publish(child_id);
        running
    }
}

pub(super) fn require_kwargs(kwargs: &Map<String, Value>) -> Result<(), String> {
    let mut unsupported: Vec<&str> = kwargs
        .keys()
        .map(String::as_str)
        .filter(|key| {
            !matches!(
                *key,
                "name"
                    | "model"
                    | "thinking"
                    | "fork"
                    | "isolation"
                    | "carry"
                    | "tree"
                    | "deny_write"
                    | "deny_read"
                    | "deny_url"
                    | "context"
                    | "context_chars"
                    | "check"
                    | "deadline_s"
                    | "tokens"
                    | "parent_close"
                    | "role"
                    | "partition"
                    | "tools"
                    | "turns"
                    | "schema"
                    | "readers"
            )
        })
        .collect();
    if unsupported.is_empty() {
        return Ok(());
    }
    unsupported.sort_unstable();
    Err(format!(
        "Unsupported rlm.run kwargs: {}",
        unsupported.join(", ")
    ))
}

/// Invariant: a spawn refused after its directory was made leaves none behind, so the family
/// names only children that ran (a placeable, a lane, a container and the build all refuse).
pub(super) struct SpawnDir {
    path: PathBuf,
    listed: bool,
}

impl SpawnDir {
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn keep(&mut self) {
        self.listed = true;
    }
}

impl Drop for SpawnDir {
    fn drop(&mut self) {
        if !self.listed {
            let _removal_is_the_point = std::fs::remove_dir_all(&self.path);
        }
    }
}

pub(super) fn create_child_dir(parent_dir: &Path) -> Result<(SpawnDir, String), String> {
    std::fs::create_dir_all(parent_dir).map_err(|error| error.to_string())?;
    for _ in 0..100 {
        let suffix = super::random_suffix()?;
        let dir = parent_dir.join(format!("sub-{suffix}"));
        match std::fs::create_dir(&dir) {
            Ok(()) => {
                return Ok((
                    SpawnDir {
                        path: dir,
                        listed: false,
                    },
                    format!("sub-{suffix}"),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(format!("{}: {error}", dir.display())),
        }
    }
    Err("Unable to create unique RLM child session directory".to_owned())
}

/// How much parent history seeds a child's transcript. `LastN(n)` counts turn boundaries
/// rather than messages, so a child never opens on half of an exchange (§11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fork {
    None,
    All,
    LastN(u64),
}

/// Whether a child edits the parent's checkout or gets a git worktree of its own (§11); a
/// container child gets the same worktree and runs its bash in a container over it (D286).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Isolation {
    None,
    Worktree,
    Container(String),
}

pub(super) fn parse_isolation(kwargs: &Map<String, Value>) -> Result<Isolation, String> {
    if kwargs.contains_key("tree") {
        return Ok(Isolation::Worktree);
    }
    match kwargs.get("isolation") {
        None | Some(Value::Null) => Ok(Isolation::None),
        Some(Value::String(value)) => match value.trim() {
            "none" => Ok(Isolation::None),
            "worktree" => Ok(Isolation::Worktree),
            other => match crate::node::image_of(other) {
                Some(image) => Ok(Isolation::Container(image?.to_owned())),
                None => Err(format!(
                    "rlm.run isolation must be \"none\", \"worktree\" or \"container:<image>\", got {other}"
                )),
            },
        },
        Some(other) => Err(format!("rlm.run isolation must be a string, got {other}")),
    }
}

const CARRY_MAX: usize = 256;

/// `carry=[".env"]`: ignored files a fork brings along. A name a model gave never becomes a ref.
fn parse_carry(kwargs: &Map<String, Value>) -> Result<Vec<String>, String> {
    let entries = match kwargs.get("carry") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(entries)) => entries,
        Some(other) => {
            return Err(format!(
                "rlm.run carry must be a list of paths, got {other}"
            ));
        }
    };
    if entries.len() > CARRY_MAX {
        return Err(format!(
            "rlm.run carry names {} paths; the cap is {CARRY_MAX}",
            entries.len()
        ));
    }
    (entries.iter())
        .map(|entry| {
            let path = entry.as_str();
            path.map(str::to_owned)
                .ok_or_else(|| format!("rlm.run carry entries must be strings, got {entry}"))
        })
        .collect()
}

pub(super) fn parse_fork(kwargs: &Map<String, Value>) -> Result<Fork, String> {
    match kwargs.get("fork") {
        None | Some(Value::Null) => Ok(Fork::None),
        Some(Value::String(value)) => match value.trim() {
            "none" => Ok(Fork::None),
            "all" => Ok(Fork::All),
            other => other
                .parse::<u64>()
                .ok()
                .filter(|turns| *turns > 0)
                .map(Fork::LastN)
                .ok_or_else(|| {
                    format!("rlm.run fork must be \"none\", \"all\", or a positive turn count, got {other}")
                }),
        },
        Some(Value::Number(number)) => number
            .as_u64()
            .filter(|turns| *turns > 0)
            .map(Fork::LastN)
            .ok_or_else(|| format!("rlm.run fork turn count must be positive, got {number}")),
        Some(other) => Err(format!("rlm.run fork must be a string or number, got {other}")),
    }
}
