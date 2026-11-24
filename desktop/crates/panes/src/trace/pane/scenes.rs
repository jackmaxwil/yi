//! The pane's scenes: the mockup's turn from its rows, and a real recording derived live.

use yd_core::feed::FeedEvent;
use yd_core::ids::{Millis, RowId};
use yd_core::msg::{Msg, step};
use yd_core::row::{Row, Tone, Value, rows_from_json};
use yd_core::trace::Trace;
use yd_core::wire::replay::play_line;
use yd_ui::list::state::Lists;
use yd_ui::pane::SCENE_PANE;
use yd_ui::win::Hub;

use super::FEED;

const ROWS: &str = include_str!("../../../../../fixtures/mockup/trace.rows.json");
/// A real server's turn that asks to run a command: the recording stopped at the ask.
const ASKED: &str = include_str!("../../../../../fixtures/wire/approval-allow.jsonl");
/// The recording has no times: line `n` happens this long after the first, in milliseconds.
const BEGAN: u64 = 1_794_679_000_000;
const SPACING: u64 = 100;
/// The recording's lines up to and including the server's `requires_action` state.
const LINES: usize = 29;
/// How long the ask has waited by the time the scene is drawn.
const WAITED: u64 = 42_000;

/// The record's first open row: the step that waits on you.
fn waiting(rows: &[Row]) -> Option<RowId> {
    let bar = |row: &&Row| {
        matches!(
            row.get("timeline"),
            Some(Value::Bar {
                tone: Tone::Need,
                ..
            })
        )
    };
    rows.iter().find(bar).map(|row| row.id.clone())
}

fn open(hub: &mut Hub, row: Option<RowId>) {
    if let Some(lists) = hub.panes.get_mut::<Lists>(Some(SCENE_PANE)) {
        lists.list_mut("").set_open(row);
    }
}

/// The mockup's eleven rows through the feed, with the waiting step open.
pub(super) fn mockup(hub: &mut Hub) {
    // A fixture that does not decode is a scene bug the tests catch by what is missing.
    let rows = rows_from_json(ROWS).unwrap_or_default();
    let row = waiting(&rows);
    step(&mut hub.model, Msg::Feed(FEED, FeedEvent::Reset(rows)));
    open(hub, row);
}

/// The recorded turn as the pane derives it from the thread, while its ask still waits.
pub(super) fn live(hub: &mut Hub) {
    for (at, line) in (0_u64..).zip(ASKED.lines().take(LINES)) {
        let now = Millis(BEGAN.saturating_add(at.saturating_mul(SPACING)));
        step(&mut hub.model, Msg::Tick(now));
        let _ = play_line(&mut hub.model, line);
    }
    let last = BEGAN.saturating_add(LINES as u64 * SPACING);
    step(
        &mut hub.model,
        Msg::Tick(Millis(last.saturating_add(WAITED))),
    );
    let thread = yd_ui::shown::of_pane(hub, SCENE_PANE);
    let rows = thread.and_then(|thread| Trace::of(thread, None, hub.model.now));
    let row = rows.and_then(|trace| waiting(&trace.rows()));
    open(hub, row);
}
