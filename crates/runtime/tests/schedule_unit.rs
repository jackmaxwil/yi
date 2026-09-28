use std::collections::HashSet;

use yi_runtime::schedule::{
    JobSpec, ONE_MINUTE_MS, RunOutcome, SessionActivity, Zone, claim_due_in_state, new_job,
    parse_iso_ms, parse_schedule, parse_schedule_in, record_dispatch_result_in_state, should_defer,
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

    let (schedule, next) = parse_schedule_in("@hourly", now, &Zone::default())?;
    assert_eq!(schedule.expression, "0 * * * *");
    assert_eq!(next, now + 60 * ONE_MINUTE_MS);

    let (_, next) = parse_schedule_in("30 9 * * 1", now, &Zone::default())?;
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
    let (_, next) = parse_schedule_in("*/15 * * * *", now, &Zone::default())?;
    assert_eq!(next, now + 15 * ONE_MINUTE_MS);
    // Weekday 7 must behave as Sunday (weekday 0), six days ahead.
    let (_, next) = parse_schedule_in("0 0 * * 7", now, &Zone::default())?;
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
fn defer_table_matches_the_reference_rules() {
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
    let queued_behind_a_turn = SessionActivity {
        is_streaming: true,
        has_pending_session_work: true,
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
    assert!(should_defer(&heartbeat, &queued_behind_a_turn));
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
    let claimed = claim_due_in_state(&mut state, 1_000, 1_000, &mut new_id, &HashSet::new());
    assert_eq!(claimed.len(), 1);
    assert_eq!(state.dispatches.len(), 1);
    assert_eq!(
        state.jobs[0].next_run_at,
        Some(11_000),
        "claim must advance the schedule immediately"
    );

    state.jobs[0].next_run_at = Some(1_500);
    let doubled = claim_due_in_state(&mut state, 2_000, 2_000, &mut new_id, &HashSet::new());
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
    let claimed = claim_due_in_state(&mut state, 1_000, 1_000, &mut new_id, &HashSet::new());
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
fn claim_leaves_a_busy_session_due() {
    let mut state = ScheduleState::default();
    let mut job = job_with(Some(yi_types::schedule::JobSource::Heartbeat), None);
    job.session_id = "busy".to_owned();
    job.next_run_at = Some(1_000);
    state.jobs.push(job);
    let mut new_id = || "d1".to_owned();
    let skip = HashSet::from(["busy".to_owned()]);
    let claimed = claim_due_in_state(&mut state, 1_000, 1_000, &mut new_id, &skip);
    assert!(claimed.is_empty());
    assert!(state.dispatches.is_empty());
    assert_eq!(
        state.jobs[0].next_run_at,
        Some(1_000),
        "a busy lane must stay due rather than skip the slot"
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

/// A wait on a recurring address (`clock://every 1h`) means the next tick, not every tick.
#[test]
fn a_recurring_wait_completes_after_its_one_firing() {
    let mut job = job_with(None, None);
    job.unblocks = yi_types::plan::doc::TodoLabel::new("ship it").ok();
    let mut state = ScheduleState {
        jobs: vec![job],
        ..ScheduleState::default()
    };
    let claimed = claim_due_in_state(
        &mut state,
        MONDAY_MIDNIGHT,
        MONDAY_MIDNIGHT,
        || "dsp-1".to_owned(),
        &HashSet::new(),
    );
    let id = claimed
        .first()
        .map(|dispatch| dispatch.id.clone())
        .unwrap_or_default();
    let job =
        record_dispatch_result_in_state(&mut state, &id, RunOutcome::Ran, None, MONDAY_MIDNIGHT);
    assert_eq!(
        job.map(|job| job.status),
        Some(JobStatus::Completed),
        "a spent wait stayed armed and would re-fire every interval"
    );
}

#[test]
fn a_deferred_tick_stays_owed_and_retries_before_its_next_tick() {
    let mut state = ScheduleState::default();
    let mut job = job_with(Some(yi_types::schedule::JobSource::Heartbeat), None);
    job.schedule = CronSchedule {
        kind: ScheduleKind::Cron,
        expression: "0 9 * * 1-5".to_owned(),
        interval_ms: None,
    };
    job.next_run_at = Some(1_000);
    state.jobs.push(job);
    let mut new_id = || "d1".to_owned();
    let claimed = claim_due_in_state(&mut state, 1_000, 1_000, &mut new_id, &HashSet::new());
    assert_eq!(claimed.len(), 1);

    let updated =
        record_dispatch_result_in_state(&mut state, "d1", RunOutcome::Deferred, None, 2_000);

    let updated = updated.map(|job| (job.status, job.run_count, job.next_run_at));
    assert_eq!(
        updated,
        Some((
            JobStatus::Active,
            0,
            Some(2_000 + yi_runtime::schedule::DEFER_RETRY_MS)
        )),
        "a deferred weekday tick waited for the next weekday instead of retrying"
    );
}

fn instant(iso: &str) -> Result<u64, String> {
    parse_iso_ms(iso).ok_or_else(|| format!("bad instant {iso}"))
}

/// tzdata 2026c's compiled Los Angeles zone, so no case reads the runner's own zone.
fn los_angeles() -> Result<Zone, String> {
    Zone::from_tzif(include_bytes!("fixtures/zoneinfo/America_Los_Angeles"))
        .ok_or_else(|| "the fixture is not a TZif file".to_owned())
}

#[test]
fn a_nine_am_weekday_cron_fires_at_nine_local_on_both_sides_of_a_dst_change() -> Result<(), String>
{
    let zone = los_angeles()?;
    let (_, friday) = parse_schedule_in("0 9 * * 1-5", instant("2026-10-30T12:00Z")?, &zone)?;
    assert_eq!(friday, instant("2026-10-30T16:00Z")?, "09:00 PDT");
    let (_, monday) = parse_schedule_in("0 9 * * 1-5", friday, &zone)?;
    assert_eq!(
        monday,
        instant("2026-11-02T17:00Z")?,
        "09:00 PST after the fall-back"
    );
    let (_, spring) = parse_schedule_in("0 9 * * *", instant("2026-03-07T18:00Z")?, &zone)?;
    assert_eq!(
        spring,
        instant("2026-03-08T16:00Z")?,
        "09:00 PDT the morning DST starts"
    );
    Ok(())
}

#[test]
fn a_wall_time_dst_skips_waits_a_day_and_one_it_repeats_fires_once() -> Result<(), String> {
    let zone = los_angeles()?;
    let (_, gap) = parse_schedule_in("30 2 * * *", instant("2026-03-08T08:00Z")?, &zone)?;
    assert_eq!(
        gap,
        instant("2026-03-09T09:30Z")?,
        "02:30 does not exist on 03-08"
    );
    let (_, first) = parse_schedule_in("30 1 * * *", instant("2026-11-01T07:00Z")?, &zone)?;
    assert_eq!(first, instant("2026-11-01T08:30Z")?, "01:30 PDT");
    let (_, next) = parse_schedule_in("30 1 * * *", first, &zone)?;
    assert_eq!(
        next,
        instant("2026-11-02T09:30Z")?,
        "not again at 01:30 PST"
    );
    Ok(())
}
