//! Incident: a worker dropped a session only on `session/close`, which no client sends, so a
//! daemon kept every session it ever opened, with its store, kernel and tools, for its life.

use std::collections::BTreeSet;
use std::sync::{Mutex, PoisonError};

use yi_runtime::session_store::SessionRepo;
use yi_types::subagent::ChildStatus;

use crate::attach::Incoming;
use crate::{AcpState, SessionHandle};

/// Idle sessions a worker keeps loaded, the console's own count of parked chats.
pub(crate) const RESIDENT_MAX: usize = 8;

/// Sessions this worker unloaded while idle; a request or a due job loads one again.
static EVICTED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
/// Evicted sessions loading again, whose deferred subscriptions are due once they are back.
static RELOADING: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

fn evicted() -> std::sync::MutexGuard<'static, BTreeSet<String>> {
    EVICTED.lock().unwrap_or_else(PoisonError::into_inner)
}

fn reloading() -> std::sync::MutexGuard<'static, BTreeSet<String>> {
    RELOADING.lock().unwrap_or_else(PoisonError::into_inner)
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
    } else if !crate::update::queued(&handle.prompts).is_empty() {
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
        let back = reloading()
            .remove(kept)
            .then(|| self.sessions.get(kept))
            .flatten();
        if let Some(heartbeats) = back.and_then(|handle| handle.session.heartbeat_service()) {
            heartbeats.rearm_channels();
        }
        while self.sessions.len() > RESIDENT_MAX {
            let idlest = (self.sessions.iter())
                .filter(|(id, handle)| id.as_str() != kept && pin(handle).is_none())
                .min_by_key(|(_, handle)| handle.last_active.get())
                .map(|(id, _)| id.clone());
            let Some(id) = idlest else {
                eprintln!(
                    "worker: {} sessions stay loaded past {RESIDENT_MAX}: every other one is pinned",
                    self.sessions.len()
                );
                return;
            };
            let wake = self.wake.clone();
            yi_runtime::schedule::on_absent_lane(Box::new(move |id| {
                let owed = evicted().contains(id);
                if let (true, Some(wake)) = (owed, wake.upgrade()) {
                    let _worker_gone = wake.send(Incoming::Built(id.to_owned()));
                }
                owed
            }));
            let handle = self.sessions.get(&id);
            if let Some(heartbeats) = handle.and_then(|handle| handle.session.heartbeat_service()) {
                heartbeats.hold_unloaded();
            }
            self.user_cells.close(&id);
            self.drop_session(&id);
            evicted().insert(id);
        }
    }

    /// Starts loading `id` again if this worker evicted it; a no-op for any other session.
    pub(crate) fn reload(&mut self, id: &str) {
        if !evicted().remove(id) || self.sessions.contains_key(id) {
            return;
        }
        if let Ok(store) = self.repo.open(id) {
            reloading().insert(id.to_owned());
            self.attach_later(&store);
        }
    }
}
