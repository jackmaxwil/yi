use std::path::Path;

use yi_types::message::{AgentMessage, Content};

use super::{ChildActivity, ChildBuild, Standing, SubagentHost};
use crate::mailbox::ParentLink;
use crate::plan::ops::{ENGINE_AGENT, OWNER_AGENT};
use crate::provider::resolve_model;
use yi_types::model::{Effort, Model};

/// What a build is made from, read off the spawn's kwargs: the model, the effort, the wall.
pub(super) type Cast = (Model, Effort, crate::wall::Wall);

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
    /// Invariant: the roster lock is never held across a build, so `states` and `list` answer
    /// while a factory runs; the name and the slot are reserved under it instead.
    pub(super) fn reserve(
        &self,
        session_name: &str,
        session_dir: &Path,
        ask: &crate::lease::Ask,
        capped: bool,
    ) -> Result<(Reservation<'_>, yi_types::lease::Lease), String> {
        let mut children = self
            .children
            .lock()
            .map_err(|_| "subagent state poisoned")?;
        // Jurors sit in the verification reserve (plan section 7.6) and a service is never
        // reaped while it serves: neither fills the worker cap nor is refused by it.
        let worker = |record: &&super::ChildRecord| matches!(record.standing, Standing::Worker);
        let workers = children.values().filter(worker).count();
        let reserved = [OWNER_AGENT, ENGINE_AGENT, "host"];
        let refusal = if reserved.contains(&session_name) {
            Some(format!(
                "\"{session_name}\" is reserved: it names the plan's owner, engine or host; pick another name"
            ))
        } else if capped
            && workers.saturating_add(children.building.len()) >= self.options.max_children
        {
            Some(format!(
                "RLM child limit reached ({} children retained); rlm.delete_subagent a finished child first",
                self.options.max_children
            ))
        } else if children
            .values()
            .map(|record| &record.session_name)
            .chain(children.building.iter().map(|(name, _)| name))
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
        };
        let lease = lease.inspect_err(|_| {
            let _ = std::fs::remove_dir_all(session_dir);
        })?;
        let tokens = lease.tokens.unwrap_or(0);
        children.building.push((session_name.to_owned(), tokens));
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
        let wall = self.wall_for(kwargs)?;
        Ok((model, thinking.unwrap_or(parent_effort), wall))
    }

    /// Invariant: a child that runs unrecorded leaves nothing to read when it fails, so a
    /// transcript it cannot open refuses the build. `kept` is a respawned service's own.
    pub(super) fn build(
        self: &std::sync::Arc<Self>,
        (model, thinking, wall): Cast,
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
        })?;
        if let Some(clock) = clock {
            child.set_deadline(clock);
        }
        let root = cwd.unwrap_or(self.options.cwd.as_path());
        let store = match kept {
            Some(store) => Ok(store),
            None => {
                yi_session::create_flat_session(session_dir.to_path_buf(), root.to_string_lossy())
            }
        };
        store
            .and_then(|store| child.attach_store(store))
            .map_err(|error| error.to_string())?;
        Ok(child)
    }

    /// A lagged watch missed events: counters and activity are read again from the session, and
    /// true means a run is live whose start event may be lost, so no reader would conclude it.
    pub(super) fn refold(&self, child_id: &str) -> bool {
        let mut running = false;
        if let Ok(mut children) = self.children.lock()
            && let Some(record) = children.get_mut(child_id)
        {
            // ponytail: `messages` trails a live turn, so counters only ever move up here.
            let messages = record.session.messages();
            let mut fresh = (0_u64, 0_u64);
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
                }
            }
            record.tool_use_count = record.tool_use_count.max(fresh.0);
            record.token_count = record.token_count.max(fresh.1);
            running = record.session.status() == crate::session::Status::Running;
            record.activity = if running {
                ChildActivity::Writing
            } else {
                ChildActivity::Waiting
            };
        }
        self.publish(child_id);
        running
    }
}
