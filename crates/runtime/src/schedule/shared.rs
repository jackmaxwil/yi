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
        let lanes = lock_lanes(&self.lanes);
        match lanes.get(&job.session_id) {
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
