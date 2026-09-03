use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::task::{Id, JoinSet};
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

/// Invariant: `busy` is released by task id, never by the lane's return value —
/// an aborted lane yields none, and would otherwise stay busy for good.
fn reap_lane(
    res: Result<(Id, ()), tokio::task::JoinError>,
    owners: &mut HashMap<Id, String>,
    busy: &mut HashSet<String>,
) {
    let (id, aborted) = match res {
        Ok((id, ())) => (id, None),
        Err(error) => (error.id(), Some(error)),
    };
    let Some(session) = owners.remove(&id) else {
        return;
    };
    if let Some(error) = aborted {
        eprintln!("heartbeat lane for session {session} did not finish: {error}");
    }
    busy.remove(&session);
}

fn spawn_claimed(
    store: &Arc<JobStore>,
    deliver: &Arc<DeliverFn>,
    claimed: Vec<ClaimedDispatch>,
    lanes: &mut JoinSet<()>,
    owners: &mut HashMap<Id, String>,
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
        let handle = lanes.spawn_blocking(move || {
            // Invariant: a panicked DeliverFn leaves that lane's claims until
            // the next Scheduler::start recovery.
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
        });
        owners.insert(handle.id(), session);
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
            let mut owners = HashMap::new();
            let mut busy = HashSet::new();
            loop {
                while let Some(res) = lanes.try_join_next_with_id() {
                    reap_lane(res, &mut owners, &mut busy);
                }
                // Invariant: `JobStore::mutate` wakes with `notify_waiters`, which stores no
                // permit, so the waiter registers before reading the snapshot it acts on.
                let mut woken = std::pin::pin!(changed.notified());
                let _ = woken.as_mut().enable();
                let now = yi_session::now_ms();
                let next = next_claimable_run_at(&store.snapshot(), &busy);
                if next.is_some_and(|at| at <= now) {
                    let claimed = store
                        .mutate(|state| claim_due_in_state(state, now, now, &mut new_id, &busy));
                    spawn_claimed(
                        &store,
                        &deliver,
                        claimed,
                        &mut lanes,
                        &mut owners,
                        &mut busy,
                    );
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
                    Some(res) = lanes.join_next_with_id() => {
                        reap_lane(res, &mut owners, &mut busy);
                    }
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

#[cfg(test)]
mod tests {
    use super::{HashMap, HashSet, JoinSet, reap_lane};

    #[tokio::test]
    async fn an_aborted_lane_frees_its_session() -> Result<(), Box<dyn std::error::Error>> {
        let mut lanes: JoinSet<()> = JoinSet::new();
        let mut owners = HashMap::new();
        let mut busy = HashSet::new();
        let handle = lanes.spawn(std::future::pending::<()>());
        owners.insert(handle.id(), "a".to_owned());
        busy.insert("a".to_owned());

        handle.abort();
        let res = lanes.join_next_with_id().await.ok_or("lane never joined")?;
        assert!(res.is_err(), "an aborted lane must join as an error");
        reap_lane(res, &mut owners, &mut busy);

        assert!(
            !busy.contains("a"),
            "an aborted lane left its session busy for the process lifetime"
        );
        assert!(owners.is_empty(), "the task-id map must not leak the lane");
        Ok(())
    }
}
