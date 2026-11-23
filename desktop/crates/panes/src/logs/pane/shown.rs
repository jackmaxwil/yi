//! The lines the list shows: the feed's rows and the model's, put in time order, filtered,
//! cut to a window of the newest, and styled by level. The list engine reads the result.

use std::collections::BTreeSet;
use std::hash::{DefaultHasher, Hash, Hasher};

use yd_core::feed::{FeedEvent, FeedStatus, Table};
use yd_core::ids::Millis;
use yd_core::row::{Row, Rows, Tone, Value};

use super::state::{Filters, Level};

/// Rows the list holds at once: Rune reads every row key again when the count changes, so a
/// log lists a window of its newest rows and loads older ones on request.
pub const WINDOW: usize = 5_000;
/// Rows of the feed looked at for one view, newest first.
pub const SCAN: usize = 50_000;
/// How far from a line "Logs around it" reaches, each way.
pub const AROUND: u64 = 30_000;

/// What the list shows, and the numbers the header and footer say about it.
#[derive(Debug)]
pub struct Shown {
    pub table: Table,
    /// Sources among the rows looked at, any filter.
    pub sources: BTreeSet<String>,
    /// Errors and warnings among the rows looked at, any filter.
    pub errors: usize,
    pub warnings: usize,
    /// Lines there are: the feed's and the model's.
    pub total: usize,
    /// Lines the filters let through, listed or not.
    pub matched: usize,
    /// Lines of those that are older than the window.
    pub older: usize,
    /// Rows of the feed that were looked at.
    pub scanned: usize,
}

fn level_of(row: &Row) -> Option<Level> {
    Level::of(row.words("level")?)
}

fn time_of(row: &Row) -> Option<u64> {
    row.words("ts")?.parse().ok()
}

fn lower(text: &str) -> String {
    text.to_lowercase()
}

/// Whether `row` passes the filters. `needle` is the text filter, lower case.
fn passes(row: &Row, filters: &Filters, needle: &str) -> bool {
    let level = level_of(row).is_none_or(|level| filters.on(level));
    let source = filters
        .source
        .as_deref()
        .is_none_or(|only| row.words("source") == Some(only));
    let near = filters
        .around
        .is_none_or(|at| time_of(row).is_some_and(|ts| ts.abs_diff(at.0) <= AROUND));
    let found = needle.is_empty()
        || ["message", "source"]
            .iter()
            .filter_map(|field| row.words(field))
            .any(|text| lower(text).contains(needle));
    level && source && near && found
}

/// A row as the list draws it: the level as plain words in the level's colour.
fn styled(row: &Row) -> Row {
    let mut row = row.clone();
    let level = level_of(&row);
    if let Some(Value::Badge(word, _)) = row.cells.get("level").cloned() {
        row.cells.insert("level".to_owned(), Value::Text(word));
    }
    row.tone = match level {
        Some(Level::Error) => Some(Tone::Bad),
        Some(Level::Warn) => Some(Tone::Warn),
        Some(Level::Debug) => Some(Tone::Faint),
        Some(Level::Info) | None => None,
    };
    row
}

/// A row that says what the window left out, in place of the lines it cut.
fn cut(said: String) -> Row {
    let mut row = Row::new("cut").text("message", said);
    row.tone = Some(Tone::Faint);
    row
}

fn older_row(older: usize, listed: usize, matched: usize) -> Row {
    cut(format!(
        "[…] {older} older lines are not listed: the newest {listed} of {matched} are shown (WINDOW {WINDOW}); Load older in the footer lists more"
    ))
}

/// The list for `feed` and the model's own `wire` lines under `filters` and the text filter.
pub fn shown(feed: Option<&Table>, wire: &[Row], filters: &Filters, text: &str) -> Shown {
    let needle = lower(text.trim());
    let limit = WINDOW.saturating_mul(filters.older.saturating_add(1));
    let mut out = Shown {
        table: Table::default(),
        sources: BTreeSet::new(),
        errors: 0,
        warnings: 0,
        total: feed.map_or(0, Table::len).saturating_add(wire.len()),
        matched: 0,
        older: 0,
        scanned: 0,
    };
    let newest_first = feed
        .into_iter()
        .flat_map(|table| (0..table.len()).rev().filter_map(|at| table.get(at)))
        .take(SCAN)
        .inspect(|_| out.scanned = out.scanned.saturating_add(1))
        .chain(wire.iter());
    let mut kept: Vec<&Row> = Vec::new();
    let mut seen = BTreeSet::new();
    let (mut errors, mut warnings, mut matched) = (0_usize, 0_usize, 0_usize);
    for row in newest_first {
        match level_of(row) {
            Some(Level::Error) => errors = errors.saturating_add(1),
            Some(Level::Warn) => warnings = warnings.saturating_add(1),
            Some(Level::Info | Level::Debug) | None => {}
        }
        if let Some(source) = row.words("source") {
            seen.insert(source.to_owned());
        }
        if passes(row, filters, &needle) {
            matched = matched.saturating_add(1);
            if kept.len() < limit || row.id.as_str().starts_with("wire-") {
                kept.push(row);
            }
        }
    }
    kept.sort_by_key(|row| std::cmp::Reverse(time_of(row)));
    kept.truncate(limit);
    kept.reverse();
    let older = matched.saturating_sub(kept.len());
    let mut rows: Vec<Row> = kept.into_iter().map(styled).collect();
    if older > 0 {
        rows.insert(0, older_row(older, rows.len(), matched));
    }
    if feed.is_some_and(|table| out.scanned < table.len()) {
        let said = format!(
            "[…] the newest {} of {} feed lines were looked at (SCAN {SCAN}); the rest are not in this list",
            out.scanned,
            feed.map_or(0, Table::len)
        );
        rows.insert(0, cut(said));
    }
    out.table.apply(FeedEvent::Reset(rows));
    match feed.map(Table::status) {
        Some(status @ (FeedStatus::Offline | FeedStatus::Error(_))) => {
            out.table.apply(FeedEvent::Status(status.clone()));
        }
        Some(FeedStatus::Loading) | None if out.table.len() == 0 => {
            out.table.apply(FeedEvent::Status(FeedStatus::Loading));
        }
        Some(FeedStatus::Live | FeedStatus::Empty | FeedStatus::Loading) | None => {}
    }
    out.sources = seen;
    out.errors = errors;
    out.warnings = warnings;
    out.matched = matched;
    out.older = older;
    out
}

/// A number that moves when anything `shown` reads besides the feed's rows does.
pub fn stamp(wire: &[Row], filters: &Filters, text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    (filters, text, wire.len()).hash(&mut hasher);
    wire.iter().for_each(|row| row.id.hash(&mut hasher));
    hasher.finish()
}

/// Most lines folded into a list one by one; more are cheaper to list again.
const FEW: usize = 16;

/// Folds the feed's rows from `from` on into `out`, made from the rows before them under the
/// same filters. False when the list must be made again: too many rows, or a first cut.
pub fn append(out: &mut Shown, feed: &Table, from: usize, filters: &Filters, text: &str) -> bool {
    let (end, needle) = (feed.len(), lower(text.trim()));
    let limit = WINDOW.saturating_mul(filters.older.saturating_add(1));
    if end.saturating_sub(from) > FEW || end > SCAN || out.scanned != from {
        return false;
    }
    for row in (from..end).filter_map(|at| feed.get(at)) {
        match level_of(row) {
            Some(Level::Error) => out.errors = out.errors.saturating_add(1),
            Some(Level::Warn) => out.warnings = out.warnings.saturating_add(1),
            Some(Level::Info | Level::Debug) | None => {}
        }
        out.scanned = out.scanned.saturating_add(1);
        out.total = out.total.saturating_add(1);
        if let Some(source) = row.words("source") {
            out.sources.insert(source.to_owned());
        }
        if passes(row, filters, &needle) {
            out.matched = out.matched.saturating_add(1);
            out.table.apply(FeedEvent::Upsert(styled(row)));
        }
    }
    let cut = usize::from(out.older > 0);
    while out.table.len().saturating_sub(cut) > limit {
        let Some(oldest) = out.table.get(cut).map(|row| row.id.clone()) else {
            break;
        };
        out.table.apply(FeedEvent::Remove(oldest));
        out.older = out.older.saturating_add(1);
    }
    let listed = out.table.len().saturating_sub(cut);
    match (cut, out.older) {
        (0, 0) => true,
        (0, _) => false,
        (_, older) => {
            out.table
                .apply(FeedEvent::Upsert(older_row(older, listed, out.matched)));
            true
        }
    }
}

/// The time a log line carries, for "Logs around it".
pub fn when(row: &Row) -> Option<Millis> {
    time_of(row).map(Millis)
}
