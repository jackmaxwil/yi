use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use super::{DeliverFn, JobStore, RunOutcome, Scheduler};

pub(crate) struct DeliveryHub {
    lanes: Mutex<HashMap<String, Arc<DeliverFn>>>,
}

impl DeliveryHub {
    fn new() -> Self {
        Self {
            lanes: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn register(&self, session_id: String, deliver: Arc<DeliverFn>) {
        lock_lanes(&self.lanes).insert(session_id, deliver);
    }

    pub(crate) fn dispatch(&self, job: &yi_types::schedule::Job) -> RunOutcome {
        let lane = lock_lanes(&self.lanes).get(&job.session_id).map(Arc::clone);
        match lane {
            Some(deliver) => deliver(job),
            None => RunOutcome::Skipped,
        }
    }
}

pub(crate) struct SharedSchedule {
    pub store: Arc<JobStore>,
    pub hub: Arc<DeliveryHub>,
    _scheduler: Scheduler,
}

fn lock_lanes(
    lanes: &Mutex<HashMap<String, Arc<DeliverFn>>>,
) -> std::sync::MutexGuard<'_, HashMap<String, Arc<DeliverFn>>> {
    lanes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn intern_map() -> &'static Mutex<HashMap<PathBuf, Arc<SharedSchedule>>> {
    static MAP: OnceLock<Mutex<HashMap<PathBuf, Arc<SharedSchedule>>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn intern(path: PathBuf) -> Arc<SharedSchedule> {
    let mut map = intern_map()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(existing) = map.get(&path) {
        return Arc::clone(existing);
    }
    let store = Arc::new(JobStore::open(path.clone()));
    let hub = Arc::new(DeliveryHub::new());
    let dispatch = Arc::clone(&hub);
    let scheduler = Scheduler::start(
        Arc::clone(&store),
        Arc::new(move |job| dispatch.dispatch(job)),
        || {
            format!(
                "dsp-{}",
                crate::subagent::random_suffix().unwrap_or_else(|_| "00000000".to_owned())
            )
        },
    );
    let shared = Arc::new(SharedSchedule {
        store,
        hub,
        _scheduler: scheduler,
    });
    map.insert(path, Arc::clone(&shared));
    shared
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::{JobSpec, RunOutcome, new_job};
    use std::sync::{Condvar, Mutex};
    use std::time::{Duration, Instant};
    use yi_types::schedule::{CronSchedule, JobSource, ScheduleKind};

    fn job(session: &str) -> yi_types::schedule::Job {
        let mut job = new_job(JobSpec {
            id: session.to_owned(),
            session_id: session.to_owned(),
            cwd: "/tmp".to_owned(),
            source: JobSource::Heartbeat,
            delivery_mode: None,
            label: None,
            prompt: "p".to_owned(),
            schedule: CronSchedule {
                kind: ScheduleKind::Interval,
                expression: "every 10s".to_owned(),
                interval_ms: Some(10_000),
            },
            next_run_at: 0,
            now_ms: 0,
        });
        job.session_id = session.to_owned();
        job
    }

    #[test]
    fn dispatch_drops_the_lanes_lock_before_deliver() {
        let hub = Arc::new(DeliveryHub::new());
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let a_gate = Arc::clone(&gate);
        hub.register(
            "a".to_owned(),
            Arc::new(move |_| {
                let (lock, cvar) = &*a_gate;
                let mut go = lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                while !*go {
                    go = cvar
                        .wait(go)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                RunOutcome::Ran
            }),
        );
        hub.register("b".to_owned(), Arc::new(|_| RunOutcome::Ran));

        let hub_a = Arc::clone(&hub);
        let thread = std::thread::spawn(move || hub_a.dispatch(&job("a")));
        std::thread::sleep(Duration::from_millis(50));
        let started = Instant::now();
        assert_eq!(hub.dispatch(&job("b")), RunOutcome::Ran);
        assert!(
            started.elapsed() < Duration::from_millis(400),
            "session b waited on the lanes mutex while a delivered"
        );
        {
            let (lock, cvar) = &*gate;
            *lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
            cvar.notify_all();
        }
        thread.join().expect("lane a");
    }
}
