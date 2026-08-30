use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinSet;
use yi_types::schedule::{JobStatus, ScheduleState};

use super::{
    ClaimedDispatch, DeliverFn, JobStore, claim_due_in_state, record_dispatch_result_in_state,
    recover_interrupted_in_state,
};

/// Invariant: a session with a lane in flight is unclaimable, so a blocked
/// session's own re-armed interval must not pin the timer's next deadline.
fn next_claimable_run_at(state: &ScheduleState, busy: &HashSet<String>) -> Option<u64> {
    state
        .jobs
        .iter()
        .filter(|job| job.status == JobStatus::Active && !busy.contains(&job.session_id))
        .filter_map(|job| job.next_run_at)
        .min()
}

fn reap_lane(res: Result<String, tokio::task::JoinError>, busy: &mut HashSet<String>) {
    if let Ok(session) = res {
        busy.remove(&session);
    }
}

fn spawn_claimed(
    store: &Arc<JobStore>,
    deliver: &Arc<DeliverFn>,
    claimed: Vec<ClaimedDispatch>,
    lanes: &mut JoinSet<String>,
    busy: &mut HashSet<String>,
) {
    let mut groups = BTreeMap::<String, Vec<ClaimedDispatch>>::new();
    for dispatch in claimed {
        groups
            .entry(dispatch.job.session_id.clone())
            .or_default()
            .push(dispatch);
    }
    for (session, lane) in groups {
        if !busy.insert(session.clone()) {
            continue;
        }
        let store = Arc::clone(store);
        let deliver = Arc::clone(deliver);
        lanes.spawn_blocking(move || {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                for dispatch in &lane {
                    let outcome = deliver(&dispatch.job);
                    store.mutate(|state| {
                        record_dispatch_result_in_state(
                            state,
                            &dispatch.id,
                            outcome,
                            None,
                            yi_session::now_ms(),
                        );
                    });
                }
            }));
            session
        });
    }
}

/// Design H-flow: one timer; after claim, each `session_id` is its own serial lane.
pub struct Scheduler {
    task: tokio::task::JoinHandle<()>,
}

impl Scheduler {
    pub fn start(
        store: Arc<JobStore>,
        deliver: Arc<DeliverFn>,
        mut new_id: impl FnMut() -> String + Send + 'static,
    ) -> Self {
        let now = yi_session::now_ms();
        if !store.snapshot().dispatches.is_empty() {
            store.mutate(|state| recover_interrupted_in_state(state, now, None));
        }
        let changed = store.changed();
        let task = tokio::spawn(async move {
            let mut lanes = JoinSet::new();
            let mut busy = HashSet::new();
            loop {
                while let Some(res) = lanes.try_join_next() {
                    reap_lane(res, &mut busy);
                }
                // Invariant: `JobStore::mutate` wakes with `notify_waiters`,
                // which stores no permit, so the waiter registers before the
                // snapshot it is about to act on is read.
                let mut woken = std::pin::pin!(changed.notified());
                let _ = woken.as_mut().enable();
                let now = yi_session::now_ms();
                let next = next_claimable_run_at(&store.snapshot(), &busy);
                if next.is_some_and(|at| at <= now) {
                    let claimed = store
                        .mutate(|state| claim_due_in_state(state, now, now, &mut new_id, &busy));
                    spawn_claimed(&store, &deliver, claimed, &mut lanes, &mut busy);
                    continue;
                }
                let until_due = next.map(|at| Duration::from_millis(at.saturating_sub(now)));
                let due = async move {
                    match until_due {
                        Some(wait) => tokio::time::sleep(wait).await,
                        None => std::future::pending().await,
                    }
                };
                tokio::select! {
                    () = due => {}
                    () = woken => {}
                    Some(res) = lanes.join_next() => reap_lane(res, &mut busy),
                }
            }
        });
        Self { task }
    }

    pub fn stop(&self) {
        self.task.abort();
    }
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        self.task.abort();
    }
}
