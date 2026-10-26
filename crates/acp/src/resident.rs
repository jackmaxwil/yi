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
const RELOAD_RETRY_MS: u64 = 30_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Away {
    Evicted,
    WakePending,
    Loading,
    Failed { retry_at: u64 },
}

fn due(state: Away, now: u64) -> bool {
    match state {
        Away::Evicted => true,
        Away::WakePending | Away::Loading => false,
        Away::Failed { retry_at } => retry_at <= now,
    }
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

/// A job came due for `id`: true while it is unloaded or loading again. A failed load
/// retries on the scheduler's cadence, not on every request that names the session.
fn owed(id: &str, wake: &WeakUnboundedSender<Incoming>) -> bool {
    let mut states = away();
    let Some(state) = states.get_mut(id) else {
        return false;
    };
    if due(*state, crate::daemon::now_ms()) {
        *state = Away::WakePending;
        if let Some(wake) = wake.upgrade() {
            let _worker_gone = wake.send(Incoming::Built(id.to_owned()));
        }
    }
    true
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
            .any(|child| child.update.status == ChildStatus::Running)
        || kernel_pins(session.kernel_service().as_deref())
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
        let due = matches!(*state, Away::WakePending) || due(*state, crate::daemon::now_ms());
        if !due {
            return;
        }
        *state = Away::Loading;
        drop(states);
        if self.open_later(id).is_err() {
            away().insert(
                id.to_owned(),
                Away::Failed {
                    retry_at: crate::daemon::now_ms().saturating_add(RELOAD_RETRY_MS),
                },
            );
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

    #[test]
    fn a_scheduler_wake_starts_loading_the_evicted_session()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::collections::HashMap;
        use std::sync::Arc;
        use yi_runtime::session_store::{CreateOptions, JsonlRepo, SessionRepo};

        let root = std::env::temp_dir().join(format!(
            "yi-acp-wake-{}-{}",
            std::process::id(),
            crate::daemon::now_ms()
        ));
        let sessions = root.join("sessions");
        std::fs::create_dir_all(&sessions)?;
        let mut repo = JsonlRepo::new(sessions, root.display().to_string());
        drop(repo.create(CreateOptions {
            id: Some("wake-reload".to_owned()),
            ..CreateOptions::default()
        })?);
        let (sender, mut wakes) = tokio::sync::mpsc::unbounded_channel();
        let mut state = AcpState {
            user_cells: crate::cells::UserCells::default(),
            repo,
            schedules_root: root.join("schedules"),
            sessions: HashMap::new(),
            building: HashMap::new(),
            failed: HashMap::new(),
            wake: sender.downgrade(),
            defaults: Arc::new(|| None),
            build: Arc::new(|_, _| Err("not built".to_owned())),
            sink: Arc::new(|_| {}),
            pending: Arc::new(Mutex::new(Some(HashMap::new()))),
            agent_version: String::new(),
            initialized: true,
            cwd: root.clone(),
            prompt_serial: 0,
        };
        away().insert("wake-reload".to_owned(), Away::Evicted);

        assert!(owed("wake-reload", &state.wake));
        assert!(matches!(
            wakes.try_recv()?,
            Incoming::Built(id) if id == "wake-reload"
        ));
        state.reload("wake-reload");
        assert!(
            state.building.contains_key("wake-reload"),
            "the scheduler wake did not start loading the evicted session"
        );
        forget("wake-reload");
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn a_failed_reload_waits_for_its_retry_instead_of_waking_on_every_job() {
        let (sender, mut wakes) = tokio::sync::mpsc::unbounded_channel();
        let weak = sender.downgrade();
        away().insert("retry".to_owned(), Away::Failed { retry_at: u64::MAX });

        assert!(owed("retry", &weak));
        assert!(wakes.try_recv().is_err(), "a failed load retried at once");
        away().insert("retry".to_owned(), Away::Failed { retry_at: 0 });
        assert!(owed("retry", &weak) && owed("retry", &weak));
        assert!(wakes.try_recv().is_ok(), "the due retry sent no wake");
        assert!(wakes.try_recv().is_err(), "one retry sent two wakes");
        forget("retry");
    }

    #[tokio::test]
    async fn an_unsaved_kernel_pins_its_session() {
        use std::path::PathBuf;
        use std::sync::Arc;
        use yi_runtime::{HostRegistry, KernelService, KernelServiceOptions};

        let mut registry = HostRegistry::default();
        registry.register_mcp_stubs();
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let service = KernelService::new(KernelServiceOptions {
            cwd: std::env::temp_dir(),
            home: home.unwrap_or_default(),
            session_dir: None,
            family_dir: None,
            host: Arc::new(registry),
            on_restore: None,
            on_boot: None,
            sandbox: None,
            snapshot_key: None,
            per_session_state: false,
            cell_ceiling: None,
        });
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
