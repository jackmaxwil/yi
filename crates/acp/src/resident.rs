//! Incident: a worker dropped a session only on `session/close`, which no client sends, so a
//! daemon kept every session it ever opened, with its store, kernel and tools, for its life.

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde_json::Value;
use tokio::sync::mpsc::WeakUnboundedSender;
use yi_types::subagent::ChildStatus;

use crate::attach::Incoming;
use crate::{AcpState, SessionHandle};

/// Idle sessions a worker keeps loaded, the console's own count of parked chats.
pub(crate) const RESIDENT_MAX: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Away {
    Evicted,
    Loading,
}

/// Sessions this worker unloaded while idle, absent once one is loaded again.
static AWAY: Mutex<BTreeMap<String, Away>> = Mutex::new(BTreeMap::new());

fn away() -> MutexGuard<'static, BTreeMap<String, Away>> {
    AWAY.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Gives up on a load that failed, so a due job is skipped instead of deferred for good.
pub(crate) fn forget(id: &str) {
    away().remove(id);
}

/// A job came due for `id`: true while it is unloaded or loading again. The first sends the one
/// wake that loads it, so a second due together defers instead of queueing a wake that blocks.
fn owed(id: &str, wake: &WeakUnboundedSender<Incoming>) -> bool {
    let was = away()
        .get_mut(id)
        .map(|state| std::mem::replace(state, Away::Loading));
    if was == Some(Away::Evicted)
        && let Some(wake) = wake.upgrade()
    {
        let _worker_gone = wake.send(Incoming::Built(id.to_owned()));
    }
    was.is_some()
}

/// Whether dropping the session would stop or lose something, so it stays past [`RESIDENT_MAX`].
pub(crate) fn pinned(handle: &SessionHandle) -> bool {
    let session = &handle.session;
    session.status() == yi_runtime::Status::Running
        || !crate::update::queued(&handle.prompts).is_empty()
        || session.pending_count() > 0
        || handle
            .host
            .children_view()
            .iter()
            .any(|child| child_pins(&child.update))
        || kernel_pins(session.kernel_service().as_deref())
}

pub(crate) fn child_pins(update: &yi_types::subagent::ChildUpdate) -> bool {
    update.status == ChildStatus::Running
}

pub(crate) fn kernel_pins(kernel: Option<&yi_runtime::KernelService>) -> bool {
    kernel.is_some_and(|kernel| kernel.unsaved() || kernel.user_cell_running())
}

impl AcpState {
    /// A session closed or deleted without a loaded lane: its parked schedule timer
    /// would otherwise be held forever by the eviction that unloaded it.
    pub(crate) fn release_parked(&self, id: &str) {
        yi_runtime::schedule::release_unloaded(&self.schedules_root, id);
    }

    pub(crate) fn session_id(params: &Value) -> Result<String, (i64, String)> {
        params
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or((crate::INVALID_PARAMS, "missing sessionId".to_owned()))
    }

    /// Drops the longest-idle unpinned sessions past [`RESIDENT_MAX`], never `kept`.
    pub(crate) fn evict_idle(&mut self, kept: &str) {
        let back = away().remove(kept).and_then(|_| self.sessions.get(kept));
        if let Some(heartbeats) = back.and_then(|handle| handle.session.heartbeat_service()) {
            heartbeats.rearm_channels();
        }
        while self.sessions.len() > RESIDENT_MAX {
            let idlest = (self.sessions.iter())
                .filter(|(id, handle)| id.as_str() != kept && !pinned(handle))
                .min_by_key(|(_, handle)| handle.last_active.load(Ordering::Relaxed))
                .map(|(id, _)| id.clone());
            let Some(id) = idlest else {
                eprintln!(
                    "worker: {} sessions stay loaded past {RESIDENT_MAX}: every other one is pinned",
                    self.sessions.len()
                );
                return;
            };
            let handle = self.sessions.get(&id);
            if let Some(heartbeats) = handle.and_then(|handle| handle.session.heartbeat_service()) {
                let wake = self.wake.clone();
                heartbeats.hold_unloaded(move |id| owed(id, &wake));
            }
            self.user_cells.close(&id);
            self.drop_session(&id);
            away().insert(id, Away::Evicted);
        }
    }

    /// Starts loading `id` again if this worker evicted it; a no-op for any other session.
    pub(crate) fn reload(&mut self, id: &str) {
        if self.store_of(id).is_some() {
            return;
        }
        let mut states = away();
        let Some(state) = states.get_mut(id) else {
            return;
        };
        *state = Away::Loading;
        drop(states);
        if self.open_later(id).is_err() {
            forget(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_jobs_due_together_for_an_unloaded_session_send_one_wake()
    -> Result<(), Box<dyn std::error::Error>> {
        let (sender, mut wakes) = tokio::sync::mpsc::unbounded_channel();
        let weak = sender.downgrade();
        away().insert("two-due".to_owned(), Away::Evicted);

        assert!(owed("two-due", &weak) && owed("two-due", &weak));

        let first = wakes.try_recv();
        assert!(matches!(first, Ok(Incoming::Built(ref id)) if id == "two-due"));
        assert!(
            wakes.try_recv().is_err(),
            "the second due job queued a wake that blocks the request loop"
        );
        assert!(!owed("never-evicted", &weak));
        forget("two-due");
        Ok(())
    }

    fn child_update(status: yi_types::subagent::ChildStatus) -> yi_types::subagent::ChildUpdate {
        use yi_types::subagent::{ChildActivity, ChildId, ChildUpdate};
        ChildUpdate {
            id: ChildId("c1".to_owned()),
            name: "c1".to_owned(),
            status,
            activity: ChildActivity::Waiting,
            tool_use_count: 0,
            token_count: 0,
            cost: None,
            answer_preview: None,
            error: None,
            exit: None,
            flag: None,
        }
    }

    #[test]
    fn only_a_running_child_pins_its_session() {
        use yi_types::subagent::ChildStatus;
        assert!(child_pins(&child_update(ChildStatus::Running)));
        assert!(!child_pins(&child_update(ChildStatus::Completed)));
        assert!(
            !child_pins(&child_update(ChildStatus::Error)),
            "a failed child holds nothing an eviction could lose"
        );
        assert!(
            !child_pins(&child_update(ChildStatus::Other("paused".to_owned()))),
            "a status a newer Yi wrote is not known to be running"
        );
    }

    #[tokio::test]
    async fn an_unsaved_kernel_pins_its_session() {
        use std::path::PathBuf;
        use std::sync::Arc;
        use yi_runtime::{HostRegistry, KernelService, KernelServiceOptions};

        let mut registry = HostRegistry::default();
        registry.register_mcp_stubs();
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let service = KernelService::new(KernelServiceOptions::bare(
            std::env::temp_dir(),
            home.unwrap_or_default(),
            Arc::new(registry),
        ));
        assert!(!kernel_pins(Some(&service)), "an idle kernel pins nothing");
        type Cancel = Arc<dyn Fn() -> bool + Send + Sync>;
        let cancelled: Cancel = Arc::new(|| false);
        service
            .execute_user_cell("unsaved_pin_probe = 1", &cancelled)
            .await;
        assert!(
            kernel_pins(Some(&service)),
            "a kernel whose snapshot does not hold its names stays loaded"
        );
    }
}
