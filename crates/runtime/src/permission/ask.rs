use std::path::PathBuf;
use std::sync::Arc;

use super::PermissionBroker;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskOutcome {
    AllowOnce,
    AllowAlways(usize),
    Reject,
}

/// One approval request. `description` and `patch` are separate so a structured consumer
/// (ACP, §17.2) sends the patch as content while a text one renders `text()` over both.
pub struct PermissionAsk<'a> {
    /// True once the asking session is aborted: the asker releases the wait as a rejection.
    pub stop: Option<&'a Stop>,
    pub title: &'a str,
    pub description: &'a str,
    pub patch: Option<&'a str>,
    pub changes: &'a [PathBuf],
    pub grants: &'a [yi_permission::Grant],
    pub tool_call_id: Option<&'a str>,
}

impl PermissionAsk<'_> {
    pub fn text(&self) -> String {
        match self.patch {
            Some(patch) => format!(
                "{}\n{}",
                self.description,
                PermissionBroker::cut_preview(patch)
            ),
            None => self.description.to_owned(),
        }
    }
}

pub type Stop = yi_tools::CancelFlag;
pub type Asker = Arc<dyn Fn(&PermissionAsk<'_>) -> AskOutcome + Send + Sync>;
