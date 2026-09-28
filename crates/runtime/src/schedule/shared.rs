use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use super::{DeliverFn, JobStore, RunOutcome, Scheduler};

/// The kill switch's reach into one session: `true` holds its wakes and interrupts its turn,
/// `false` lifts the hold; each answers whether it changed anything.
pub(crate) type StopFn = dyn Fn(bool) -> bool + Send + Sync;

pub(crate) struct DeliveryHub {
    lanes: Mutex<HashMap<String, Arc<DeliverFn>>>,
    stops: Mutex<HashMap<String, Arc<StopFn>>>,
}

impl DeliveryHub {
    pub(crate) fn new() -> Self {
        Self {
            lanes: Mutex::new(HashMap::new()),
            stops: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn register(&self, session_id: String, deliver: Arc<DeliverFn>) {
        lock_lanes(&self.lanes).insert(session_id, deliver);
    }

    pub(crate) fn register_stop(&self, session_id: String, stop: Arc<StopFn>) {
        lock_lanes(&self.stops).insert(session_id, stop);
    }

    pub(crate) fn unregister(&self, session_id: &str) {
        lock_lanes(&self.lanes).remove(session_id);
        lock_lanes(&self.stops).remove(session_id);
    }

    pub(crate) fn stop_all(&self, on: bool) -> u64 {
        let stops: Vec<Arc<StopFn>> = lock_lanes(&self.stops).values().map(Arc::clone).collect();
        let changed = stops.iter().filter(|stop| stop(on)).count();
        u64::try_from(changed).unwrap_or(u64::MAX)
    }

    fn lane_count(&self) -> usize {
        lock_lanes(&self.lanes).len()
    }

    pub(crate) fn dispatch(
        &self,
        job: &yi_types::schedule::Job,
        firing: &super::Firing,
    ) -> Result<RunOutcome, String> {
        let lane = lock_lanes(&self.lanes).get(&job.session_id).map(Arc::clone);
        match lane {
            Some(deliver) => deliver(job, firing),
            None => Ok(RunOutcome::Skipped),
        }
    }
}

pub(crate) struct SharedSchedule {
    pub store: Arc<JobStore>,
    pub hub: Arc<DeliveryHub>,
    _scheduler: Scheduler,
}

fn lock_lanes<T: ?Sized>(
    lanes: &Mutex<HashMap<String, Arc<T>>>,
) -> std::sync::MutexGuard<'_, HashMap<String, Arc<T>>> {
    lanes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn intern_map() -> &'static Mutex<HashMap<PathBuf, Arc<SharedSchedule>>> {
    static MAP: OnceLock<Mutex<HashMap<PathBuf, Arc<SharedSchedule>>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn halt_all(on: bool, now: u64) -> (u64, u64) {
    let all: Vec<Arc<SharedSchedule>> = intern_map()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .values()
        .map(Arc::clone)
        .collect();
    all.iter().fold((0, 0), |(jobs, sessions), shared| {
        let (held, stopped) = super::clock::halt_store(&shared.store, Some(&shared.hub), on, now);
        (jobs.saturating_add(held), sessions.saturating_add(stopped))
    })
}

/// `prime` runs on the hub before a fresh timer starts, so its first claim finds the lane.
pub(crate) fn intern(path: PathBuf, prime: impl FnOnce(&DeliveryHub)) -> Arc<SharedSchedule> {
    let mut map = intern_map()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(existing) = map.get(&path) {
        prime(&existing.hub);
        return Arc::clone(existing);
    }
    let store = Arc::new(JobStore::open(path.clone()));
    let hub = Arc::new(DeliveryHub::new());
    prime(&hub);
    let dispatch = Arc::clone(&hub);
    let scheduler = Scheduler::start(
        Arc::clone(&store),
        Arc::new(move |job, firing| dispatch.dispatch(job, firing)),
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

pub(crate) fn release(store: &Arc<JobStore>) {
    intern_map()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|_, shared| !Arc::ptr_eq(&shared.store, store) || shared.hub.lane_count() > 0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::HeartbeatService as Service;
    use crate::schedule::{Firing, JobSpec, RunOutcome, new_job};
    use crate::scratch::Scratch;
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
    fn dispatch_drops_the_lanes_lock_before_deliver() -> Result<(), Box<dyn std::error::Error>> {
        let hub = Arc::new(DeliveryHub::new());
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let a_gate = Arc::clone(&gate);
        let (entered, delivering) = std::sync::mpsc::channel();
        hub.register(
            "a".to_owned(),
            Arc::new(move |_, _| {
                let _ = entered.send(());
                let (lock, cvar) = &*a_gate;
                let mut go = lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                while !*go {
                    go = cvar
                        .wait(go)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                Ok(RunOutcome::Ran)
            }),
        );
        hub.register("b".to_owned(), Arc::new(|_, _| Ok(RunOutcome::Ran)));

        let hub_a = Arc::clone(&hub);
        let thread = std::thread::spawn(move || hub_a.dispatch(&job("a"), &Firing::at(0)));
        delivering.recv()?;
        let started = Instant::now();
        assert_eq!(hub.dispatch(&job("b"), &Firing::at(0)), Ok(RunOutcome::Ran));
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
        thread.join().map_err(|_| "lane a panicked")??;
        Ok(())
    }

    #[test]
    fn the_kill_switch_reaches_every_bound_session_and_no_ended_one() {
        let hub = DeliveryHub::new();
        hub.register_stop("running".to_owned(), Arc::new(|_| true));
        hub.register_stop("idle".to_owned(), Arc::new(|_| false));
        assert_eq!(
            hub.stop_all(true),
            1,
            "only the running turn was interrupted"
        );
        hub.unregister("running");
        assert_eq!(hub.stop_all(true), 0, "an ended session's stop still ran");
    }

    #[test]
    fn an_ended_session_stops_dispatching_and_leaves_no_lane() {
        let hub = Arc::new(DeliveryHub::new());
        hub.register("a".to_owned(), Arc::new(|_, _| Ok(RunOutcome::Ran)));
        hub.register("b".to_owned(), Arc::new(|_, _| Ok(RunOutcome::Ran)));
        assert_eq!(hub.dispatch(&job("a"), &Firing::at(0)), Ok(RunOutcome::Ran));

        hub.unregister("a");

        assert_eq!(
            hub.dispatch(&job("a"), &Firing::at(0)),
            Ok(RunOutcome::Skipped),
            "an ended session's still-Active heartbeat fired into a stale lane"
        );
        assert_eq!(hub.dispatch(&job("b"), &Firing::at(0)), Ok(RunOutcome::Ran));
        assert_eq!(hub.lane_count(), 1, "the ended session's lane leaked");
    }

    fn lane_service(dir: &std::path::Path, hub: &Arc<DeliveryHub>) -> (Arc<JobStore>, Service) {
        let store = Arc::new(JobStore::open(dir.join("scheduled-jobs.json")));
        let service = Service::new(Arc::clone(&store), "/tmp")
            .with_lane(Arc::clone(hub), Arc::new(|_, _| Ok(RunOutcome::Ran)));
        (store, service)
    }

    #[test]
    fn dropping_an_attached_service_withdraws_its_lane() -> std::io::Result<()> {
        let dir = Scratch::new("yi-hub-drop")?;
        let hub = Arc::new(DeliveryHub::new());
        let (_store, service) = lane_service(&dir, &hub);
        service.bind_session("a".to_owned());
        assert_eq!(hub.lane_count(), 1);

        drop(service);

        assert_eq!(hub.lane_count(), 0, "the session ended but kept its lane");
        assert_eq!(
            hub.dispatch(&job("a"), &Firing::at(0)),
            Ok(RunOutcome::Skipped)
        );
        Ok(())
    }

    #[test]
    fn rebinding_the_service_withdraws_the_previous_lane() -> std::io::Result<()> {
        let dir = Scratch::new("yi-hub-rebind")?;
        let hub = Arc::new(DeliveryHub::new());
        let (_store, service) = lane_service(&dir, &hub);
        service.bind_session("a".to_owned());
        service.bind_session("b".to_owned());

        assert_eq!(
            hub.dispatch(&job("a"), &Firing::at(0)),
            Ok(RunOutcome::Skipped),
            "the rebound session's predecessor still dispatched into a stale lane"
        );
        assert_eq!(
            hub.lane_count(),
            1,
            "the rebind left session a's lane behind"
        );
        assert_eq!(hub.dispatch(&job("b"), &Firing::at(0)), Ok(RunOutcome::Ran));
        Ok(())
    }

    #[tokio::test]
    async fn an_ended_session_leaves_the_timer_nothing_to_re_arm()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = Scratch::new("yi-hub-ended")?;
        let path = dir.join("scheduled-jobs.json");
        let hub = Arc::new(DeliveryHub::new());
        let (store, service) = lane_service(&dir, &hub);
        service.bind_session("a".to_owned());
        let set = crate::schedule::parse_heartbeat_command("/heartbeat every 10s watch A")?;
        service.apply(&set, yi_session::now_ms())?;
        store.mutate(|state| {
            for job in &mut state.jobs {
                job.next_run_at = Some(1);
            }
        });

        drop(service);

        let settled = std::fs::read_to_string(&path)?;
        let dispatch = Arc::clone(&hub);
        let scheduler = Scheduler::start(
            Arc::clone(&store),
            Arc::new(move |job, firing| dispatch.dispatch(job, firing)),
            || "dsp-1".to_owned(),
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        scheduler.stop();

        assert_eq!(
            std::fs::read_to_string(&path)?,
            settled,
            "the timer kept re-arming and re-fsyncing an ended session's job"
        );
        let state = store.snapshot();
        let job = state.jobs.first().ok_or("the heartbeat left the ledger")?;
        assert_eq!(
            job.status,
            yi_types::schedule::JobStatus::Paused,
            "an ended session's job stayed claimable"
        );
        assert_eq!(job.run_count, 0, "a dead session's job ran");
        assert!(
            state.dispatches.is_empty(),
            "a dead session's job was claimed"
        );
        Ok(())
    }

    #[test]
    fn a_heartbeat_before_attach_is_refused_not_stamped_empty()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = Scratch::new("yi-hub-unbound")?;
        let store = Arc::new(JobStore::open(dir.join("scheduled-jobs.json")));
        let service = Service::new(Arc::clone(&store), "/tmp").with_lane(
            Arc::new(DeliveryHub::new()),
            Arc::new(|_, _| Ok(RunOutcome::Ran)),
        );

        let set = crate::schedule::parse_heartbeat_command("/heartbeat every 10m watch the build")?;
        let refused = service.apply(&set, 0);

        assert!(
            refused.as_ref().is_err_and(|why| why.contains("attach")),
            "an unbound heartbeat must name the attach order: {refused:?}"
        );
        assert!(
            store.snapshot().jobs.is_empty(),
            "a job stamped with the empty session id reached the ledger"
        );
        Ok(())
    }
}
