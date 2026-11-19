//! Lines the model already holds: what the server said about a thread and what refused, and
//! what the agent ran and printed. They join the feed's lines in the list.

use yd_core::ids::Millis;
use yd_core::model::Model;
use yd_core::reduce::threads::Thread;
use yd_core::reduce::turns::ToolState;
use yd_core::row::Row;

use crate::chat::messages::line;
use crate::chat::tools::{output, target};

/// Tool calls and refusals looked at, newest last; older ones are not lines of this list.
pub const CALLS: usize = 40;
/// Lines of one call's output listed.
pub const PRINTED: usize = 4;
/// Characters of a message listed.
const SAID: usize = 300;

/// Unix milliseconds as `HH:MM:SS.mmm` in UTC, as the feed's lines write it.
pub(super) fn clock_text(at: Millis) -> String {
    let day = at.0 % 86_400_000;
    let (h, m, s, ms) = (
        day / 3_600_000,
        day / 60_000 % 60,
        day / 1000 % 60,
        day % 1000,
    );
    format!("{h:02}:{m:02}:{s:02}.{ms:03}")
}

/// A line of the list from the model: `kind` and `n` keep its id the same from one view to
/// the next, so an open line stays open.
fn row(kind: &str, n: usize, at: Millis, level: &str, source: &str, said: &str) -> Row {
    let message = line(said, SAID);
    let raw: String = said.chars().take(2 * SAID).collect();
    let row = Row::new(format!("wire-{kind}-{}-{n}", at.0))
        .text("ts", at.0.to_string())
        .text("time", clock_text(at))
        .text("level", level)
        .text("source", source);
    let row = if raw == message {
        row
    } else {
        row.text("json", raw)
    };
    row.text("message", message)
}

/// The model's lines for the thread a window shows.
pub fn lines(model: &Model, thread: Option<&Thread>) -> Vec<Row> {
    let failures = model.wire.failures.iter().enumerate();
    let mut rows: Vec<Row> = failures
        .map(|(n, fail)| row("refused", n, fail.at, "WARN", "yi", &fail.why.to_string()))
        .collect();
    let Some(thread) = thread else {
        return rows;
    };
    let name = thread.title.as_deref().unwrap_or(thread.id.as_str());
    let with_thread = |row: Row| row.text("thread", line(name, 60));
    let notes = thread.notes.iter().enumerate();
    rows.extend(
        notes.map(|(n, note)| with_thread(row("note", n, note.at, "INFO", "yi", &note.text))),
    );
    let mut calls: Vec<_> = thread
        .transcript
        .tools
        .values()
        .filter_map(|call| Some((call.started?, call)))
        .collect();
    calls.sort_by_key(|(started, _)| *started);
    let skip = calls.len().saturating_sub(CALLS);
    if let Some((first, _)) = calls.get(skip).filter(|_| skip > 0) {
        let said = format!("[…] {skip} older tool calls are not listed (CALLS {CALLS})");
        rows.push(row("cut", 0, *first, "INFO", "agent", &said));
    }
    for (n, (started, call)) in calls.into_iter().enumerate().skip(skip) {
        let level = if call.state == ToolState::Failed {
            "WARN"
        } else {
            "INFO"
        };
        let said = format!("{} {}", call.name, target(call).unwrap_or_default());
        // The call's step in the Trace pane, by the id the trace gives it.
        let span = format!("tool-{}", call.id.as_str().replace('/', ":"));
        let stepped = |row: Row| with_thread(row).text("span", span.as_str());
        rows.push(stepped(row("call", n, started, level, "agent", &said)));
        let printed = output(call);
        let printed: Vec<&str> = printed.lines().filter(|l| !l.trim().is_empty()).collect();
        let at = call.ended.unwrap_or(started);
        let more = format!(
            "[…] {} more output lines of {} are not listed (PRINTED {PRINTED})",
            printed.len().saturating_sub(PRINTED),
            call.name
        );
        let shown = printed.iter().take(PRINTED).copied();
        let more = (printed.len() > PRINTED).then_some(more.as_str());
        for (k, said) in shown.chain(more).enumerate() {
            let id = n.saturating_mul(PRINTED + 1).saturating_add(k);
            rows.push(stepped(row("out", id, at, "DEBUG", "agent", said)));
        }
    }
    rows
}
