use std::path::Path;

use yi_types::message::{AgentMessage, Content};

use super::{ChildActivity, SubagentHost};

/// Dropped, it frees the name and the slot a build reserved, whether the build landed or not.
pub(super) struct Reservation<'a> {
    host: &'a SubagentHost,
    name: String,
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        if let Ok(mut children) = self.host.children.lock() {
            children.building.retain(|(name, _)| *name != self.name);
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
    ) -> Result<(Reservation<'_>, yi_types::lease::Lease), String> {
        let mut children = self
            .children
            .lock()
            .map_err(|_| "subagent state poisoned")?;
        let refusal = if children.len().saturating_add(children.building.len())
            >= self.options.max_children
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

    /// A lagged watch missed events, so the counters are read again from the session itself
    /// and a missed tool end cannot leave the activity on `Executing`.
    pub(super) fn refold(&self, child_id: &str) {
        if let Ok(mut children) = self.children.lock()
            && let Some(record) = children.get_mut(child_id)
        {
            // ponytail: `messages` trails a live turn, so counters only ever move up here.
            let messages = record.session.messages();
            let mut fresh = (0_u64, 0_u64);
            for message in &messages {
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
            record.activity = if record.session.status() == crate::session::Status::Idle {
                ChildActivity::Waiting
            } else {
                ChildActivity::Writing
            };
        }
        self.publish(child_id);
    }
}
