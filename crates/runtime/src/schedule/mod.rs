mod lanes;
pub(crate) mod shared;

use std::collections::HashSet;

pub use lanes::Scheduler;

use yi_types::schedule::{
    CronSchedule, DeliveryMode, DispatchRecord, Job, JobStatus, ScheduleKind, ScheduleState,
};

pub const ONE_SECOND_MS: u64 = 1_000;
pub const ONE_MINUTE_MS: u64 = 60_000;
pub const DEFAULT_HEARTBEAT_SCHEDULE: &str = "every 5m";
pub const DEFAULT_HEARTBEAT_DELIVERY_MODE: DeliveryMode = DeliveryMode::Steer;
pub const HEARTBEAT_USAGE: &str =
    "Usage: /heartbeat [--every <interval>] [--steer|--follow-up] <instruction>";
// The exact recovery marker; tests and `/heartbeat status` surface it.
pub const INTERRUPTED_ERROR: &str = "Interrupted before scheduled operation completion";

fn strip_matching_quotes(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

fn normalize_cron_alias(text: &str) -> &str {
    match text {
        "@hourly" => "0 * * * *",
        "@daily" => "0 0 * * *",
        "@weekly" => "0 0 * * 0",
        "@monthly" => "0 0 1 * *",
        other => other,
    }
}

fn split_amount_unit(text: &str) -> Option<(u64, &str)> {
    let digits_end = text.find(|ch: char| !ch.is_ascii_digit())?;
    if digits_end == 0 {
        return None;
    }
    let amount: u64 = text[..digits_end].parse().ok()?;
    let unit = text[digits_end..].trim();
    Some((amount, unit))
}

const MINUTE_UNITS: [&str; 5] = ["m", "min", "mins", "minute", "minutes"];
const HOUR_UNITS: [&str; 5] = ["h", "hr", "hrs", "hour", "hours"];
const DAY_UNITS: [&str; 3] = ["d", "day", "days"];
const SECOND_UNITS: [&str; 5] = ["s", "sec", "secs", "second", "seconds"];

fn is_unit(unit: &str, table: &[&str]) -> bool {
    table.iter().any(|entry| unit.eq_ignore_ascii_case(entry))
}

fn parse_in_clause(text: &str) -> Option<u64> {
    let rest = text
        .strip_prefix("in ")
        .or_else(|| text.strip_prefix("In "))
        .or_else(|| text.strip_prefix("IN "))?;
    let (amount, unit) = split_amount_unit(rest.trim())?;
    let multiplier = if is_unit(unit, &MINUTE_UNITS) {
        ONE_MINUTE_MS
    } else if is_unit(unit, &HOUR_UNITS) {
        60 * ONE_MINUTE_MS
    } else if is_unit(unit, &DAY_UNITS) {
        24 * 60 * ONE_MINUTE_MS
    } else {
        return None;
    };
    amount.checked_mul(multiplier)
}

fn parse_every_clause(text: &str) -> Option<Result<u64, String>> {
    let lowered = text.to_ascii_lowercase();
    let rest = lowered
        .strip_prefix("every ")
        .or_else(|| lowered.strip_prefix("each "))?;
    let (amount, unit) = split_amount_unit(rest.trim())?;
    let multiplier = if is_unit(unit, &SECOND_UNITS) {
        ONE_SECOND_MS
    } else if is_unit(unit, &MINUTE_UNITS) {
        ONE_MINUTE_MS
    } else if is_unit(unit, &HOUR_UNITS) {
        60 * ONE_MINUTE_MS
    } else {
        return None;
    };
    let interval_ms = amount.checked_mul(multiplier)?;
    if interval_ms < 10 * ONE_SECOND_MS {
        return Some(Err(
            "Recurring interval must be at least 10 seconds".to_owned()
        ));
    }
    Some(Ok(interval_ms))
}

/// `"in 5m"`, `"every 10m"`, `"at <ISO>"`, `@hourly`-style aliases, or a
/// five-field cron expression, with its first run in epoch ms.
pub fn parse_schedule(input: &str, now_ms: u64) -> Result<(CronSchedule, u64), String> {
    let text = strip_matching_quotes(input.trim()).trim();
    if text.is_empty() {
        return Err("Cron schedule cannot be empty".to_owned());
    }
    if let Some(delay) = parse_in_clause(text) {
        return Ok((
            CronSchedule {
                kind: ScheduleKind::Once,
                expression: text.to_owned(),
                interval_ms: None,
            },
            now_ms.saturating_add(delay),
        ));
    }
    if let Some(every) = parse_every_clause(text) {
        let interval_ms = every?;
        return Ok((
            CronSchedule {
                kind: ScheduleKind::Interval,
                expression: text.to_owned(),
                interval_ms: Some(interval_ms),
            },
            now_ms.saturating_add(interval_ms),
        ));
    }
    if text.len() >= 3 && text[..3].eq_ignore_ascii_case("at ") {
        let when = parse_iso_ms(text[3..].trim())
            .ok_or_else(|| "Invalid one-shot schedule. Use: at <ISO date>".to_owned())?;
        if when <= now_ms {
            return Err("One-shot schedule must be in the future".to_owned());
        }
        return Ok((
            CronSchedule {
                kind: ScheduleKind::Once,
                expression: text.to_owned(),
                interval_ms: None,
            },
            when,
        ));
    }
    let expression = normalize_cron_alias(text);
    let next = next_cron_run_after(expression, now_ms)?;
    Ok((
        CronSchedule {
            kind: ScheduleKind::Cron,
            expression: expression.to_owned(),
            interval_ms: None,
        },
        next,
    ))
}

/// None for a spent one-shot.
pub fn next_run_at_for_schedule(
    schedule: &CronSchedule,
    after_ms: u64,
) -> Result<Option<u64>, String> {
    match schedule.kind {
        ScheduleKind::Once => Ok(None),
        ScheduleKind::Interval => {
            let interval = schedule.interval_ms.unwrap_or(0);
            if interval == 0 {
                return Err(format!(
                    "Invalid interval schedule: {}",
                    schedule.expression
                ));
            }
            Ok(Some(after_ms.saturating_add(interval)))
        }
        ScheduleKind::Cron => Ok(Some(next_cron_run_after(&schedule.expression, after_ms)?)),
    }
}

struct CronFields {
    minute: HashSet<u64>,
    hour: HashSet<u64>,
    day_of_month: HashSet<u64>,
    month: HashSet<u64>,
    day_of_week: HashSet<u64>,
}

fn parse_cron_number(value: &str, min: u64, max: u64) -> Result<u64, String> {
    if value.is_empty() || !value.chars().all(|ch| ch.is_ascii_digit()) {
        return Err(format!("Invalid cron number: {value}"));
    }
    let parsed: u64 = value
        .parse()
        .map_err(|_| format!("Invalid cron number: {value}"))?;
    if parsed < min || parsed > max {
        return Err(format!("Cron number out of range: {value}"));
    }
    Ok(parsed)
}

fn parse_cron_field(field: &str, min: u64, max: u64) -> Result<HashSet<u64>, String> {
    let mut values = HashSet::new();
    for part in field.split(',') {
        if part.is_empty() {
            return Err(format!("Invalid cron field: {field}"));
        }
        let (range_text, step_text) = match part.split_once('/') {
            Some((range, step)) => (range, Some(step)),
            None => (part, None),
        };
        let step = match step_text {
            Some(step) => parse_cron_number(step, 1, max)?,
            None => 1,
        };
        let (start, end) = if range_text == "*" {
            (min, max)
        } else if let Some((start_text, end_text)) = range_text.split_once('-') {
            let start = parse_cron_number(start_text, min, max)?;
            let end = parse_cron_number(end_text, min, max)?;
            if start > end {
                return Err(format!("Invalid cron range: {range_text}"));
            }
            (start, end)
        } else {
            let single = parse_cron_number(range_text, min, max)?;
            (single, single)
        };
        let mut value = start;
        while value <= end {
            values.insert(value);
            match value.checked_add(step) {
                Some(next) => value = next,
                None => break,
            }
        }
    }
    Ok(values)
}

fn parse_cron_expression(expression: &str) -> Result<CronFields, String> {
    let parts: Vec<&str> = expression.split_whitespace().collect();
    if parts.len() != 5 {
        return Err(
            "Unsupported cron schedule. Use 'in 10m', 'at <ISO date>', @hourly, or five fields: minute hour day month weekday"
                .to_owned(),
        );
    }
    Ok(CronFields {
        minute: parse_cron_field(parts[0], 0, 59)?,
        hour: parse_cron_field(parts[1], 0, 23)?,
        day_of_month: parse_cron_field(parts[2], 1, 31)?,
        month: parse_cron_field(parts[3], 1, 12)?,
        day_of_week: parse_cron_field(parts[4], 0, 7)?,
    })
}

// Cron and `at` evaluate in UTC on purpose: std has no tzdata and no date crate is
// an allowed dependency (§18.3), so local time would need a table Yi does not carry.
struct Civil {
    minute: u64,
    hour: u64,
    day: u64,
    month: u64,
    weekday: u64,
}

fn civil_of(ms: u64) -> Civil {
    let seconds = ms / 1_000;
    let days = seconds / 86_400;
    let (_, month, day) = civil_from_days(days);
    Civil {
        minute: (seconds % 3_600) / 60,
        hour: (seconds % 86_400) / 3_600,
        day,
        month,
        // 1970-01-01 was a Thursday.
        weekday: (days + 4) % 7,
    }
}

use yi_kernel::client::civil_from_days;

fn days_from_civil(year: u64, month: u64, day: u64) -> u64 {
    // Hinnant days-from-civil, valid for the post-1970 range Yi uses.
    let adjusted_year = if month <= 2 { year - 1 } else { year };
    let era = adjusted_year / 400;
    let yoe = adjusted_year % 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn matches_cron_fields(civil: &Civil, fields: &CronFields) -> bool {
    let day_matches = fields.day_of_week.contains(&civil.weekday)
        || (civil.weekday == 0 && fields.day_of_week.contains(&7));
    fields.minute.contains(&civil.minute)
        && fields.hour.contains(&civil.hour)
        && fields.day_of_month.contains(&civil.day)
        && fields.month.contains(&civil.month)
        && day_matches
}

fn next_cron_run_after(expression: &str, after_ms: u64) -> Result<u64, String> {
    let fields = parse_cron_expression(expression)?;
    // Truncate to the minute, then step forward one minute at a time.
    let mut candidate = (after_ms / ONE_MINUTE_MS).saturating_add(1) * ONE_MINUTE_MS;
    let deadline = candidate.saturating_add(366 * 24 * 60 * ONE_MINUTE_MS);
    while candidate <= deadline {
        if matches_cron_fields(&civil_of(candidate), &fields) {
            return Ok(candidate);
        }
        candidate = candidate.saturating_add(ONE_MINUTE_MS);
    }
    Err(format!(
        "Cron schedule did not match within one year: {expression}"
    ))
}

/// Minimal UTC ISO-8601 parse: `YYYY-MM-DD[THH:MM[:SS]][Z]`. Offsets other
/// than Z are rejected — Yi schedules in UTC.
pub fn parse_iso_ms(text: &str) -> Option<u64> {
    let text = text.trim().trim_end_matches('Z');
    let (date, time) = match text.split_once('T') {
        Some((date, time)) => (date, Some(time)),
        None => (text, None),
    };
    let mut date_parts = date.split('-');
    let year: u64 = date_parts.next()?.parse().ok()?;
    let month: u64 = date_parts.next()?.parse().ok()?;
    let day: u64 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (hour, minute, second) = match time {
        None => (0, 0, 0),
        Some(time) => {
            let mut parts = time.split(':');
            let hour: u64 = parts.next()?.parse().ok()?;
            let minute: u64 = parts.next()?.parse().ok()?;
            let second: u64 = match parts.next() {
                Some(second) => second.parse().ok()?,
                None => 0,
            };
            if parts.next().is_some() || hour > 23 || minute > 59 || second > 60 {
                return None;
            }
            (hour, minute, second)
        }
    };
    if year < 1970 {
        return None;
    }
    let days = days_from_civil(year, month, day);
    Some((days * 86_400 + hour * 3_600 + minute * 60 + second) * 1_000)
}

/// Parsed form of the `/heartbeat` slash command; the kernel's `rlm_heartbeat` calls and the
/// ACP `_yi/heartbeat` method reach the same scheduler through this one grammar (§15.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeartbeatCommand {
    Status,
    Pause,
    Resume,
    Clear,
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

pub fn parse_heartbeat_command(input: &str) -> Result<HeartbeatCommand, String> {
    let text = input.strip_prefix("/heartbeat").unwrap_or(input).trim();
    match text {
        "" | "status" => return Ok(HeartbeatCommand::Status),
        "pause" => return Ok(HeartbeatCommand::Pause),
        "resume" => return Ok(HeartbeatCommand::Resume),
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
    pub is_retrying: bool,
    pub is_bash_running: bool,
    pub has_pending_session_work: bool,
    pub unfinished_action_count: u64,
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
    if !is_heartbeat_job(job) {
        return false;
    }
    // States where delivering a heartbeat is unsafe or would stack redundant
    // work, regardless of delivery mode.
    let busy_besides_streaming = activity.is_compacting
        || activity.is_retrying
        || activity.is_bash_running
        || activity.has_pending_session_work
        || (!activity.is_streaming && activity.unfinished_action_count > 0);
    if busy_besides_streaming {
        return true;
    }
    // "steer" heartbeats interrupt the current turn, so a plain streaming turn
    // must not defer them; "follow_up" heartbeats wait, so streaming defers.
    if job.delivery_mode.unwrap_or(DEFAULT_HEARTBEAT_DELIVERY_MODE) == DeliveryMode::Steer {
        return false;
    }
    activity.is_streaming
}

fn is_due_job(job: &Job, now_ms: u64) -> bool {
    job.status == JobStatus::Active && job.next_run_at.is_some_and(|at| at <= now_ms)
}

pub struct ClaimedDispatch {
    pub id: String,
    pub job: Job,
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
        });
    }
    dispatches
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    Ran,
    Skipped,
}

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
        if job.schedule.kind == ScheduleKind::Once {
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

/// The exact text a scheduled job puts in front of the model; the element wrapper is what
/// tells the model this turn was machine-triggered rather than typed by the user (§15.2).
pub fn heartbeat_text(job: &Job) -> String {
    format!(
        "<heartbeat job=\"{}\" run=\"{}\">{}</heartbeat>",
        job.label.as_deref().unwrap_or(&job.id),
        job.run_count.saturating_add(1),
        job.prompt
    )
}

pub struct JobStore {
    path: std::path::PathBuf,
    state: std::sync::Mutex<ScheduleState>,
    changed: std::sync::Arc<tokio::sync::Notify>,
}

fn lock_state(state: &std::sync::Mutex<ScheduleState>) -> std::sync::MutexGuard<'_, ScheduleState> {
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
        }
    }

    pub fn changed(&self) -> std::sync::Arc<tokio::sync::Notify> {
        std::sync::Arc::clone(&self.changed)
    }

    pub fn snapshot(&self) -> ScheduleState {
        lock_state(&self.state).clone()
    }

    /// Every mutation persists (tmp + fsync + rename) before notifying the
    /// timer — the claim-before-deliver contract (§15.2) rides on this ordering.
    pub fn mutate<R>(&self, action: impl FnOnce(&mut ScheduleState) -> R) -> R {
        let result = {
            let mut state = lock_state(&self.state);
            let result = action(&mut state);
            self.persist(&state);
            result
        };
        self.changed.notify_waiters();
        result
    }

    fn persist(&self, state: &ScheduleState) {
        let Ok(text) = serde_json::to_string_pretty(state) else {
            return;
        };
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = self.path.with_extension("json.tmp");
        let write = std::fs::write(&tmp, text.as_bytes()).and_then(|()| {
            let file = std::fs::File::open(&tmp)?;
            file.sync_all()?;
            std::fs::rename(&tmp, &self.path)
        });
        if let Err(error) = write {
            eprintln!("scheduled-jobs.json write failed: {error}");
        }
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
        extra: serde_json::Map::new(),
    }
}

pub type DeliverFn = dyn Fn(&Job) -> RunOutcome + Send + Sync;

/// Design §15.2: the persisted heartbeat message — `custom{heartbeat_prompt}`
/// with the job's identity in details.
pub fn heartbeat_message(job: &Job, now_ms: u64) -> yi_types::message::AgentMessage {
    yi_types::message::AgentMessage::Custom {
        custom_type: "heartbeat_prompt".to_owned(),
        content: yi_types::message::UserContent::Text(heartbeat_text(job)),
        display: false,
        details: serde_json::to_value(serde_json::json!({
            "jobId": job.id,
            "schedule": job.schedule,
            "runCount": job.run_count,
            "nextRunAt": job.next_run_at,
        }))
        .ok(),
        timestamp: now_ms,
    }
}

/// Design §15.2 surfaces: the `/heartbeat` verbs and the kernel's
/// `rlm_heartbeat.*` vocabulary over one store.
pub struct HeartbeatService {
    pub store: std::sync::Arc<JobStore>,
    session_id: std::sync::Mutex<String>,
    pub cwd: String,
    hub: Option<std::sync::Arc<shared::DeliveryHub>>,
    deliver: Option<std::sync::Arc<DeliverFn>>,
}

impl HeartbeatService {
    pub fn new(store: std::sync::Arc<JobStore>, cwd: impl Into<String>) -> Self {
        Self {
            store,
            session_id: std::sync::Mutex::new(String::new()),
            cwd: cwd.into(),
            hub: None,
            deliver: None,
        }
    }

    pub(crate) fn with_lane(
        mut self,
        hub: std::sync::Arc<shared::DeliveryHub>,
        deliver: std::sync::Arc<DeliverFn>,
    ) -> Self {
        self.hub = Some(hub);
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
        if let (Some(hub), Some(deliver)) = (&self.hub, &self.deliver) {
            hub.register(session_id.clone(), std::sync::Arc::clone(deliver));
        }
        *bound = session_id;
    }

    /// Invariant: the interned timer outlives every session on its ledger, so a departing one
    /// loses its lane and pauses its Active jobs; left claimable they re-arm forever.
    fn withdraw(&self, session_id: &str) {
        let Some(hub) = &self.hub else { return };
        if session_id.is_empty() {
            return;
        }
        hub.unregister(session_id);
        // Incident: an unconditional mutate persists, so every session that
        // never armed a heartbeat still seeded `rlm-{pid}/scheduled-jobs.json`.
        let claimable = |job: &Job| {
            job.session_id == session_id && is_heartbeat_job(job) && job.status == JobStatus::Active
        };
        if !self.store.snapshot().jobs.iter().any(claimable) {
            return;
        }
        let now = yi_session::now_ms();
        self.store.mutate(|state| {
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
}

/// Invariant: a lane is withdrawn by whichever comes first, the session dropping or
/// [`HeartbeatService::bind_session`] rebinding; neither alone covers a rebind with no drop.
impl Drop for HeartbeatService {
    fn drop(&mut self) {
        self.withdraw(&self.session_id());
    }
}

fn render_job_line(job: &Job) -> String {
    format!(
        "{} [{}] {} — {} (runs: {}, next: {})",
        job.id,
        match job.status {
            JobStatus::Active => "active",
            JobStatus::Paused => "paused",
            JobStatus::Completed => "completed",
            JobStatus::Cancelled => "cancelled",
        },
        job.schedule.expression,
        job.prompt,
        job.run_count,
        job.next_run_at
            .map(|at| at.to_string())
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
                let state = self.store.snapshot();
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
            HeartbeatCommand::Pause | HeartbeatCommand::Resume => {
                let target = if matches!(command, HeartbeatCommand::Pause) {
                    JobStatus::Paused
                } else {
                    JobStatus::Active
                };
                let owner = self.session_id();
                let changed = self.store.mutate(|state| {
                    let mut changed = 0_u64;
                    for job in &mut state.jobs {
                        if job.session_id != owner
                            || !is_heartbeat_job(job)
                            || matches!(job.status, JobStatus::Completed | JobStatus::Cancelled)
                        {
                            continue;
                        }
                        job.status = target;
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
                let cleared = self.store.mutate(|state| {
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
                let job = new_job(JobSpec {
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
                let line = render_job_line(&job);
                self.store.mutate(|state| {
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

    /// Registers the kernel-side vocabulary (design §15.2): list, create,
    /// update (pause/resume), delete.
    pub fn register(self: &std::sync::Arc<Self>, registry: &mut crate::kernel::HostRegistry) {
        let list = std::sync::Arc::clone(self);
        registry.register("rlm_heartbeat.list", move |_payload| {
            let state = list.store.snapshot();
            let jobs: Vec<_> = state
                .jobs
                .iter()
                .filter(|job| {
                    job.source == Some(yi_types::schedule::JobSource::RlmHeartbeat)
                        && list.owns(job)
                })
                .collect();
            let reply = serde_json::json!({"jobs": jobs})
                .as_object()
                .cloned()
                .unwrap_or_default();
            Box::pin(async move { Ok(reply) })
        });
        let create = std::sync::Arc::clone(self);
        registry.register("rlm_heartbeat.create", move |payload| {
            let result = (|| {
                let schedule_text = payload
                    .get("schedule")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("rlm_heartbeat.create requires a schedule")?;
                let prompt = payload
                    .get("prompt")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("rlm_heartbeat.create requires a prompt")?;
                let label = payload
                    .get("label")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
                let delivery = match payload
                    .get("deliveryMode")
                    .and_then(serde_json::Value::as_str)
                {
                    None => None,
                    Some("steer") => Some(DeliveryMode::Steer),
                    Some("follow_up") => Some(DeliveryMode::FollowUp),
                    Some(_) => {
                        return Err(
                            "Heartbeat delivery mode must be \"steer\" or \"follow_up\"".to_owned()
                        );
                    }
                };
                let now = yi_session::now_ms();
                let (parsed, next_run_at) =
                    parse_schedule(&normalize_heartbeat_schedule(Some(schedule_text)), now)?;
                let job = new_job(JobSpec {
                    id: format!("rhb-{}", crate::subagent::random_suffix()?),
                    session_id: create.bound_session_id()?,
                    cwd: create.cwd.clone(),
                    source: yi_types::schedule::JobSource::RlmHeartbeat,
                    delivery_mode: delivery,
                    label,
                    prompt: prompt.to_owned(),
                    schedule: parsed,
                    next_run_at,
                    now_ms: now,
                });
                create.store.mutate(|state| state.jobs.push(job.clone()));
                serde_json::json!({"job": job})
                    .as_object()
                    .cloned()
                    .ok_or_else(|| "serialization failed".to_owned())
            })();
            Box::pin(async move { result })
        });
        let update = std::sync::Arc::clone(self);
        registry.register("rlm_heartbeat.update", move |payload| {
            let result = (|| {
                let id = payload
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("rlm_heartbeat.update requires an id")?
                    .to_owned();
                let target = match payload.get("status").and_then(serde_json::Value::as_str) {
                    Some("pause") => JobStatus::Paused,
                    Some("resume") => JobStatus::Active,
                    _ => {
                        return Err(
                            "rlm_heartbeat.update status must be \"pause\" or \"resume\""
                                .to_owned(),
                        );
                    }
                };
                let now = yi_session::now_ms();
                let owner = update.bound_session_id()?;
                let updated = update.store.mutate(|state| {
                    let found = state
                        .jobs
                        .iter_mut()
                        .find(|job| job.id == id && job.session_id == owner);
                    found.map(|job| {
                        job.status = target;
                        if target == JobStatus::Active && job.next_run_at.is_none() {
                            job.next_run_at =
                                next_run_at_for_schedule(&job.schedule, now).ok().flatten();
                        }
                        job.updated_at = now;
                        job.clone()
                    })
                });
                let job = updated.ok_or(format!("unknown heartbeat job: {id}"))?;
                serde_json::json!({"job": job})
                    .as_object()
                    .cloned()
                    .ok_or_else(|| "serialization failed".to_owned())
            })();
            Box::pin(async move { result })
        });
        let delete = std::sync::Arc::clone(self);
        registry.register("rlm_heartbeat.delete", move |payload| {
            let result = (|| {
                let id = payload
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("rlm_heartbeat.delete requires an id")?
                    .to_owned();
                let now = yi_session::now_ms();
                let owner = delete.bound_session_id()?;
                let found = delete.store.mutate(|state| {
                    let target = state
                        .jobs
                        .iter_mut()
                        .find(|job| job.id == id && job.session_id == owner);
                    target.map(|job| {
                        job.status = JobStatus::Cancelled;
                        job.next_run_at = None;
                        job.updated_at = now;
                    })
                });
                if found.is_none() {
                    return Err(format!("unknown heartbeat job: {id}"));
                }
                serde_json::json!({"deleted": true})
                    .as_object()
                    .cloned()
                    .ok_or_else(|| "serialization failed".to_owned())
            })();
            Box::pin(async move { result })
        });
    }
}
