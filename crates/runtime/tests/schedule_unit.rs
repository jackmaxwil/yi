use yi_runtime::schedule::{
    JobSpec, ONE_MINUTE_MS, RunOutcome, SessionActivity, claim_due_in_state, new_job, parse_iso_ms,
    parse_schedule, record_dispatch_result_in_state, should_defer,
};
use yi_types::schedule::{CronSchedule, DeliveryMode, Job, JobStatus, ScheduleKind, ScheduleState};

// 2026-08-24 is a Monday; 00:00:00 UTC epoch ms.
const MONDAY_MIDNIGHT: u64 = 1_787_529_600_000;

#[test]
fn parse_covers_in_every_at_alias_and_cron() -> Result<(), String> {
    let now = MONDAY_MIDNIGHT;
    let (schedule, next) = parse_schedule("in 5m", now)?;
    assert_eq!(schedule.kind, ScheduleKind::Once);
    assert_eq!(next, now + 5 * ONE_MINUTE_MS);

    let (schedule, next) = parse_schedule("every 10m", now)?;
    assert_eq!(schedule.interval_ms, Some(10 * ONE_MINUTE_MS));
    assert_eq!(next, now + 10 * ONE_MINUTE_MS);

    let (schedule, next) = parse_schedule("at 2026-08-24T01:30Z", now)?;
    assert_eq!(schedule.kind, ScheduleKind::Once);
    assert_eq!(next, now + 90 * ONE_MINUTE_MS);

    let (schedule, next) = parse_schedule("@hourly", now)?;
    assert_eq!(schedule.expression, "0 * * * *");
    assert_eq!(next, now + 60 * ONE_MINUTE_MS);

    let (_, next) = parse_schedule("30 9 * * 1", now)?;
    assert_eq!(
        next,
        now + (9 * 60 + 30) * ONE_MINUTE_MS,
        "a Monday cron from Monday midnight fires the same day"
    );

    assert_eq!(
        parse_schedule("", now).err().as_deref(),
        Some("Cron schedule cannot be empty")
    );
    assert_eq!(
        parse_schedule("every 5s", now).err().as_deref(),
        Some("Recurring interval must be at least 10 seconds")
    );
    assert_eq!(
        parse_schedule("at yesterday", now).err().as_deref(),
        Some("Invalid one-shot schedule. Use: at <ISO date>")
    );
    assert!(
        parse_schedule("not a schedule", now)
            .err()
            .is_some_and(|error| error.starts_with("Unsupported cron schedule")),
    );
    Ok(())
}

#[test]
fn cron_steps_ranges_and_sunday_alias_match() -> Result<(), String> {
    let now = MONDAY_MIDNIGHT;
    let (_, next) = parse_schedule("*/15 * * * *", now)?;
    assert_eq!(next, now + 15 * ONE_MINUTE_MS);
    // Weekday 7 must behave as Sunday (weekday 0), six days ahead.
    let (_, next) = parse_schedule("0 0 * * 7", now)?;
    assert_eq!(next, now + 6 * 24 * 60 * ONE_MINUTE_MS);
    Ok(())
}

fn job_with(source: Option<yi_types::schedule::JobSource>, mode: Option<DeliveryMode>) -> Job {
    let mut job = new_job(JobSpec {
        id: "j".to_owned(),
        session_id: "s".to_owned(),
        cwd: "/".to_owned(),
        source: source.unwrap_or(yi_types::schedule::JobSource::Cron),
        delivery_mode: mode,
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
    job.source = source;
    job
}

#[test]
fn defer_table_matches_prime_rules() {
    let heartbeat = job_with(Some(yi_types::schedule::JobSource::Heartbeat), None);
    let follow_up = job_with(
        Some(yi_types::schedule::JobSource::Heartbeat),
        Some(DeliveryMode::FollowUp),
    );
    let cron = job_with(Some(yi_types::schedule::JobSource::Cron), None);
    let idle = SessionActivity::default();
    let streaming = SessionActivity {
        is_streaming: true,
        ..SessionActivity::default()
    };
    let compacting = SessionActivity {
        is_compacting: true,
        ..SessionActivity::default()
    };
    let pending_actions = SessionActivity {
        unfinished_action_count: 1,
        ..SessionActivity::default()
    };
    assert!(!should_defer(&heartbeat, &idle));
    assert!(
        !should_defer(&heartbeat, &streaming),
        "steer must not defer on plain streaming"
    );
    assert!(
        should_defer(&follow_up, &streaming),
        "follow-up waits for the turn to finish"
    );
    assert!(should_defer(&heartbeat, &compacting));
    assert!(should_defer(&heartbeat, &pending_actions));
    assert!(
        !should_defer(&cron, &compacting),
        "non-heartbeat jobs never defer"
    );
}

#[test]
fn claim_advances_and_skips_already_claimed_jobs() {
    let mut state = ScheduleState::default();
    let mut job = job_with(Some(yi_types::schedule::JobSource::Heartbeat), None);
    job.next_run_at = Some(1_000);
    state.jobs.push(job);
    let mut counter = 0;
    let mut new_id = || {
        counter += 1;
        format!("d{counter}")
    };
    let claimed = claim_due_in_state(&mut state, 1_000, 1_000, &mut new_id);
    assert_eq!(claimed.len(), 1);
    assert_eq!(state.dispatches.len(), 1);
    assert_eq!(
        state.jobs[0].next_run_at,
        Some(11_000),
        "claim must advance the schedule immediately"
    );

    state.jobs[0].next_run_at = Some(1_500);
    let doubled = claim_due_in_state(&mut state, 2_000, 2_000, &mut new_id);
    assert!(
        doubled.is_empty(),
        "a job with an unresolved claim must be skipped, not double-delivered"
    );
    assert_eq!(state.jobs[0].last_skipped_at, Some(2_000));

    let updated = record_dispatch_result_in_state(&mut state, "d1", RunOutcome::Ran, None, 3_000);
    assert!(state.dispatches.is_empty());
    assert_eq!(updated.map(|job| job.run_count), Some(1));
}

#[test]
fn once_jobs_complete_and_clean_skips_do_not_count_runs() {
    let mut state = ScheduleState::default();
    let mut job = job_with(Some(yi_types::schedule::JobSource::Heartbeat), None);
    job.schedule = CronSchedule {
        kind: ScheduleKind::Once,
        expression: "in 1m".to_owned(),
        interval_ms: None,
    };
    job.next_run_at = Some(1_000);
    state.jobs.push(job);
    let mut new_id = || "d1".to_owned();
    let claimed = claim_due_in_state(&mut state, 1_000, 1_000, &mut new_id);
    assert_eq!(claimed.len(), 1);
    let updated =
        record_dispatch_result_in_state(&mut state, "d1", RunOutcome::Skipped, None, 2_000);
    let updated = updated.map(|job| (job.status, job.run_count, job.last_skipped_at));
    assert_eq!(
        updated,
        Some((JobStatus::Completed, 0, Some(2_000))),
        "a clean skip re-arms without counting a run; a one-shot completes"
    );
}

#[test]
fn iso_parse_accepts_utc_rejects_offsets() {
    assert_eq!(parse_iso_ms("1970-01-02"), Some(86_400_000));
    assert_eq!(parse_iso_ms("2026-08-24T00:00:00Z"), Some(MONDAY_MIDNIGHT));
    assert_eq!(
        parse_iso_ms("2026-08-24T01:30Z"),
        Some(MONDAY_MIDNIGHT + 90 * ONE_MINUTE_MS)
    );
    assert_eq!(parse_iso_ms("not a date"), None);
    assert_eq!(parse_iso_ms("2026-13-01"), None);
}
