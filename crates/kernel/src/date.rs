/// Hinnant civil-from-days, shared with the runtime scheduler (§15.2 cron math).
pub fn civil_from_days(days: u64) -> (u64, u64, u64) {
    // Howard Hinnant's civil-from-days, unsigned since the epoch is 1970.
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let adjusted_year = if month <= 2 { year + 1 } else { year };
    (adjusted_year, month, day)
}
