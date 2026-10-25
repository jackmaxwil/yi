use std::collections::HashSet;

use yi_kernel::client::civil_from_days;
use yi_types::schedule::{CronSchedule, ScheduleKind};

use super::{ONE_MINUTE_MS, ONE_SECOND_MS, Zone};

fn strip_matching_quotes(value: &str) -> &str {
    ['"', '\'']
        .iter()
        .find_map(|quote| value.strip_prefix(*quote)?.strip_suffix(*quote))
        .unwrap_or(value)
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

pub(super) fn split_amount_unit(text: &str) -> Option<(u64, &str)> {
    let digits_end = text.find(|ch: char| !ch.is_ascii_digit())?;
    if digits_end == 0 {
        return None;
    }
    let (amount, unit) = text.split_at_checked(digits_end)?;
    Some((amount.parse().ok()?, unit.trim()))
}

pub(super) const MINUTE_UNITS: [&str; 5] = ["m", "min", "mins", "minute", "minutes"];
pub(super) const HOUR_UNITS: [&str; 5] = ["h", "hr", "hrs", "hour", "hours"];
const DAY_UNITS: [&str; 3] = ["d", "day", "days"];
pub(super) const SECOND_UNITS: [&str; 5] = ["s", "sec", "secs", "second", "seconds"];

pub(super) fn is_unit(unit: &str, table: &[&str]) -> bool {
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

pub fn parse_schedule(input: &str, now_ms: u64) -> Result<(CronSchedule, u64), String> {
    parse_schedule_in(input, now_ms, &Zone::local())
}

/// `"in 5m"`, `"every 10m"`, `"at <ISO>"`, `@hourly`-style aliases, or a five-field cron
/// expression in `zone`'s wall time, with its first run in epoch ms.
pub fn parse_schedule_in(
    input: &str,
    now_ms: u64,
    zone: &Zone,
) -> Result<(CronSchedule, u64), String> {
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
    if text
        .get(..3)
        .is_some_and(|head| head.eq_ignore_ascii_case("at "))
    {
        let when = parse_iso_ms(text.get(3..).unwrap_or_default().trim())
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
    let next = next_cron_run_after(expression, now_ms, zone)?;
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
        ScheduleKind::Cron => Ok(Some(next_cron_run_after(
            &schedule.expression,
            after_ms,
            &Zone::local(),
        )?)),
    }
}

pub(super) struct CronFields {
    minute: HashSet<u64>,
    hour: HashSet<u64>,
    day_of_month: HashSet<u64>,
    month: HashSet<u64>,
    day_of_week: HashSet<u64>,
    /// Invariant: two restricted day fields match on either, as crontab(5) runs them.
    either_day: bool,
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

pub(super) fn parse_cron_expression(expression: &str) -> Result<CronFields, String> {
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
        either_day: !parts[2].starts_with('*') && !parts[4].starts_with('*'),
    })
}

pub(super) struct Civil {
    pub(super) minute: u64,
    pub(super) hour: u64,
    day: u64,
    month: u64,
    weekday: u64,
}

pub(super) fn civil_of(ms: u64) -> Civil {
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
    let weekday = fields.day_of_week.contains(&civil.weekday)
        || (civil.weekday == 0 && fields.day_of_week.contains(&7));
    let month_day = fields.day_of_month.contains(&civil.day);
    let day_matches = if fields.either_day {
        weekday || month_day
    } else {
        weekday && month_day
    };
    fields.minute.contains(&civil.minute)
        && fields.hour.contains(&civil.hour)
        && fields.month.contains(&civil.month)
        && day_matches
}

fn next_cron_run_after(expression: &str, after_ms: u64, zone: &Zone) -> Result<u64, String> {
    let fields = parse_cron_expression(expression)?;
    let after_wall = zone.wall_ms(after_ms);
    // Truncate to the minute, then step forward one minute at a time.
    let mut candidate = (after_ms / ONE_MINUTE_MS).saturating_add(1) * ONE_MINUTE_MS;
    let deadline = candidate.saturating_add(366 * 24 * 60 * ONE_MINUTE_MS);
    while candidate <= deadline {
        let wall = zone.wall_ms(candidate);
        // Invariant: the hour a fall-back repeats has wall times already passed; each fires once.
        if wall > after_wall && matches_cron_fields(&civil_of(wall), &fields) {
            return Ok(candidate);
        }
        candidate = candidate.saturating_add(ONE_MINUTE_MS);
    }
    Err(format!(
        "Cron schedule did not match within one year: {expression}"
    ))
}

/// Minimal UTC ISO-8601 parse: `YYYY-MM-DD[THH:MM[:SS]][Z]`. Offsets other
/// than Z are rejected: an `at` is an absolute instant, and only cron fields are local.
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
