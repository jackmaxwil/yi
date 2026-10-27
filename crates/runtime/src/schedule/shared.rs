use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use super::{DeliverFn, JobStore, RunOutcome, Scheduler};

/// The kill switch's reach into one session: `true` holds its wakes and interrupts its turn,
/// `false` lifts the hold; each answers whether it changed anything.
pub(crate) type StopFn = dyn Fn(bool) -> bool + Send + Sync;
pub(crate) type NoticeFn = dyn Fn(&str) + Send + Sync;

pub(crate) struct DeliveryHub {
    lanes: Mutex<HashMap<String, Arc<DeliverFn>>>,
    stops: Mutex<HashMap<String, Arc<StopFn>>>,
    notices: Mutex<HashMap<String, Arc<NoticeFn>>>,
    absent: Mutex<Option<Arc<AbsentFn>>>,
}

impl DeliveryHub {
    pub(crate) fn new() -> Self {
        Self {
            lanes: Mutex::new(HashMap::new()),
            stops: Mutex::new(HashMap::new()),
            notices: Mutex::new(HashMap::new()),
            absent: Mutex::new(None),
        }
    }

    /// Invariant: an evicted session's wake is owed, not skipped: `reload` loads it and the
    /// deferred job delivers once its lane is back.
    pub(crate) fn on_absent(&self, reload: Arc<AbsentFn>) {
        *self
            .absent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(reload);
    }

    pub(crate) fn register(&self, session_id: String, deliver: Arc<DeliverFn>) {
        lock_lanes(&self.lanes).insert(session_id, deliver);
    }

    pub(crate) fn register_stop(&self, session_id: String, stop: Arc<StopFn>) {
        lock_lanes(&self.stops).insert(session_id, stop);
    }

    pub(crate) fn register_notice(&self, session_id: String, notice: Arc<NoticeFn>) {
        lock_lanes(&self.notices).insert(session_id, notice);
    }

    /// `None` tells every session on the ledger; the hooks run with no lock held.
    pub(crate) fn notify(&self, session_id: Option<&str>, text: &str) {
        let notices: Vec<Arc<NoticeFn>> = {
            let all = lock_lanes(&self.notices);
            match session_id {
                Some(id) => all.get(id).map(Arc::clone).into_iter().collect(),
                None => all.values().map(Arc::clone).collect(),
            }
        };
        notices.iter().for_each(|notice| notice(text));
    }

    /// Invariant: a session reopened before its old handle drops re-registered under the same id,
    /// so only the lane `deliver` put there is removed; `false` means another holder owns it.
    pub(crate) fn unregister(&self, session_id: &str, deliver: &Arc<DeliverFn>) -> bool {
        let mut lanes = lock_lanes(&self.lanes);
        if !lanes
            .get(session_id)
            .is_some_and(|lane| Arc::ptr_eq(lane, deliver))
        {
            return false;
        }
        lanes.remove(session_id);
        lock_lanes(&self.stops).remove(session_id);
        lock_lanes(&self.notices).remove(session_id);
        true
    }

    pub(crate) fn stop_all(&self, on: bool) -> u64 {
        let stops: Vec<Arc<StopFn>> = lock_lanes(&self.stops).values().map(Arc::clone).collect();
        let changed = stops.iter().filter(|stop| stop(on)).count();
        u64::try_from(changed).unwrap_or(u64::MAX)
    }

    #[cfg(test)]
    fn lane_count(&self) -> usize {
        lock_lanes(&self.lanes).len()
    }

    pub(crate) fn dispatch(
        &self,
        job: &yi_types::schedule::Job,
        firing: &super::Firing,
    ) -> Result<RunOutcome, String> {
        let lane = lock_lanes(&self.lanes).get(&job.session_id).map(Arc::clone);
        let owed = || (job.channel.as_ref()).is_none_or(|sub| super::channel::unread(job, sub));
        let reload = || {
            let hook = self
                .absent
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            hook.clone()
        };
        let absent = || owed() && reload().is_some_and(|reload| reload(&job.session_id));
        match lane {
            Some(deliver) => deliver(job, firing),
            None if absent() => Ok(RunOutcome::Deferred),
            None => Ok(RunOutcome::Skipped),
        }
    }
}

/// Answers whether the session `id`, unloaded while idle, is being loaded again for a wake.
pub(crate) type AbsentFn = dyn Fn(&str) -> bool + Send + Sync;

pub(crate) struct SharedSchedule {
    pub store: Arc<JobStore>,
    pub hub: Arc<DeliveryHub>,
    _scheduler: Scheduler,
}

struct Interned {
    live: Weak<SharedSchedule>,
    parked: Option<Arc<SharedSchedule>>,
}

impl Interned {
    fn get(&self) -> Option<Arc<SharedSchedule>> {
        self.parked.clone().or_else(|| self.live.upgrade())
    }
}

fn lock_lanes<T: ?Sized>(
    lanes: &Mutex<HashMap<String, Arc<T>>>,
) -> std::sync::MutexGuard<'_, HashMap<String, Arc<T>>> {
    lanes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Invariant: wired sessions own their timer; `parked` owns it only while its session is
/// unloaded idle, so a close or delete can release it without reviving the session.
fn intern_map() -> &'static Mutex<HashMap<PathBuf, Interned>> {
    static MAP: OnceLock<Mutex<HashMap<PathBuf, Interned>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn live() -> Vec<Arc<SharedSchedule>> {
    intern_map()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .values()
        .filter_map(Interned::get)
        .collect()
}

pub(crate) fn halt_all(on: bool, now: u64) -> (u64, u64) {
    let all = live();
    all.iter().fold((0, 0), |(jobs, sessions), shared| {
        let (held, stopped) = super::clock::halt_store(&shared.store, Some(&shared.hub), on, now);
        (jobs.saturating_add(held), sessions.saturating_add(stopped))
    })
}

/// A message landed in the buffer at `path`: each subscription reading it comes due now, or one
/// cadence after its last delivery, so a burst still wakes a session at most once a cadence.
pub(crate) fn poke(path: &std::path::Path) {
    due(path, true);
}

/// A letter is one message a person or agent is waiting on, so its mailbox comes due now.
pub(crate) fn poke_now(path: &std::path::Path) {
    due(path, false);
}

fn due(path: &std::path::Path, paced: bool) {
    let all = live();
    let reads = |job: &yi_types::schedule::Job| {
        super::claimable(job)
            && job
                .channel
                .as_ref()
                .is_some_and(|sub| std::path::Path::new(&sub.path) == path)
    };
    let now = yi_session::now_ms();
    for shared in all {
        if !shared.store.snapshot().jobs.iter().any(reads) {
            continue;
        }
        shared.store.mutate(|state| {
            for job in state.jobs.iter_mut().filter(|job| reads(job)) {
                let cadence = (job.schedule.interval_ms.filter(|_| paced)).unwrap_or_default();
                let due = job
                    .last_run_at
                    .map_or(now, |last| last.saturating_add(cadence))
                    .max(now);
                job.next_run_at = Some(job.next_run_at.map_or(due, |at| at.min(due)));
            }
        });
    }
}

/// `prime` runs on the hub before a fresh timer starts, so its first claim finds the lane.
pub(crate) fn intern(path: PathBuf, prime: impl FnOnce(&DeliveryHub)) -> Arc<SharedSchedule> {
    let mut map = intern_map()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(entry) = map.get_mut(&path)
        && let Some(existing) = entry.get()
    {
        entry.parked = None;
        prime(&existing.hub);
        return existing;
    }
    map.retain(|_, entry| entry.get().is_some());
    let hub = Arc::new(DeliveryHub::new());
    let store = Arc::new(JobStore::open(path.clone()).with_hub(Arc::clone(&hub)));
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
    map.insert(
        path,
        Interned {
            live: Arc::downgrade(&shared),
            parked: None,
        },
    );
    shared
}

pub(crate) fn held(store: &Arc<JobStore>) -> Option<Arc<SharedSchedule>> {
    let map = intern_map()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    map.values()
        .filter_map(Interned::get)
        .find(|shared| Arc::ptr_eq(&shared.store, store))
}

pub(crate) fn park(store: &Arc<JobStore>) {
    let mut map = intern_map()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for entry in map.values_mut() {
        let Some(shared) = entry.get() else {
            continue;
        };
        if Arc::ptr_eq(&shared.store, store) {
            entry.parked = Some(shared);
            break;
        }
    }
}

/// The durable schedules directory under a sessions directory, one subdirectory per session.
pub fn schedules_root(session_dir: &std::path::Path) -> PathBuf {
    session_dir.join("schedules")
}

/// A session's durable job file under [`schedules_root`].
pub(crate) fn jobs_path(root: &std::path::Path, session_id: &str) -> PathBuf {
    root.join(session_id).join("scheduled-jobs.json")
}

/// A session the worker closed or deleted without loading it: drop the ownership its
/// eviction retained, unless a live session still owns the same timer.
pub fn release_unloaded(root: &std::path::Path, session_id: &str) {
    let path = jobs_path(root, session_id);
    let mut map = intern_map()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(entry) = map.get_mut(&path) {
        entry.parked = None;
    }
    map.retain(|_, entry| entry.get().is_some());
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
        let lane: Arc<DeliverFn> = Arc::new(|_, _| Ok(RunOutcome::Ran));
        hub.register("running".to_owned(), Arc::clone(&lane));
        hub.register_stop("running".to_owned(), Arc::new(|_| true));
        hub.register_stop("idle".to_owned(), Arc::new(|_| false));
        assert_eq!(
            hub.stop_all(true),
            1,
            "only the running turn was interrupted"
        );
        hub.unregister("running", &lane);
        assert_eq!(hub.stop_all(true), 0, "an ended session's stop still ran");
    }

    #[test]
    fn an_ended_session_stops_dispatching_and_leaves_no_lane() {
        let hub = Arc::new(DeliveryHub::new());
        let lane: Arc<DeliverFn> = Arc::new(|_, _| Ok(RunOutcome::Ran));
        hub.register("a".to_owned(), Arc::clone(&lane));
        hub.register("b".to_owned(), Arc::new(|_, _| Ok(RunOutcome::Ran)));
        assert_eq!(hub.dispatch(&job("a"), &Firing::at(0)), Ok(RunOutcome::Ran));

        hub.unregister("a", &lane);

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

    #[test]
    fn an_old_handle_dropping_after_a_reopen_keeps_the_new_lane() -> std::io::Result<()> {
        let dir = Scratch::new("yi-hub-reopen")?;
        let hub = Arc::new(DeliveryHub::new());
        let (_old_store, old) = lane_service(&dir, &hub);
        old.bind_session("a".to_owned());
        let (_new_store, new) = lane_service(&dir, &hub);
        new.bind_session("a".to_owned());

        drop(old);

        assert_eq!(
            hub.dispatch(&job("a"), &Firing::at(0)),
            Ok(RunOutcome::Ran),
            "the old handle's drop withdrew the reopened session's lane"
        );
        drop(new);
        assert_eq!(hub.lane_count(), 0, "the reopened session's lane leaked");
        Ok(())
    }

    #[test]
    fn a_clock_job_due_for_an_unloaded_session_defers_and_wakes_it_once() {
        let hub = DeliveryHub::new();
        let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = Arc::clone(&wakes);
        hub.on_absent(Arc::new(move |id| {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            id == "a"
        }));
        let due = job("a");
        assert!(due.channel.is_none());

        assert_eq!(
            hub.dispatch(&due, &Firing::at(0)),
            Ok(RunOutcome::Deferred),
            "a heartbeat due for an evicted session was skipped instead of loading it"
        );
        assert_eq!(wakes.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_session_loaded_again_and_closed_leaves_no_held_timer()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = Scratch::new("yi-hub-held")?;
        let path = dir.join("scheduled-jobs.json");
        let shared = intern(path.clone(), |_| {});
        park(&shared.store);
        let timer = Arc::downgrade(&shared);
        drop(shared);
        assert!(
            timer.upgrade().is_some(),
            "an unloaded session lost its timer"
        );

        let shared = intern(path, |hub| {
            hub.register("a".to_owned(), Arc::new(|_, _| Ok(RunOutcome::Ran)));
        });
        drop(shared);
        assert!(
            timer.upgrade().is_none(),
            "a closed session's timer kept ticking and skipping its jobs"
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_session_deleted_while_parked_leaves_no_held_timer()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = Scratch::new("yi-hub-unheld")?;
        let path = jobs_path(&dir, "a");
        let shared = intern(path, |_| {});
        park(&shared.store);
        let timer = Arc::downgrade(&shared);
        drop(shared);
        assert!(
            timer.upgrade().is_some(),
            "a parked session has a held timer to lift in the first place"
        );

        crate::schedule::release_unloaded(&dir, "a");
        assert!(
            timer.upgrade().is_none(),
            "a deleted session's held timer kept firing and skipping its jobs forever"
        );
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

    fn wired(path: &std::path::Path, lane: Arc<DeliverFn>) -> Service {
        let shared = intern(path.to_path_buf(), |_| {});
        Service::new(Arc::clone(&shared.store), "/tmp")
            .with_lane(Arc::clone(&shared.hub), lane)
            .interned()
    }

    fn ran() -> Arc<DeliverFn> {
        Arc::new(|_, _| Ok(RunOutcome::Ran))
    }

    #[tokio::test]
    async fn a_departed_child_leaves_the_interned_timer_nothing_to_hold()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = Scratch::new("yi-hub-departed")?;
        let path = dir.join("scheduled-jobs.json");
        let (a, b) = (wired(&path, ran()), wired(&path, ran()));
        a.bind_session("a".to_owned());
        b.bind_session("b".to_owned());
        let store = Arc::downgrade(&a.store());

        drop((a, b));

        let deadline = Instant::now() + Duration::from_secs(5);
        while store.upgrade().is_some() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            store.upgrade().is_none(),
            "two departed children left their store and timer interned"
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_wired_session_that_never_binds_still_releases_the_timer()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = Scratch::new("yi-hub-unbound-wired")?;
        let a = wired(&dir.join("scheduled-jobs.json"), ran());
        let store = Arc::downgrade(&a.store());

        drop(a);

        let deadline = Instant::now() + Duration::from_secs(5);
        while store.upgrade().is_some() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            store.upgrade().is_none(),
            "a session whose attach failed after wiring left its timer interned"
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_walk_in_flight_as_the_last_session_leaves_still_lets_the_timer_go()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = Scratch::new("yi-hub-walked")?;
        let a = wired(&dir.join("scheduled-jobs.json"), ran());
        a.bind_session("a".to_owned());
        let walking = held(&a.store()).ok_or("the wired session holds no timer")?;
        let timer = Arc::downgrade(&walking);

        a.end();
        drop(walking);

        assert!(
            timer.upgrade().is_none(),
            "a poke walking the map as the last session left kept its timer interned"
        );
        drop(a);
        Ok(())
    }

    #[tokio::test]
    async fn a_sibling_still_bound_keeps_the_timer_and_it_still_delivers()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = Scratch::new("yi-hub-sibling")?;
        let path = dir.join("scheduled-jobs.json");
        let (sent, mut delivered) = tokio::sync::mpsc::unbounded_channel();
        let lane = |sent: tokio::sync::mpsc::UnboundedSender<String>| -> Arc<DeliverFn> {
            Arc::new(move |job, _| {
                let _ = sent.send(job.session_id.clone());
                Ok(RunOutcome::Ran)
            })
        };
        let (a, b) = (wired(&path, ran()), wired(&path, lane(sent.clone())));
        a.bind_session("a".to_owned());
        b.bind_session("b".to_owned());
        let set = crate::schedule::parse_heartbeat_command("/heartbeat every 10s watch it")?;
        let due = |service: &Service| -> Result<(), String> {
            service.apply(&set, yi_session::now_ms())?;
            service.store().mutate(|state| {
                for job in &mut state.jobs {
                    job.next_run_at = Some(1);
                }
            });
            Ok(())
        };

        drop(a);
        due(&b)?;
        let first = tokio::time::timeout(Duration::from_secs(25), delivered.recv())
            .await
            .map_err(|_| "a leaving stopped the timer b is still bound to")?;
        assert_eq!(first.as_deref(), Some("b"));

        let c = wired(&path, lane(sent));
        drop(b);
        c.bind_session("c".to_owned());
        due(&c)?;
        let next = tokio::time::timeout(Duration::from_secs(25), delivered.recv())
            .await
            .map_err(|_| "the last lane leaving stopped the timer c was wired to")?;
        assert_eq!(next.as_deref(), Some("c"));
        Ok(())
    }

    #[tokio::test]
    async fn a_durable_bind_releases_the_ledger_it_left() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = Scratch::new("yi-hub-durable")?;
        let (wiring_path, root) = (dir.join("scheduled-jobs.json"), dir.join("schedules"));
        let root_session = wired(&wiring_path, ran()).durable(root.clone());
        let wired_to = root_session.store();
        root_session.bind_session("a".to_owned());
        assert!(
            held(&wired_to).is_none(),
            "a durable root kept the ledger it was wired to"
        );
        assert!(
            held(&root_session.store()).is_some(),
            "a durable root let go of the ledger it is bound to"
        );
        let first = root_session.store();
        root_session.bind_session("b".to_owned());
        assert!(held(&first).is_none(), "a rebind kept the ledger it left");

        let pending = wired(&wiring_path, ran());
        wired(&wiring_path, ran())
            .durable(root)
            .bind_session("c".to_owned());
        assert!(
            held(&pending.store()).is_some(),
            "a durable bind stopped the timer a wired sibling is about to join"
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
