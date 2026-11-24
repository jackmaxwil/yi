//! The report pane's scenes: frame 1's page, and a thread that wrote none.

use yd_core::feed::{FeedEvent, FeedStatus};
use yd_core::ids::Millis;
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::wire::replay::{Sync, play_line};
use yd_ui::pane::{SCENE_PANE, Scene, Stage};
use yd_ui::win::Hub;

use super::sample::count;
use super::{DESIGN, FEED, Sample, State, zoom};
use crate::web::pane::scenes::{framed, strings};

const TEXT: &str = include_str!("../../../../../fixtures/mockup/panes/report.text.json");
/// The page the agent wrote, as the mockup draws it.
const PAGE: &str = include_str!("../upload-limit-simulated.html");
const THREAD: &str = "report-thread";
const START: u64 = 1_794_679_000_000;
/// Frame 1's pane is 286 points wide, which draws the 600 point page at 48%: the mockup's
/// own `42%` is waived.
const ROOM: (f32, f32) = (286.0, 260.0);

/// A real turn that wrote no file.
const TURN: &str = include_str!("../../../../../fixtures/wire/turn.jsonl");

pub const SCENES: &[Scene] = &[
    Scene {
        name: "report/empty",
        stage: Stage::Pane(PaneKind::Report),
        setup: |hub| {
            let _ = yd_core::wire::replay::play(&mut hub.model, TURN);
        },
    },
    Scene {
        name: "report/mockup",
        stage: Stage::View(|win, hub, cx| framed(win, hub, cx, PaneKind::Report, ROOM)),
        setup: mockup,
    },
];

/// A JSON string literal of `text`.
fn quote(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One event of `thread`, as the server sends it.
fn event_of(thread: &str, seq: u64, event: &str) -> String {
    format!(
        r#"{{"dir":"recv","frame":{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"{thread}","update":{{"sessionUpdate":"_yi/event","seq":{seq},"event":{event}}}}}}}}}"#
    )
}

fn event(seq: u64, event: &str) -> String {
    event_of(THREAD, seq, event)
}

/// The step that writes the report, in `thread` from its event `seq` on.
fn wrote(model: &mut yd_core::model::Model, thread: &str, seq: u64) {
    let args = format!(
        r#"{{"path":"reports/upload-limit-simulated.html","content":{}}}"#,
        quote(PAGE)
    );
    let start = format!(
        r#"{{"type":"tool_execution_start","toolCallId":"w1","toolName":"write","args":{args}}}"#
    );
    let _ = play_line(model, &event_of(thread, seq, &start));
    let end = r#"{"type":"tool_execution_end","toolCallId":"w1","toolName":"write","result":{"content":[],"details":{}},"isError":false}"#;
    let _ = play_line(model, &event_of(thread, seq.saturating_add(1), end));
}

fn prompt(seq: u64, words: &str) -> [String; 2] {
    let message = format!(
        r#"{{"role":"user","content":{},"timestamp":0,"attribution":"user"}}"#,
        quote(words)
    );
    [
        event(
            seq,
            &format!(r#"{{"type":"message_start","message":{message}}}"#),
        ),
        event(
            seq + 1,
            &format!(r#"{{"type":"message_end","message":{message}}}"#),
        ),
    ]
}

/// Two turns as the server sends them; the second writes the report.
fn mockup(hub: &mut Hub) {
    let model = &mut hub.model;
    let at = |model: &mut yd_core::model::Model, ms: u64| {
        step(model, Msg::Tick(Millis(START + ms)));
    };
    if model.wire.threads.is_empty() {
        let opened = [
            r#"{"dir":"send","frame":{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/Users/me/atlas-api","mcpServers":[]}}}"#.to_owned(),
            format!(r#"{{"dir":"recv","frame":{{"jsonrpc":"2.0","id":2,"result":{{"sessionId":"{THREAD}"}}}}}}"#),
        ];
        at(model, 0);
        for line in opened
            .iter()
            .chain(&prompt(0, "Rate-limit the upload endpoint"))
        {
            let _ = play_line(model, line);
        }
        at(model, 60_000);
        for line in &prompt(2, "Simulate one user hitting the limit and show me") {
            let _ = play_line(model, line);
        }
        at(model, 70_000);
        wrote(model, THREAD, 4);
    } else if let Some(shown) = model.wire.listed().first() {
        // Another scene's thread is the one the window shows: the same step, in its stream.
        let next = match shown.sync {
            Sync::Live(last) => last.map_or(Some(0), |last| last.checked_add(1)),
            Sync::Apart | Sync::Resuming | Sync::Broken => None,
        };
        let id = shown.id.to_string();
        if let Some(next) = next {
            wrote(model, &id, next);
        }
    }
    step(model, Msg::Feed(FEED, FeedEvent::Status(FeedStatus::Live)));
    let s = strings(TEXT);
    let line = |i: usize| s.get(i).cloned().unwrap_or_default();
    let Some(state) = hub.panes.get_mut::<State>(Some(SCENE_PANE)) else {
        return;
    };
    let (legend, axis) = ([line(10), line(11)], [line(7), line(8), line(9)]);
    state.sample = Some(Sample {
        heading: line(5),
        paragraph: line(6),
        passed: count(&legend[0]),
        refused: count(&legend[1]),
        axis,
        legend,
    });
    let room = (ROOM.0, ROOM.1 - 42.0);
    state.room = Some(room);
    let high = (room.1 / zoom(room.0)).round() as u32;
    let _ = state.screen.push(
        DESIGN as u32,
        high,
        vec![255; DESIGN as usize * high as usize * 4],
    );
}
