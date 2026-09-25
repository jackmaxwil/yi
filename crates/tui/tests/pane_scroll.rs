//! A chat pane scrolled above the bottom holds its rows still while the turn writes below.

use std::error::Error;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use yi_tui::app::{App, TuiOptions};
use yi_tui::cell::{Cell, ToolCell, ToolStatus};
use yi_tui::colors::{ColorTier, Theme};
use yi_types::event::{AgentEvent, AssistantMessageEvent};
use yi_types::message::{AgentMessage, StopReason};

mod common;

type TestResult = Result<(), Box<dyn Error>>;

const AREA: Rect = Rect::new(0, 0, 60, 20);

fn app() -> App {
    App::new(
        TuiOptions {
            model: common::test_model("faux-1"),
            session_name: "s".to_owned(),
            cwd: "/tmp".to_owned(),
            lane: None,
            context_window: 128_000,
            session_dir: String::new(),
            keys: Vec::new(),
            initial_prompt: None,
            pace: 0,
        },
        Theme::new(ColorTier::TrueColor, true),
        yi_tui::keymap::default_keymap(),
        80,
    )
}

fn tool(id: &str, rows: usize) -> Cell {
    Cell::Tool(ToolCell {
        name: "bash".to_owned(),
        call_id: id.to_owned(),
        intent: None,
        status: ToolStatus::Done,
        summary: ToolCell::summary_of("bash", "ls"),
        digest: None,
        preview: (0..rows).map(|i| format!("out {id} {i}")).collect(),
        elapsed_ms: 0,
        calls: 1,
        details: serde_json::Value::Null,
    })
}

fn paint(app: &mut App, scroll: &mut usize) -> (Vec<String>, Option<(usize, usize)>) {
    let mut buffer = Buffer::empty(AREA);
    let thumb = yi_tui::render::paint_pane(app, None, &mut buffer, AREA, scroll);
    let rows = (0..AREA.height)
        .map(|y| {
            (0..AREA.width)
                .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol().to_owned()))
                .collect::<String>()
        })
        .collect();
    (rows, thumb)
}

/// Thirty turns of question and two-paragraph answer, painted once at the bottom.
fn seeded() -> App {
    let mut app = app();
    for i in 0..30 {
        app.commit_cell(&Cell::User {
            text: format!("question {i}"),
        });
        app.commit_cell(&Cell::Assistant {
            markdown: format!("answer {i}\n\nsecond para {i}\n"),
        });
    }
    let mut scroll = 0;
    let _ = paint(&mut app, &mut scroll);
    app
}

fn message(thinking: &str, text: &str) -> AgentMessage {
    let mut content = Vec::new();
    if !thinking.is_empty() {
        content.push(yi_runtime::faux::faux_thinking(thinking));
    }
    if !text.is_empty() {
        content.push(yi_runtime::faux::faux_text(text));
    }
    yi_runtime::faux::faux_assistant_message(content, StopReason::Stop)
}

/// Streams `thinking` then `text` seven bytes at a time under a view held `hold` rows up,
/// and names every frame whose top row moved.
fn moves_while_streaming(thinking: &str, text: &str, hold: usize) -> Vec<String> {
    let mut app = seeded();
    app.commit_cell(&Cell::User {
        text: "go".to_owned(),
    });
    app.reduce_agent(AgentEvent::AgentStart);
    let mut scroll = 0;
    let _ = paint(&mut app, &mut scroll);
    scroll = hold;
    let (before, _) = paint(&mut app, &mut scroll);
    let anchor = before.first().cloned().unwrap_or_default();
    let both = format!("{thinking}{text}");
    let mut moves = Vec::new();
    let mut end = 5;
    loop {
        let at = end.min(both.len());
        let (seen_thought, seen_text) = if at <= thinking.len() {
            (both.get(..at).unwrap_or_default(), "")
        } else {
            (thinking, both.get(thinking.len()..at).unwrap_or_default())
        };
        app.reduce_agent(AgentEvent::MessageUpdate {
            assistant_message_event: AssistantMessageEvent::Done {
                reason: StopReason::Stop,
                message: message(seen_thought, seen_text),
            },
        });
        let (after, _) = paint(&mut app, &mut scroll);
        if after.first() != Some(&anchor) {
            moves.push(format!("byte {at}: top {:?}", after.first()));
        }
        if at == both.len() {
            break;
        }
        end += 7;
    }
    app.reduce_agent(AgentEvent::MessageEnd {
        message: message(thinking, text),
    });
    let (after, _) = paint(&mut app, &mut scroll);
    if after.first() != Some(&anchor) {
        moves.push(format!("end: top {:?}", after.first()));
    }
    moves
}

/// Incident: a multi-row card committed one more row than the transcript grew, so each
/// finished call pushed a held view up a row.
#[test]
fn a_held_view_stays_put_as_calls_commit_below_it() -> TestResult {
    let mut app = seeded();
    let mut scroll = 12;
    let (before, _) = paint(&mut app, &mut scroll);
    for i in 0..6 {
        app.commit_cell(&tool(&format!("t{i}"), 4));
        let (after, _) = paint(&mut app, &mut scroll);
        assert_eq!(before.first(), after.first(), "moved after call {i}");
    }
    Ok(())
}

/// Incident: a thought committed paragraph by paragraph renders a blank between them only
/// once merged, so the held view drifted a row a paragraph.
#[test]
fn a_held_view_stays_put_while_reasoning_streams() -> TestResult {
    let thought: String = (0..8)
        .map(|i| format!("Thinking step {i} goes on for a while here.\n\n"))
        .collect();
    let moves = moves_while_streaming(&thought, "Answer.\n", 12);
    assert!(moves.is_empty(), "{moves:#?}");
    Ok(())
}

#[test]
fn a_held_view_stays_put_while_rich_prose_streams() -> TestResult {
    let mut text = String::from("# Title\n\nIntro para.\n\n```rust\n");
    for i in 0..15 {
        text.push_str(&format!("let x{i} = {i};\n"));
    }
    text.push_str("```\n\n| a | b |\n| --- | --- |\n");
    for i in 0..15 {
        text.push_str(&format!("| r{i} | v{i} |\n"));
    }
    text.push_str("\n## Next\n\n1. one\n2. two\n\n> quote here\n\nDone.\n");
    let moves = moves_while_streaming("", &text, 12);
    assert!(moves.is_empty(), "{moves:#?}");
    Ok(())
}

/// The working row arriving under the transcript is floor, not content: it must not move
/// what a held reader sees.
#[test]
fn a_held_view_stays_put_when_the_floor_grows() -> TestResult {
    let mut app = seeded();
    app.commit_cell(&Cell::User {
        text: "go".to_owned(),
    });
    let mut scroll = 12;
    let (before, _) = paint(&mut app, &mut scroll);
    app.reduce_agent(AgentEvent::AgentStart);
    let (after, _) = paint(&mut app, &mut scroll);
    assert_eq!(before.first(), after.first(), "scroll {scroll}");
    Ok(())
}

/// The thumb reads the whole transcript: shown at the bottom, and away from the top when
/// held just above it.
#[test]
fn the_thumb_places_the_view_in_the_whole_transcript() -> TestResult {
    let mut app = seeded();
    let mut scroll = 0;
    let (_, bottom) = paint(&mut app, &mut scroll);
    let (total, top) = bottom.ok_or("no thumb at the bottom of a long transcript")?;
    assert!(top > 0 && total > usize::from(AREA.height), "{total} {top}");
    scroll = 10;
    let (_, held) = paint(&mut app, &mut scroll);
    assert_eq!(held.map(|(_, top)| top), Some(top.saturating_sub(10)));
    Ok(())
}
