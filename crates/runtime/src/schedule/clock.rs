use yi_types::message::{AgentMessage, UserContent};
use yi_types::plan::doc::{BlockedOn, Note, PlanId, TODO_LABEL_MAX, Todo, TodoLabel, TodoState};
use yi_types::schedule::{
    CLOCK_KEY, CatchUp, ClockStamp, CronSchedule, HALT_ENTRY_TYPE, HaltRecord, Job, JobStatus,
    Overlap, ScheduleKind,
};
use yi_types::todo::TodoList;
use yi_types::url::Url;

use super::{
    HeartbeatService, JobSpec, JobStore, RunOutcome, new_job, next_run_at_for_schedule,
    parse_iso_ms, parse_schedule, shared,
};
use crate::todo::{Op, TodoStore};

pub const CLOCK_SCHEME: &str = "clock://";
pub const CLOCK_ACTOR: &str = "clock";
/// How often an `External` probe's command runs as an `exec` wait; the lever `plan.probe_first_s`.
pub const PROBE_EVERY_S: u64 = 60;
/// The ceiling a wait's unchanged source backs off to; the lever `plan.probe_max_s`.
pub const PROBE_MAX_S: u64 = 1800;

/// Invariant: `catch_up: all` adds at most this many todos a claim; the last one's note counts
/// the rest, so a week asleep on a 10-minute clock cannot bury the list.
pub const CATCH_UP_ALL_MAX: usize = 24;

/// Every due time from the job's scheduled one to its claim, the first [`CATCH_UP_ALL_MAX`] listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Firing {
    pub ticks: Vec<u64>,
    pub last: u64,
    pub total: u64,
}

impl Firing {
    pub fn at(tick: u64) -> Self {
        Self {
            ticks: vec![tick],
            last: tick,
            total: 1,
        }
    }
}

pub(super) fn firing(schedule: &CronSchedule, first: u64, now: u64) -> Firing {
    let mut firing = Firing::at(first);
    while let Ok(Some(next)) = next_run_at_for_schedule(schedule, firing.last) {
        if next > now || next <= firing.last {
            break;
        }
        firing.last = next;
        firing.total = firing.total.saturating_add(1);
        if firing.ticks.len() < CATCH_UP_ALL_MAX {
            firing.ticks.push(next);
        }
    }
    firing
}

/// A wait's schedule: `at <ISO>` keeps its time even once past, so a restart after it fires at
/// once; anything else is the heartbeat grammar, counted from now.
pub fn wait_schedule(address: &str, now: u64) -> Result<(CronSchedule, u64), String> {
    let text = address
        .strip_prefix(CLOCK_SCHEME)
        .ok_or_else(|| format!("{address:?} is not a clock:// address"))?
        .trim();
    if let Some(when) = text.strip_prefix("at ").and_then(parse_iso_ms) {
        let schedule = CronSchedule {
            kind: ScheduleKind::Once,
            expression: text.to_owned(),
            interval_ms: None,
        };
        return Ok((schedule, when));
    }
    parse_schedule(text, now)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fired {
    Created(Vec<String>),
    Unblocked(TodoLabel),
    Held(String),
    /// A channel delivery: what it did to the list, and its messages rendered as fenced data.
    Delivered(Box<Fired>, String),
    /// A channel tick that found nothing past its ack.
    Idle,
    /// The channel's adapter is past its restart intensity; the subscription pauses.
    Stopped(String),
}

/// A blocked todo as [`HeartbeatService::arm`] takes it: the plan owning it, if one does.
pub type Wait = (Option<PlanId>, TodoLabel, BlockedOn);

/// What a blocked todo waits on as a channel address and filter. Invariant: an `External` probe
/// is an `exec` wait on its command's exit, so a plan written before channels still unblocks.
pub fn wait_of(on: &BlockedOn) -> Option<(String, Option<String>)> {
    match on {
        BlockedOn::Channel { address, filter } => Some((address.clone(), filter.clone())),
        BlockedOn::External {
            probe: Some(command),
        } => Some((
            format!(
                "exec://{}?every={}s",
                command.as_str(),
                crate::levers::get().plan_probe_first_s
            ),
            Some("ok=true".to_owned()),
        )),
        BlockedOn::Child(_) | BlockedOn::User | BlockedOn::External { probe: None } => None,
        BlockedOn::Other(_) => None,
    }
}

pub fn armed_command(on: &BlockedOn) -> Option<String> {
    wait_of(on).and_then(|(address, _)| super::adapter::exec_command(&address))
}

fn address(job: &Job) -> String {
    format!("{CLOCK_SCHEME}{}", job.schedule.expression)
}

fn stamp(item: &Todo) -> Option<ClockStamp> {
    serde_json::from_value(item.extra.get(CLOCK_KEY)?.clone()).ok()
}

/// A tick never runs saved code: it appends a todo or unblocks one, and the woken agent decides.
pub fn fire(todos: &TodoStore, job: &Job, firing: &Firing) -> Result<Fired, String> {
    match (&job.channel, &job.unblocks) {
        (Some(sub), _) => super::channel::fire(todos, job, sub),
        (None, Some(label)) => unblock(todos, job, label, CLOCK_ACTOR),
        (None, None) => create(todos, job, firing),
    }
}

pub(super) fn unblock(
    todos: &TodoStore,
    job: &Job,
    label: &TodoLabel,
    actor: &str,
) -> Result<Fired, String> {
    let unblocked = match (&job.plan, &job.label) {
        (Some(plan), Some(address)) => todos.unblock_in(plan, label, address)?,
        _ => {
            todos.resync();
            let waiting = todos.list().items().any(|item| {
                item.label == *label
                    && crate::todo::mirror::row_plan(item).is_none()
                    && matches!(&item.state, TodoState::Blocked { on, .. }
                        if wait_of(on).map(|(address, _)| address).as_ref() == job.label.as_ref())
            });
            if waiting {
                todos.unblock_as(label, actor)?;
            }
            waiting
        }
    };
    if !unblocked {
        return Ok(Fired::Held(format!(
            "todo {label} no longer waits on this address"
        )));
    }
    Ok(Fired::Unblocked(label.clone()))
}

/// Room the overlap policy leaves beside `open` earlier todos, or the hold it names.
pub(super) fn overlap_room(job: &Job, open: usize) -> Result<usize, String> {
    let (policy, cap) = match job.overlap.clone().unwrap_or(Overlap::Skip) {
        Overlap::Allow => ("allow", usize::MAX),
        Overlap::BufferOne => ("buffer_one", 2),
        Overlap::Skip | Overlap::Other(_) => ("skip", 1),
    };
    match cap.saturating_sub(open) {
        0 => Err(format!(
            "{open} todo(s) from earlier ticks still open and overlap is {policy}"
        )),
        room => Ok(room),
    }
}

/// `<label or instruction> @ <tick>`, the head cut to fit the label cap.
pub(super) fn label(job: &Job, tick: u64) -> Result<TodoLabel, String> {
    let suffix = format!(" @ {}", crate::plan::program::iso(tick));
    let base = job.label.as_deref().unwrap_or(&job.prompt);
    let head: String = base
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(TODO_LABEL_MAX.saturating_sub(suffix.chars().count()))
        .collect();
    TodoLabel::new(format!("{}{suffix}", head.trim_end())).map_err(|e| e.to_string())
}

fn create(todos: &TodoStore, job: &Job, firing: &Firing) -> Result<Fired, String> {
    let catch_up = job.catch_up.clone().unwrap_or(CatchUp::Once);
    let ticks = match catch_up {
        CatchUp::Skip if firing.total > 1 => {
            return Ok(Fired::Held(format!(
                "{} ticks came due at once and catch_up is skip",
                firing.total
            )));
        }
        CatchUp::All => firing.ticks.clone(),
        CatchUp::Once | CatchUp::Skip | CatchUp::Other(_) => vec![firing.last],
    };
    let open = todos
        .list()
        .items()
        .filter(|item| !item.state.is_terminal())
        .filter(|item| stamp(item).is_some_and(|stamp| stamp.job == job.id))
        .count();
    let room = match overlap_room(job, open) {
        Ok(room) => room,
        Err(held) => return Ok(Fired::Held(held)),
    };
    let chosen: Vec<u64> = ticks.into_iter().take(room).collect();
    let count = chosen.len();
    let items = chosen
        .iter()
        .enumerate()
        .map(|(at, tick)| todo_for(job, *tick, firing, &catch_up, at.saturating_add(1) == count))
        .collect::<Result<Vec<_>, _>>()?;
    let labels: Vec<TodoLabel> = items.iter().map(|item| item.label.clone()).collect();
    let applied = todos
        .apply_as(
            Op::Append {
                phase: None,
                under: None,
                items,
            },
            None,
            CLOCK_ACTOR,
        )
        .map_err(|error| error.to_string())?;
    Ok(Fired::Created(
        applied
            .list
            .items()
            .filter(|item| labels.contains(&item.label))
            .map(|item| {
                item.id
                    .as_ref()
                    .map_or_else(|| item.label.to_string(), ToString::to_string)
            })
            .collect(),
    ))
}

fn todo_for(
    job: &Job,
    tick: u64,
    firing: &Firing,
    catch_up: &CatchUp,
    last: bool,
) -> Result<Todo, String> {
    let label = label(job, tick)?;
    let mut note = job.prompt.clone();
    let once = !matches!(catch_up, CatchUp::All);
    if firing.total > 1 && (once || last) {
        let listed = u64::try_from(firing.ticks.len()).unwrap_or(u64::MAX);
        note.push_str(&match catch_up {
            CatchUp::All if firing.total > listed => format!(
                "\n[… {listed} of {} missed ticks made todos: catch_up all stops at {CATCH_UP_ALL_MAX}; the rest end at {}]",
                firing.total,
                crate::plan::program::iso(firing.last)
            ),
            CatchUp::All => String::new(),
            _ => format!(
                "\n{} ticks missed while asleep, {}–{}",
                firing.total,
                crate::plan::program::iso(firing.ticks.first().copied().unwrap_or(tick)),
                crate::plan::program::iso(firing.last)
            ),
        });
    }
    let mut todo = Todo::pending(label);
    todo.note = Some(Note::new(note).map_err(|e| e.to_string())?);
    todo.cites.intent = job.intent.clone();
    let stamp = ClockStamp {
        job: job.id.clone(),
        at: tick,
        ticks: if once { firing.total } else { 1 },
    };
    todo.extra.insert(
        CLOCK_KEY.to_owned(),
        serde_json::to_value(stamp).map_err(|e| e.to_string())?,
    );
    Ok(todo)
}

/// How the timer records a firing: a wait whose batch carried only refusals keeps waiting.
pub fn outcome(fired: &Fired) -> RunOutcome {
    match fired {
        Fired::Created(_) | Fired::Unblocked(_) => RunOutcome::Ran,
        Fired::Delivered(inner, _) if matches!(**inner, Fired::Held(_)) => RunOutcome::Idle,
        Fired::Delivered(..) => RunOutcome::Ran,
        Fired::Held(_) => RunOutcome::Skipped,
        Fired::Idle => RunOutcome::Idle,
        Fired::Stopped(_) => RunOutcome::Paused,
    }
}

/// The wake a tick sends: the address and the todo it touched, never the job's own text.
pub fn wake_message(job: &Job, fired: &Fired, now_ms: u64) -> Option<AgentMessage> {
    let what = |fired: &Fired| match fired {
        Fired::Created(ids) => format!(
            "added {} to your todo list; its label and note say what to do",
            ids.join(", ")
        ),
        Fired::Unblocked(label) => format!("unblocked todo {label}, which waited on it"),
        Fired::Held(why) => format!("left it blocked: {why}"),
        Fired::Delivered(..) | Fired::Idle | Fired::Stopped(_) => String::new(),
    };
    let text = match (fired, &job.channel) {
        (Fired::Delivered(inner, data), Some(sub)) => format!(
            "<channel job=\"{}\" address=\"{}\">messages arrived and {}.</channel>\n{data}",
            job.id,
            sub.address,
            what(inner)
        ),
        (Fired::Stopped(why), Some(sub)) => format!(
            "<channel job=\"{}\" address=\"{}\">{why}. The subscription is paused; once the adapter is fixed, resume it with rlm_heartbeat.update {{\"id\": \"{}\", \"status\": \"resume\"}}.</channel>",
            job.id, sub.address, job.id
        ),
        (Fired::Created(_) | Fired::Unblocked(_), _) => format!(
            "<heartbeat job=\"{}\" run=\"{}\">{} ticked at {} and {}.</heartbeat>",
            job.id,
            job.run_count.saturating_add(1),
            address(job),
            crate::plan::program::iso(now_ms),
            what(fired)
        ),
        _ => return None,
    };
    Some(AgentMessage::Custom {
        custom_type: "heartbeat_prompt".to_owned(),
        content: UserContent::Text(text),
        display: false,
        details: serde_json::to_value(serde_json::json!({
            "jobId": job.id,
            "schedule": job.schedule,
            "runCount": job.run_count,
            "nextRunAt": job.next_run_at,
        }))
        .ok(),
        timestamp: now_ms,
    })
}

/// Answers the jobs held or released and the sessions interrupted or released.
pub(crate) fn halt_store(
    store: &JobStore,
    hub: Option<&shared::DeliveryHub>,
    on: bool,
    now: u64,
) -> (u64, u64) {
    let moves = |job: &Job| {
        if on {
            job.status == JobStatus::Active && !job.halted
        } else {
            job.halted
        }
    };
    let mut jobs = 0_u64;
    if store.snapshot().jobs.iter().any(moves) {
        jobs = store.mutate(|state| {
            let mut changed = 0_u64;
            for job in state.jobs.iter_mut().filter(|job| moves(job)) {
                job.halted = on;
                job.updated_at = now;
                changed = changed.saturating_add(1);
            }
            changed
        });
    }
    (jobs, hub.map_or(0, |hub| hub.stop_all(on)))
}

impl HeartbeatService {
    pub(super) fn halt(&self, on: bool, now: u64) -> String {
        let (jobs, sessions) = if self.interned {
            shared::halt_all(on, now)
        } else {
            let (jobs, _) = halt_store(&self.store(), None, on, now);
            (
                jobs,
                self.stop.as_ref().map_or(0, |stop| u64::from(stop(on))),
            )
        };
        let adapters = super::adapter::halt(on);
        if !on && jobs == 0 && sessions == 0 {
            return String::new();
        }
        self.record_halt(&HaltRecord {
            halted: on,
            at: now,
            jobs,
            sessions,
            extra: serde_json::Map::new(),
        });
        if on {
            format!(
                "Halted every session this Yi serves: {jobs} clock job(s) held, {sessions} running turn(s) interrupted, {adapters} channel adapter(s) stopped, and no machine wake starts a turn. `/heartbeat resume` releases them."
            )
        } else {
            format!(" The halt is lifted: {jobs} clock job(s) and {sessions} session(s) released.")
        }
    }

    fn record_halt(&self, record: &HaltRecord) {
        let (Some(words), Ok(payload)) = (&self.words, serde_json::to_value(record)) else {
            return;
        };
        if let Some(session) = words() {
            let _a_ledger_write_never_fails_the_switch = yi_session::lock_session(&session)
                .append_custom("main", HALT_ENTRY_TYPE, Some(payload));
        }
    }

    /// The newest message the user typed on the live branch, cited by a subscription it set up.
    pub(super) fn latest_words(&self) -> Vec<Url> {
        let Some(store) = self.words.as_ref().and_then(|words| words()) else {
            return Vec::new();
        };
        let said = crate::plan::ask::said(&store);
        said.iter()
            .rposition(Option::is_some)
            .and_then(|at| format!("user://{}", at.saturating_add(1)).parse().ok())
            .into_iter()
            .collect()
    }

    /// Invariant: the todo is the wait's authority and the store a view of it, so a restart
    /// that lost `rlm-<pid>/` re-arms every wait from the rehydrated list.
    pub fn watch(&self, list: &TodoList) {
        let waits: Vec<Wait> = list
            .items()
            .filter_map(|item| match &item.state {
                TodoState::Blocked { on, .. } => Some((
                    crate::todo::mirror::row_plan(item),
                    item.label.clone(),
                    on.clone(),
                )),
                _ => None,
            })
            .collect();
        self.arm(&waits);
    }

    /// Arms each wait not armed yet; the plan timer hands in every plan's, sub-plans included.
    pub fn arm(&self, waits: &[Wait]) {
        let Ok(session) = self.bound_session_id() else {
            return;
        };
        let waits: Vec<(Option<&PlanId>, &TodoLabel, String, Option<String>)> = waits
            .iter()
            .filter_map(|(plan, label, on)| {
                wait_of(on).map(|(address, filter)| (plan.as_ref(), label, address, filter))
            })
            .collect();
        if waits.is_empty() {
            return;
        }
        let state = self.store().snapshot();
        let armed = |plan: Option<&PlanId>, label: &TodoLabel, address: &String| {
            state.jobs.iter().any(|job| {
                job.session_id == session
                    && job.unblocks.as_ref() == Some(label)
                    && job.label.as_ref() == Some(address)
                    && (job.plan.is_none() || job.plan.as_ref() == plan)
                    && matches!(job.status, JobStatus::Active | JobStatus::Paused)
            })
        };
        let now = yi_session::now_ms();
        let fresh: Vec<Job> = waits
            .iter()
            .filter(|(plan, label, address, _)| !armed(*plan, label, address))
            .filter_map(|(plan, label, address, filter)| {
                let mut job = self.wait_job(&session, label, address, filter, now)?;
                job.plan = plan.cloned();
                Some(job)
            })
            .collect();
        if !fresh.is_empty() {
            self.store().mutate(|state| state.jobs.extend(fresh));
        }
    }

    fn wait_job(
        &self,
        session: &str,
        label: &TodoLabel,
        address: &str,
        filter: &Option<String>,
        now: u64,
    ) -> Option<Job> {
        if !address.starts_with(CLOCK_SCHEME) {
            let spec = super::channel::Subscribe {
                address: address.to_owned(),
                filter: filter.clone(),
                label: Some(address.to_owned()),
                prompt: format!("unblock {label}"),
                ..Default::default()
            };
            return self.channel_job(session, spec, Some(label), now).ok();
        }
        let (schedule, next_run_at) = wait_schedule(address, now).ok()?;
        let mut job = new_job(JobSpec {
            id: format!("wait-{}", crate::subagent::random_suffix().ok()?),
            session_id: session.to_owned(),
            cwd: self.cwd.clone(),
            source: yi_types::schedule::JobSource::Heartbeat,
            delivery_mode: None,
            label: Some(address.to_owned()),
            prompt: format!("unblock {label}"),
            schedule,
            next_run_at,
            now_ms: now,
        });
        job.source = None;
        job.unblocks = Some(label.clone());
        Some(job)
    }
}
