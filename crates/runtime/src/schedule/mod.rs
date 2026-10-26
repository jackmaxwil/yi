pub mod adapter;
pub mod channel;
pub mod clock;
mod cron;
mod kernel;
mod lanes;
pub(crate) mod shared;
mod zone;

use std::collections::HashSet;

pub use clock::Firing;
pub use cron::{next_run_at_for_schedule, parse_iso_ms, parse_schedule, parse_schedule_in};
pub use lanes::Scheduler;
pub(crate) use shared::jobs_path;
pub use shared::{release_unloaded, schedules_root};
pub use zone::Zone;

use cron::{
    HOUR_UNITS, MINUTE_UNITS, SECOND_UNITS, civil_of, is_unit, parse_cron_expression,
    split_amount_unit,
};
use yi_kernel::client::civil_from_days;

use yi_types::schedule::{
    CronSchedule, DeliveryMode, DispatchRecord, Job, JobStatus, ScheduleKind, ScheduleState,
};

pub const ONE_SECOND_MS: u64 = 1_000;
pub const ONE_MINUTE_MS: u64 = 60_000;
pub const DEFAULT_HEARTBEAT_SCHEDULE: &str = "every 5m";
pub const DEFAULT_HEARTBEAT_DELIVERY_MODE: DeliveryMode = DeliveryMode::Steer;
pub const HEARTBEAT_USAGE: &str = "Usage: /heartbeat [--every <interval> | <minute hour day month weekday>] [--steer|--follow-up] <instruction>";
// The exact recovery marker; tests and `/heartbeat status` surface it.
pub const INTERRUPTED_ERROR: &str = "Interrupted before scheduled operation completion";

/// Parsed form of the `/heartbeat` slash command; the kernel's `rlm_heartbeat` calls and the
/// ACP `_yi/heartbeat` method reach the same scheduler through this one grammar (§15.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeartbeatCommand {
    Status,
    Pause,
    Resume,
    Clear,
    Halt,
    Set {
        schedule: String,
        instruction: String,
        delivery_mode: Option<DeliveryMode>,
    },
}

pub fn normalize_heartbeat_schedule(input: Option<&str>) -> String {
    let text = input.map(str::trim).unwrap_or("");
    if text.is_empty() {
        return DEFAULT_HEARTBEAT_SCHEDULE.to_owned();
    }
    if split_amount_unit(text).is_some_and(|(_, unit)| {
        is_unit(unit, &SECOND_UNITS) || is_unit(unit, &MINUTE_UNITS) || is_unit(unit, &HOUR_UNITS)
    }) {
        return format!("every {text}");
    }
    text.to_owned()
}

fn consume_delivery_option(text: &str) -> (Option<DeliveryMode>, &str) {
    let trimmed = text.trim_start();
    if let Some(rest) = trimmed.strip_prefix("--steer") {
        return (Some(DeliveryMode::Steer), rest.trim_start());
    }
    if let Some(rest) = trimmed.strip_prefix("--follow-up") {
        return (Some(DeliveryMode::FollowUp), rest.trim_start());
    }
    (None, trimmed)
}

fn leading_cron(text: &str) -> Option<(String, &str)> {
    let mut rest = text;
    let mut fields = Vec::with_capacity(5);
    for _ in 0..5 {
        let (field, after) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        fields.push(field);
        rest = after.trim_start();
    }
    let cron = fields.join(" ");
    parse_cron_expression(&cron).ok()?;
    Some((cron, rest))
}

pub fn parse_heartbeat_command(input: &str) -> Result<HeartbeatCommand, String> {
    let text = input.strip_prefix("/heartbeat").unwrap_or(input).trim();
    match text {
        "" | "status" => return Ok(HeartbeatCommand::Status),
        "pause" => return Ok(HeartbeatCommand::Pause),
        "resume" => return Ok(HeartbeatCommand::Resume),
        "halt" => return Ok(HeartbeatCommand::Halt),
        "clear" | "stop" => return Ok(HeartbeatCommand::Clear),
        _ => {}
    }
    let (leading_delivery, remaining) = consume_delivery_option(text);
    let mut delivery_mode = leading_delivery;
    let (schedule, rest) = if let Some(after) = remaining.strip_prefix("--every ") {
        let after = after.trim_start();
        let (interval, rest) = after.split_at(
            after
                .find(|ch: char| ch.is_whitespace())
                .unwrap_or(after.len()),
        );
        (Some(interval.to_owned()), rest.trim_start())
    } else if remaining.to_ascii_lowercase().starts_with("every ") {
        // Leading `every 10m <instruction>` shorthand: the schedule is the
        // `every N<unit>` head, the rest is the instruction.
        let mut words = remaining.splitn(3, char::is_whitespace);
        let every = words.next().unwrap_or("");
        let amount = words.next().unwrap_or("");
        let rest = words.next().unwrap_or("").trim_start();
        (Some(format!("{every} {amount}")), rest)
    } else if let Some((cron, rest)) = leading_cron(remaining) {
        (Some(cron), rest)
    } else {
        (None, remaining)
    };
    let (trailing_delivery, instruction) = consume_delivery_option(rest);
    if trailing_delivery.is_some() {
        delivery_mode = trailing_delivery;
    }
    let instruction = instruction.trim();
    if instruction.is_empty() {
        return Err(HEARTBEAT_USAGE.to_owned());
    }
    Ok(HeartbeatCommand::Set {
        schedule: normalize_heartbeat_schedule(schedule.as_deref()),
        instruction: instruction.to_owned(),
        delivery_mode,
    })
}

/// What the session is doing when a heartbeat comes due (design §15.2).
#[derive(Debug, Clone, Copy, Default)]
pub struct SessionActivity {
    pub is_streaming: bool,
    pub is_compacting: bool,
    pub has_pending_session_work: bool,
}

pub fn is_heartbeat_job(job: &Job) -> bool {
    matches!(
        job.source,
        Some(yi_types::schedule::JobSource::Heartbeat)
            | Some(yi_types::schedule::JobSource::RlmHeartbeat)
    )
}

/// Holds a due heartbeat back whenever delivering it would stack redundant work or land
/// mid-operation; steering tolerates plain streaming, a follow-up does not (§15.2).
pub fn should_defer(job: &Job, activity: &SessionActivity) -> bool {
    if !is_heartbeat_job(job) && job.unblocks.is_none() && job.channel.is_none() {
        return false;
    }
    if activity.is_compacting || activity.has_pending_session_work {
        return true;
    }
    // "steer" heartbeats interrupt the current turn, so a plain streaming turn
    // must not defer them; "follow_up" heartbeats wait, so streaming defers.
    if job.delivery_mode.unwrap_or(DEFAULT_HEARTBEAT_DELIVERY_MODE) == DeliveryMode::Steer {
        return false;
    }
    activity.is_streaming
}

fn claimable(job: &Job) -> bool {
    job.status == JobStatus::Active && !job.halted
}

fn is_due_job(job: &Job, now_ms: u64) -> bool {
    claimable(job) && job.next_run_at.is_some_and(|at| at <= now_ms)
}

pub struct ClaimedDispatch {
    pub id: String,
    pub job: Job,
    pub firing: Firing,
}

/// Design §15.2: advances every due job's schedule and claims a dispatch for each
/// not already claimed; an already-claimed due job is never double-delivered.
pub fn claim_due_in_state(
    state: &mut ScheduleState,
    due_ms: u64,
    claimed_ms: u64,
    mut new_id: impl FnMut() -> String,
    skip_sessions: &HashSet<String>,
) -> Vec<ClaimedDispatch> {
    let mut dispatches = Vec::new();
    let claimed_job_ids: HashSet<String> = state
        .dispatches
        .iter()
        .map(|dispatch| dispatch.job_id.clone())
        .collect();
    for job in &mut state.jobs {
        if !is_due_job(job, due_ms) {
            continue;
        }
        if skip_sessions.contains(&job.session_id) {
            continue;
        }
        let scheduled_for = job.next_run_at.unwrap_or(due_ms);
        let firing = clock::firing(&job.schedule, scheduled_for, claimed_ms);
        job.next_run_at = next_run_at_for_schedule(&job.schedule, claimed_ms)
            .ok()
            .flatten();
        job.updated_at = claimed_ms;
        if claimed_job_ids.contains(&job.id) {
            job.last_skipped_at = Some(claimed_ms);
            continue;
        }
        let dispatch = DispatchRecord {
            id: new_id(),
            job_id: job.id.clone(),
            claimed_at: claimed_ms,
            scheduled_for,
            extra: serde_json::Map::new(),
        };
        state.dispatches.push(dispatch.clone());
        dispatches.push(ClaimedDispatch {
            id: dispatch.id,
            job: job.clone(),
            firing,
        });
    }
    dispatches
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    Ran,
    Skipped,
    /// Held back by what the session is doing: the tick is still owed and retries soon.
    Deferred,
    /// A channel tick with nothing to hand its target: the subscription keeps waiting.
    Idle,
    /// The channel's adapter is past its restart intensity, so the subscription pauses.
    Paused,
}

/// Incident: a deferred tick re-armed to its next scheduled time, so a daily clock due while a
/// turn held queued work made no todo that day; it retries this soon instead.
pub const DEFER_RETRY_MS: u64 = 30_000;

/// Design §15.2: resolves the claim and advances the job; a clean skip re-arms
/// without counting a run.
pub fn record_dispatch_result_in_state(
    state: &mut ScheduleState,
    dispatch_id: &str,
    outcome: RunOutcome,
    error: Option<String>,
    now_ms: u64,
) -> Option<Job> {
    let dispatch = state
        .dispatches
        .iter()
        .find(|candidate| candidate.id == dispatch_id)?
        .clone();
    state
        .dispatches
        .retain(|candidate| candidate.id != dispatch_id);
    let mut updated = None;
    for job in &mut state.jobs {
        if job.id != dispatch.job_id || job.status != JobStatus::Active {
            continue;
        }
        if outcome == RunOutcome::Deferred && error.is_none() {
            let retry = now_ms.saturating_add(DEFER_RETRY_MS);
            let next = next_run_at_for_schedule(&job.schedule, now_ms)
                .ok()
                .flatten();
            job.next_run_at = Some(next.map_or(retry, |next| next.min(retry)));
            job.last_skipped_at = Some(now_ms);
            job.updated_at = now_ms;
            updated = Some(job.clone());
            continue;
        }
        if outcome == RunOutcome::Idle && error.is_none() {
            job.updated_at = now_ms;
            updated = Some(job.clone());
            continue;
        }
        if outcome == RunOutcome::Paused {
            job.status = JobStatus::Paused;
            job.next_run_at = None;
            job.last_error = Some("its adapter stopped past its restart intensity".to_owned());
            job.updated_at = now_ms;
            updated = Some(job.clone());
            continue;
        }
        // A wait unblocks its todo once, whatever its schedule's shape.
        if job.schedule.kind == ScheduleKind::Once || job.unblocks.is_some() {
            job.status = JobStatus::Completed;
        }
        if outcome == RunOutcome::Skipped && error.is_none() {
            job.next_run_at = next_run_at_for_schedule(&job.schedule, now_ms)
                .ok()
                .flatten();
            job.last_skipped_at = Some(now_ms);
        } else {
            job.last_run_at = Some(now_ms);
            job.last_error = error.clone();
            job.run_count = job.run_count.saturating_add(1);
        }
        job.updated_at = now_ms;
        updated = Some(job.clone());
    }
    updated
}

/// Design §15.2: unresolved claims on start mean the process died mid-dispatch,
/// so mark them interrupted.
pub fn recover_interrupted_in_state(
    state: &mut ScheduleState,
    now_ms: u64,
    dispatch_ids: Option<&HashSet<String>>,
) -> Vec<Job> {
    let interrupted: Vec<&DispatchRecord> = match dispatch_ids {
        Some(ids) => state
            .dispatches
            .iter()
            .filter(|dispatch| ids.contains(&dispatch.id))
            .collect(),
        None => state.dispatches.iter().collect(),
    };
    if interrupted.is_empty() {
        return Vec::new();
    }
    let interrupted_job_ids: HashSet<String> = interrupted
        .iter()
        .map(|dispatch| dispatch.job_id.clone())
        .collect();
    match dispatch_ids {
        Some(ids) => state
            .dispatches
            .retain(|dispatch| !ids.contains(&dispatch.id)),
        None => state.dispatches.clear(),
    }
    let mut recovered = Vec::new();
    for job in &mut state.jobs {
        if !interrupted_job_ids.contains(&job.id) || job.status != JobStatus::Active {
            continue;
        }
        if job.schedule.kind == ScheduleKind::Once {
            job.status = JobStatus::Completed;
        }
        job.last_error = Some(INTERRUPTED_ERROR.to_owned());
        job.updated_at = now_ms;
        recovered.push(job.clone());
    }
    recovered
}

pub struct JobStore {
    path: std::path::PathBuf,
    state: std::sync::Mutex<ScheduleState>,
    changed: std::sync::Arc<tokio::sync::Notify>,
    hub: Option<std::sync::Arc<shared::DeliveryHub>>,
    /// The write error its sessions were last told of, so a ledger that stays unwritable tells
    /// them once; a write that succeeds clears it.
    failed: std::sync::Mutex<Option<String>>,
}

fn lock_or_poisoned<T>(state: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl JobStore {
    /// Design §15.2: opens (or seeds) `scheduled-jobs.json`. A corrupt file is
    /// treated as empty rather than blocking every future schedule.
    pub fn open(path: std::path::PathBuf) -> Self {
        let state = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            path,
            state: std::sync::Mutex::new(state),
            changed: std::sync::Arc::new(tokio::sync::Notify::new()),
            hub: None,
            failed: std::sync::Mutex::new(None),
        }
    }

    pub(crate) fn with_hub(mut self, hub: std::sync::Arc<shared::DeliveryHub>) -> Self {
        self.hub = Some(hub);
        self
    }

    fn notify(&self, session_id: Option<&str>, text: &str) {
        if let Some(hub) = &self.hub {
            hub.notify(session_id, text);
        }
    }

    pub fn changed(&self) -> std::sync::Arc<tokio::sync::Notify> {
        std::sync::Arc::clone(&self.changed)
    }

    pub fn snapshot(&self) -> ScheduleState {
        lock_or_poisoned(&self.state).clone()
    }

    /// Every mutation persists (tmp + fsync + rename) before notifying the
    /// timer — the claim-before-deliver contract (§15.2) rides on this ordering.
    pub fn mutate<R>(&self, action: impl FnOnce(&mut ScheduleState) -> R) -> R {
        let reading = |state: &ScheduleState| -> Vec<(String, String)> {
            state
                .jobs
                .iter()
                .filter(|job| !matches!(job.status, JobStatus::Completed | JobStatus::Cancelled))
                .filter_map(|job| Some((job.id.clone(), job.channel.as_ref()?.path.clone())))
                .collect()
        };
        let (result, ended, failed) = {
            let mut state = lock_or_poisoned(&self.state);
            let before = reading(&state);
            let result = action(&mut state);
            let failed = self.persist(&state);
            let after = reading(&state);
            let ended: Vec<(String, String)> = before
                .into_iter()
                .filter(|job| !after.contains(job))
                .collect();
            (result, ended, failed)
        };
        if let Some(text) = failed {
            self.notify(None, &text);
        }
        // Invariant: a channel job that ends by any path drops its ack, so a finished wait or a
        // cancelled subscription never holds its channel's truncation back.
        for (id, path) in ended {
            let _an_unreleased_ack_only_delays_truncation =
                channel::Channel::at(path).unsubscribe(&id);
        }
        self.changed.notify_waiters();
        result
    }

    /// Invariant: an idle channel tick re-arms in memory only; the next real write carries it,
    /// and a restart re-derives it, so the worst a crash costs is one idle tick fired again.
    fn rearm_idle(&self, now: u64, busy: &HashSet<String>) {
        let mut state = lock_or_poisoned(&self.state);
        for job in &mut state.jobs {
            if is_due_job(job, now) && !busy.contains(&job.session_id) && channel::idle(job) {
                job.next_run_at = next_run_at_for_schedule(&job.schedule, now).ok().flatten();
            }
        }
    }

    /// `Some` is a notice the ledger's sessions have not had yet.
    fn persist(&self, state: &ScheduleState) -> Option<String> {
        let text = serde_json::to_string_pretty(state).ok()?;
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let error = (yi_session::replace_file(&self.path, text.as_bytes()).err()).map(|error| {
            format!(
                "[heartbeat ledger {} not written: {error}; schedule changes hold in memory only and a restart loses them]",
                self.path.display()
            )
        });
        let told = std::mem::replace(&mut *lock_or_poisoned(&self.failed), error.clone());
        error.filter(|text| told.as_ref() != Some(text))
    }
}

pub struct JobSpec {
    pub id: String,
    pub session_id: String,
    pub cwd: String,
    pub source: yi_types::schedule::JobSource,
    pub delivery_mode: Option<DeliveryMode>,
    pub label: Option<String>,
    pub prompt: String,
    pub schedule: CronSchedule,
    pub next_run_at: u64,
    pub now_ms: u64,
}

pub fn new_job(spec: JobSpec) -> Job {
    Job {
        id: spec.id,
        status: JobStatus::Active,
        source: Some(spec.source),
        delivery_mode: spec.delivery_mode,
        session_id: spec.session_id,
        cwd: spec.cwd,
        label: spec.label,
        prompt: spec.prompt,
        schedule: spec.schedule,
        created_at: spec.now_ms,
        updated_at: spec.now_ms,
        next_run_at: Some(spec.next_run_at),
        last_run_at: None,
        last_skipped_at: None,
        last_error: None,
        run_count: 0,
        intent: Vec::new(),
        overlap: None,
        catch_up: None,
        unblocks: None,
        halted: false,
        channel: None,
        plan: None,
        extra: serde_json::Map::new(),
    }
}

/// An `Err` is a tick that ran and failed; the job records it as its last error.
pub type DeliverFn = dyn Fn(&Job, &Firing) -> Result<RunOutcome, String> + Send + Sync;
/// `Some` is the refusal of a command a subscription would run on the host.
pub type GateFn = dyn Fn(&str) -> Option<String> + Send + Sync;

/// Design §15.2 surfaces: the `/heartbeat` verbs and the kernel's
/// `rlm_heartbeat.*` vocabulary over one store.
pub struct HeartbeatService {
    store: std::sync::Mutex<std::sync::Arc<JobStore>>,
    session_id: std::sync::Mutex<String>,
    pub cwd: String,
    hub: std::sync::Mutex<Option<std::sync::Arc<shared::DeliveryHub>>>,
    /// Incident: jobs lived in `rlm-<pid>/`, so a restart lost every subscription; a root
    /// session's jobs live in `<root>/<session id>/` and its timer starts when it binds.
    durable: Option<(std::path::PathBuf, tokio::runtime::Handle)>,
    deliver: Option<std::sync::Arc<DeliverFn>>,
    stop: Option<std::sync::Arc<shared::StopFn>>,
    notice: Option<std::sync::Arc<shared::NoticeFn>>,
    words: Option<crate::goal::StoreHandle>,
    /// The machine's channel buffers, `<sessions>/channels/`.
    channels: Option<std::path::PathBuf>,
    gate: Option<std::sync::Arc<GateFn>>,
    /// Bound to a store every session of this process shares, so a halt reaches them all.
    interned: bool,
    /// Invariant: this session owns its interned timer from wiring until [`HeartbeatService::end`],
    /// so neither a sibling leaving nor a rebind's gap between lanes can stop it.
    wiring: std::sync::Mutex<Option<std::sync::Arc<shared::SharedSchedule>>>,
}

impl HeartbeatService {
    pub fn new(store: std::sync::Arc<JobStore>, cwd: impl Into<String>) -> Self {
        Self {
            store: std::sync::Mutex::new(store),
            session_id: std::sync::Mutex::new(String::new()),
            cwd: cwd.into(),
            hub: std::sync::Mutex::new(None),
            durable: None,
            deliver: None,
            stop: None,
            notice: None,
            words: None,
            channels: None,
            gate: None,
            interned: false,
            wiring: std::sync::Mutex::new(None),
        }
    }

    pub fn with_channels(mut self, home: std::path::PathBuf) -> Self {
        self.channels = Some(home);
        self
    }

    pub fn with_gate(mut self, gate: std::sync::Arc<GateFn>) -> Self {
        self.gate = Some(gate);
        self
    }

    pub fn with_words(mut self, words: crate::goal::StoreHandle) -> Self {
        self.words = Some(words);
        self
    }

    pub fn with_stop(mut self, stop: std::sync::Arc<dyn Fn(bool) -> bool + Send + Sync>) -> Self {
        self.stop = Some(stop);
        self
    }

    pub(crate) fn with_notice(mut self, notice: std::sync::Arc<shared::NoticeFn>) -> Self {
        self.notice = Some(notice);
        self
    }

    pub(crate) fn interned(mut self) -> Self {
        self.interned = true;
        self.wiring = std::sync::Mutex::new(shared::held(&self.store()));
        self
    }

    pub(crate) fn durable(mut self, root: std::path::PathBuf) -> Self {
        self.durable = tokio::runtime::Handle::try_current()
            .ok()
            .map(|runtime| (root, runtime));
        self
    }

    pub fn store(&self) -> std::sync::Arc<JobStore> {
        std::sync::Arc::clone(&lock_or_poisoned(&self.store))
    }

    fn hub(&self) -> Option<std::sync::Arc<shared::DeliveryHub>> {
        lock_or_poisoned(&self.hub).clone()
    }

    pub(crate) fn with_lane(
        mut self,
        hub: std::sync::Arc<shared::DeliveryHub>,
        deliver: std::sync::Arc<DeliverFn>,
    ) -> Self {
        self.hub = std::sync::Mutex::new(Some(hub));
        self.deliver = Some(deliver);
        self
    }

    fn session_id(&self) -> String {
        self.session_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Invariant: [`HeartbeatService::new`] starts unbound, and a job stamped with the empty
    /// id belongs to no lane, matches every other unbound session, and is skipped forever.
    fn bound_session_id(&self) -> Result<String, String> {
        let session_id = self.session_id();
        if session_id.is_empty() {
            return Err(
                "Heartbeats need a session: attach the session store before scheduling one"
                    .to_owned(),
            );
        }
        Ok(session_id)
    }

    pub fn bind_session(&self, session_id: String) {
        let mut bound = self
            .session_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *bound != session_id {
            self.withdraw(bound.as_str());
        }
        match &self.durable {
            Some((root, runtime)) if yi_session::validate_session_id(&session_id).is_ok() => {
                let _timer_spawns_here = runtime.enter();
                let path = jobs_path(root, &session_id);
                let shared = shared::intern(path, |hub| self.lane_into(hub, &session_id));
                *lock_or_poisoned(&self.store) = std::sync::Arc::clone(&shared.store);
                *lock_or_poisoned(&self.hub) = Some(std::sync::Arc::clone(&shared.hub));
                *lock_or_poisoned(&self.wiring) = Some(shared);
                self.ensure_mailbox(&session_id);
            }
            _ => {
                if let Some(hub) = self.hub() {
                    self.lane_into(&hub, &session_id);
                }
            }
        }
        *bound = session_id;
    }

    /// Invariant: the lane is in the hub before a fresh timer's first claim, which would
    /// otherwise find no lane, skip, and re-arm past the tick a restart owes.
    fn lane_into(&self, hub: &shared::DeliveryHub, session_id: &str) {
        if let Some(deliver) = &self.deliver {
            hub.register(session_id.to_owned(), std::sync::Arc::clone(deliver));
            if let Some(stop) = &self.stop {
                hub.register_stop(session_id.to_owned(), std::sync::Arc::clone(stop));
            }
            if let Some(notice) = &self.notice {
                hub.register_notice(session_id.to_owned(), std::sync::Arc::clone(notice));
            }
        }
    }

    /// Invariant: the interned timer outlives every session on its ledger, so a departing one
    /// loses its lane and pauses its Active jobs; left claimable they re-arm forever.
    fn withdraw(&self, session_id: &str) {
        let (Some(hub), Some(deliver)) = (self.hub(), &self.deliver) else {
            return;
        };
        if session_id.is_empty() || !hub.unregister(session_id, deliver) {
            return;
        }
        if self.durable.is_some() {
            // Its own file keeps the jobs Active for the next process; `end` stops its timer.
            return;
        }
        // Incident: an unconditional mutate persists, so every session that
        // never armed a heartbeat still seeded `rlm-{pid}/scheduled-jobs.json`.
        let claimable = |job: &Job| {
            job.session_id == session_id && is_heartbeat_job(job) && job.status == JobStatus::Active
        };
        let store = self.store();
        if !store.snapshot().jobs.iter().any(claimable) {
            return;
        }
        let now = yi_session::now_ms();
        store.mutate(|state| {
            for job in state.jobs.iter_mut().filter(|job| claimable(job)) {
                job.status = JobStatus::Paused;
                job.next_run_at = None;
                job.updated_at = now;
            }
        });
    }

    fn owns(&self, job: &Job) -> bool {
        job.session_id == self.session_id()
    }

    /// Invariant: the plan timer and kernel handlers outlive the session, so the id is cleared
    /// and every later write through this service is refused instead of persisting a stale ledger.
    pub(crate) fn end(&self) {
        let mut bound = lock_or_poisoned(&self.session_id);
        self.withdraw(&bound);
        bound.clear();
        lock_or_poisoned(&self.wiring).take();
    }
}

/// Invariant: a lane is withdrawn by whichever comes first, the session dropping or
/// [`HeartbeatService::bind_session`] rebinding; neither alone covers a rebind with no drop.
impl Drop for HeartbeatService {
    fn drop(&mut self) {
        self.end();
    }
}

fn local_time(utc_ms: u64) -> String {
    let zone = Zone::local();
    let wall = zone.wall_ms(utc_ms);
    let (year, month, day) = civil_from_days(wall / 86_400_000);
    let civil = civil_of(wall);
    format!(
        "{year}-{month:02}-{day:02} {:02}:{:02} {}",
        civil.hour,
        civil.minute,
        zone.abbreviation(utc_ms)
    )
}

fn render_job_line(job: &Job) -> String {
    format!(
        "{} [{}{}] {} — {} (runs: {}, next: {})",
        job.id,
        match &job.status {
            JobStatus::Active => "active",
            JobStatus::Paused => "paused",
            JobStatus::Completed => "completed",
            JobStatus::Cancelled => "cancelled",
            JobStatus::Other(status) => status.as_str(),
        },
        if job.halted { ", halted" } else { "" },
        job.channel
            .as_ref()
            .map_or(job.schedule.expression.as_str(), |sub| sub.address.as_str()),
        job.prompt,
        job.run_count,
        job.next_run_at
            .map(local_time)
            .unwrap_or_else(|| "-".to_owned()),
    )
}

impl HeartbeatService {
    fn heartbeat_jobs(state: &ScheduleState) -> Vec<&Job> {
        state
            .jobs
            .iter()
            .filter(|job| is_heartbeat_job(job) && job.status != JobStatus::Cancelled)
            .collect()
    }

    /// Applies one `/heartbeat` command; the returned text is the user-facing
    /// reply.
    pub fn apply(&self, command: &HeartbeatCommand, now_ms: u64) -> Result<String, String> {
        match command {
            HeartbeatCommand::Status => {
                let state = self.store().snapshot();
                let jobs: Vec<&Job> = Self::heartbeat_jobs(&state)
                    .into_iter()
                    .filter(|job| self.owns(job))
                    .collect();
                if jobs.is_empty() {
                    return Ok("No heartbeat is set.".to_owned());
                }
                Ok(jobs
                    .iter()
                    .map(|job| render_job_line(job))
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
            HeartbeatCommand::Halt => Ok(self.halt(true, now_ms)),
            HeartbeatCommand::Pause | HeartbeatCommand::Resume => {
                // Invariant: the halt's undo restores what it held and nothing more, so a job
                // paused before the halt stays paused.
                if matches!(command, HeartbeatCommand::Resume) {
                    let released = self.halt(false, now_ms);
                    if !released.is_empty() {
                        return Ok(released.trim_start().to_owned());
                    }
                }
                let target = if matches!(command, HeartbeatCommand::Pause) {
                    JobStatus::Paused
                } else {
                    JobStatus::Active
                };
                let owner = self.session_id();
                let changed = self.store().mutate(|state| {
                    let mut changed = 0_u64;
                    for job in &mut state.jobs {
                        if job.session_id != owner
                            || !is_heartbeat_job(job)
                            || matches!(
                                job.status,
                                JobStatus::Completed | JobStatus::Cancelled | JobStatus::Other(_)
                            )
                        {
                            continue;
                        }
                        job.status = target.clone();
                        if target == JobStatus::Active && job.next_run_at.is_none() {
                            job.next_run_at = next_run_at_for_schedule(&job.schedule, now_ms)
                                .ok()
                                .flatten();
                        }
                        job.updated_at = now_ms;
                        changed = changed.saturating_add(1);
                    }
                    changed
                });
                Ok(format!(
                    "{changed} heartbeat job(s) {}.",
                    if target == JobStatus::Paused {
                        "paused"
                    } else {
                        "resumed"
                    }
                ))
            }
            HeartbeatCommand::Clear => {
                let owner = self.session_id();
                let cleared = self.store().mutate(|state| {
                    let mut cleared = 0_u64;
                    for job in &mut state.jobs {
                        if job.session_id == owner
                            && is_heartbeat_job(job)
                            && job.status != JobStatus::Cancelled
                        {
                            job.status = JobStatus::Cancelled;
                            job.updated_at = now_ms;
                            cleared = cleared.saturating_add(1);
                        }
                    }
                    cleared
                });
                Ok(format!("{cleared} heartbeat job(s) cleared."))
            }
            HeartbeatCommand::Set {
                schedule,
                instruction,
                delivery_mode,
            } => {
                let (parsed, next_run_at) = parse_schedule(schedule, now_ms)?;
                let mut job = new_job(JobSpec {
                    id: format!("hb-{}", crate::subagent::random_suffix()?),
                    session_id: self.bound_session_id()?,
                    cwd: self.cwd.clone(),
                    source: yi_types::schedule::JobSource::Heartbeat,
                    delivery_mode: *delivery_mode,
                    label: None,
                    prompt: instruction.clone(),
                    schedule: parsed,
                    next_run_at,
                    now_ms,
                });
                job.intent = self.latest_words();
                let line = render_job_line(&job);
                self.store().mutate(|state| {
                    // One user heartbeat per session: `/heartbeat <x>` replaces.
                    for existing in &mut state.jobs {
                        if existing.source == Some(yi_types::schedule::JobSource::Heartbeat)
                            && existing.status == JobStatus::Active
                            && existing.session_id == job.session_id
                        {
                            existing.status = JobStatus::Cancelled;
                            existing.updated_at = now_ms;
                        }
                    }
                    state.jobs.push(job);
                });
                Ok(format!("Heartbeat set: {line}"))
            }
        }
    }

    /// Parses and applies one `/heartbeat` line, the entry point RPC, ACP and slash share (§15.2).
    pub fn run(&self, line: &str) -> Result<String, String> {
        parse_heartbeat_command(line).and_then(|command| self.apply(&command, yi_session::now_ms()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;

    /// Dies with a flapping disk told once for the store's whole life: a write that succeeds
    /// clears the told error, and a different error is told again.
    #[test]
    fn a_ledger_error_is_told_again_after_a_success_or_when_it_changes() -> std::io::Result<()> {
        let dir = Scratch::new("yi-ledger-flap")?;
        let parent = dir.join("schedules");
        let store = JobStore::open(parent.join("scheduled-jobs.json"));
        let state = ScheduleState::default();
        std::fs::write(&parent, "a file where the directory goes")?;
        let not_dir = store.persist(&state);
        let again = store.persist(&state);
        std::fs::remove_file(&parent)?;
        let healed = store.persist(&state);
        std::fs::remove_dir_all(&parent)?;
        std::fs::write(&parent, "a file where the directory goes")?;
        let relapse = store.persist(&state);
        std::fs::remove_file(&parent)?;
        std::fs::create_dir_all(parent.join("scheduled-jobs.json/held"))?;
        let is_dir = store.persist(&state);

        assert!(
            not_dir
                .as_ref()
                .is_some_and(|text| text.contains("not written"))
        );
        assert_eq!(again, None, "the same error was told twice");
        assert_eq!(healed, None, "a write that succeeded was told as a failure");
        assert_eq!(relapse, not_dir, "a failure after a success went untold");
        assert!(
            is_dir.is_some() && is_dir != not_dir,
            "a changed error went untold: {is_dir:?}"
        );
        Ok(())
    }
}
