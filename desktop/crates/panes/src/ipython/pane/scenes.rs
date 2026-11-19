//! The pane's scenes: a real server's recorded turn of six kernel cells, whole and mid-run.

use yd_core::wire::replay::play;
use yd_ui::win::Hub;

/// The recorded turn: streams, Markdown, a DataFrame, a chart, 3,000 lines and an error.
const TURN: &str = include_str!("../../../../../fixtures/wire/kernel-cells.jsonl");
/// The recording's lines up to the third cell's start, before its result.
const RUNNING: usize = 87;
/// The recording's lines up to the fourth cell's end, its chart, before the fifth starts.
const PICTURE: usize = 160;

fn shown(hub: &mut Hub, lines: usize) {
    let part: Vec<&str> = TURN.lines().take(lines).collect();
    // A recording that does not decode is a scene bug the tests catch by what is missing.
    let _ = play(&mut hub.model, &part.join("\n"));
}

/// Every cell of the turn, finished.
pub(super) fn cells(hub: &mut Hub) {
    shown(hub, usize::MAX);
}

/// The turn while its third cell runs.
pub(super) fn running(hub: &mut Hub) {
    shown(hub, RUNNING);
}

/// The first four cells, the chart last.
pub(super) fn picture(hub: &mut Hub) {
    shown(hub, PICTURE);
}
