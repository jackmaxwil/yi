use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinSet;

use super::{
    ClaimedDispatch, DeliverFn, JobStore, claim_due_in_state, next_active_run_at,
    record_dispatch_result_in_state, recover_interrupted_in_state,
};

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
                let next = next_active_run_at(&store.snapshot());
                match next {
                    None if busy.is_empty() => changed.notified().await,
                    None => {
                        if let Some(res) = lanes.join_next().await {
                            reap_lane(res, &mut busy);
                        }
                    }
                    Some(at) => {
                        let now = yi_session::now_ms();
                        if at > now {
                            let wait = Duration::from_millis(at - now);
                            if lanes.is_empty() {
                                tokio::select! {
                                    () = tokio::time::sleep(wait) => {}
                                    () = changed.notified() => {}
                                }
                            } else {
                                tokio::select! {
                                    () = tokio::time::sleep(wait) => {}
                                    () = changed.notified() => {}
                                    Some(res) = lanes.join_next() => {
                                        reap_lane(res, &mut busy);
                                    }
                                }
                            }
                            continue;
                        }
                        let claimed = store.mutate(|state| {
                            claim_due_in_state(state, now, now, &mut new_id, &busy)
                        });
                        let skipped_due = claimed.is_empty() && !busy.is_empty();
                        spawn_claimed(&store, &deliver, claimed, &mut lanes, &mut busy);
                        if skipped_due && let Some(res) = lanes.join_next().await {
                            reap_lane(res, &mut busy);
                        }
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
