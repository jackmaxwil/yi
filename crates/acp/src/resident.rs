//! Incident: a worker dropped a session only on `session/close`, which no client sends, so a
//! daemon kept every session it ever opened, with its store, kernel and tools, for its life.

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, MutexGuard, PoisonError};

use tokio::sync::mpsc::WeakUnboundedSender;
use yi_runtime::session_store::SessionRepo;
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

/// What keeps a session loaded past [`RESIDENT_MAX`]: dropping it would stop or lose this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pin {
    Turn,
    Queued,
    Children,
    Kernel,
}

pub(crate) fn pin(handle: &SessionHandle) -> Option<Pin> {
    let session = &handle.session;
    let children = || {
        (handle.host.children_view().iter())
            .any(|child| child.update.status == ChildStatus::Running)
    };
    let kernel = || {
        (session.kernel_service())
            .is_some_and(|kernel| kernel.unsaved() || kernel.user_cell_running())
    };
    if session.status() == yi_runtime::Status::Running {
        Some(Pin::Turn)
    } else if !crate::update::queued(&handle.prompts).is_empty() || session.pending_count() > 0 {
        Some(Pin::Queued)
    } else if children() {
        Some(Pin::Children)
    } else {
        kernel().then_some(Pin::Kernel)
    }
}

impl AcpState {
    /// Drops the longest-idle unpinned sessions past [`RESIDENT_MAX`], never `kept`.
    pub(crate) fn evict_idle(&mut self, kept: &str) {
        let back = away().remove(kept).and_then(|_| self.sessions.get(kept));
        if let Some(heartbeats) = back.and_then(|handle| handle.session.heartbeat_service()) {
            heartbeats.rearm_channels();
        }
        while self.sessions.len() > RESIDENT_MAX {
            let idlest = (self.sessions.iter())
                .filter(|(id, handle)| id.as_str() != kept && pin(handle).is_none())
                .min_by_key(|(_, handle)| handle.last_active.load(Ordering::Relaxed))
                .map(|(id, _)| id.clone());
            let Some(id) = idlest else {
                eprintln!(
                    "worker: {} sessions stay loaded past {RESIDENT_MAX}: every other one is pinned",
                    self.sessions.len()
                );
                return;
            };
            let wake = self.wake.clone();
            yi_runtime::schedule::on_absent_lane(Box::new(move |id| owed(id, &wake)));
            let handle = self.sessions.get(&id);
            if let Some(heartbeats) = handle.and_then(|handle| handle.session.heartbeat_service()) {
                heartbeats.hold_unloaded();
            }
            self.user_cells.close(&id);
            self.drop_session(&id);
            away().insert(id, Away::Evicted);
        }
    }

    /// Starts loading `id` again if this worker evicted it; a no-op for any other session.
    pub(crate) fn reload(&mut self, id: &str) {
        if self.sessions.contains_key(id) || self.building.contains_key(id) {
            return;
        }
        let mut away = away();
        let Some(state) = away.get_mut(id) else {
            return;
        };
        *state = Away::Loading;
        drop(away);
        match self.repo.open(id) {
            Ok(store) => {
                self.attach_later(&store);
            }
            Err(_) => forget(id),
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
}
