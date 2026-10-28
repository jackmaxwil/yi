//! A chat pane scrolled above the bottom holds its rows still while the turn writes below.

use std::error::Error;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use yi_tui::app::{App, TuiOptions};
use yi_tui::cell::{Cell, ToolCell, ToolStatus};
use yi_tui::colors::{ColorTier, Theme};
use yi_types::event::{AgentEvent, AssistantMessageEvent};
use yi_types::message::{AgentMessage, StopReason};

use crate::common;
use yi_types::config::FollowMode;

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
    paint_in(app, AREA, scroll)
}

fn paint_in(
    app: &mut App,
    area: Rect,
    scroll: &mut usize,
) -> (Vec<String>, Option<(usize, usize)>) {
    let mut buffer = Buffer::empty(Rect::new(0, 0, area.right(), area.bottom()));
    let thumb = yi_tui::render::paint_pane(app, None, &mut buffer, area, scroll);
    let rows = (area.top()..area.bottom())
        .map(|y| {
            (area.left()..area.right())
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
    moves_while_streaming_mode(thinking, text, hold, FollowMode::Slide)
}

fn moves_while_streaming_mode(
    thinking: &str,
    text: &str,
    hold: usize,
    follow: FollowMode,
) -> Vec<String> {
    let mut app = seeded();
    app.set_follow(follow);
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

#[test]
fn every_follow_mode_preserves_a_held_view() {
    for mode in [FollowMode::Slide, FollowMode::Page, FollowMode::Still] {
        let text = (0..20)
            .map(|i| format!("paragraph {i}\n\n"))
            .collect::<String>();
        let moves = moves_while_streaming_mode("", &text, 12, mode);
        assert!(moves.is_empty(), "{mode:?}: {moves:#?}");
    }
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

/// Incident: the thumb sat at the top whatever the position. A held view sits away from the
/// top, and one scrolled further up sits higher.
#[test]
fn the_thumb_places_the_view_in_the_whole_transcript() -> TestResult {
    let mut app = seeded();
    let mut scroll = 10;
    let (_, near) = paint(&mut app, &mut scroll);
    let (total, top) = near.ok_or("no thumb on a held view")?;
    assert!(top > 0 && total > usize::from(AREA.height), "{total} {top}");
    scroll = 40;
    let (_, higher) = paint(&mut app, &mut scroll);
    let (_, higher) = higher.ok_or("no thumb higher up")?;
    assert!(higher < top, "{higher} {top}");
    Ok(())
}

/// A re-wrap changes every row count above the view; read as growth below it, a narrowed
/// pane threw a view held five rows up two hundred rows away.
#[test]
fn a_held_view_keeps_its_offset_across_a_resize() -> TestResult {
    let mut app = seeded();
    for i in 0..10 {
        app.commit_cell(&Cell::Assistant {
            markdown: format!(
                "{} {i}\n",
                "a long line that wraps again when narrowed".repeat(3)
            ),
        });
    }
    let mut scroll = 5;
    let _ = paint(&mut app, &mut scroll);
    let _ = paint_in(&mut app, Rect::new(0, 0, 40, 20), &mut scroll);
    assert_eq!(scroll, 5);
    Ok(())
}

/// A drag copies the source of the cell each painted row names; a row named for a neighbour
/// pasted the neighbour's text, at the bottom and after a scroll alike.
#[test]
fn each_painted_row_names_the_cell_it_was_drawn_from() -> TestResult {
    let mut app = seeded();
    for start in [0, 12, 0] {
        let mut scroll = start;
        let (rows, _) = paint(&mut app, &mut scroll);
        let mut owned = 0;
        for (y, row) in (0u16..).zip(&rows) {
            let words = row
                .trim()
                .trim_start_matches(|c: char| !c.is_alphanumeric())
                .trim();
            if let Some(Some((index, Some(source)))) = app.pane_row(y)
                && !words.is_empty()
            {
                assert!(
                    source.contains(words),
                    "scroll {start} row {y} cell {index}: {words:?} not in {source:?}"
                );
                owned += 1;
            }
        }
        assert!(owned > 5, "scroll {start}: {owned} rows named a cell");
    }
    Ok(())
}

/// Rows the column moved up between two frames, read off the first written row of `now`.
fn shift(before: &[String], now: &[String]) -> Option<usize> {
    let row = now.iter().position(|row| !row.trim().is_empty())?;
    let was = before.iter().position(|old| *old == now[row])?;
    was.checked_sub(row)
}

/// Commits a `lines`-row fence at the bottom of a full pane and paints until its last line is
/// on screen; returns each frame's move and whether every lagging frame asked for the next.
fn roll_in(lines: usize) -> Result<(Vec<usize>, bool), Box<dyn Error>> {
    roll_in_sized(lines, AREA.height)
}

fn roll_in_sized(lines: usize, height: u16) -> Result<(Vec<usize>, bool), Box<dyn Error>> {
    roll_in_mode(lines, height, FollowMode::Slide)
}

fn roll_in_mode(
    lines: usize,
    height: u16,
    follow: FollowMode,
) -> Result<(Vec<usize>, bool), Box<dyn Error>> {
    let area = Rect::new(0, 0, AREA.width, height);
    let mut app = seeded();
    app.set_follow(follow);
    app.set_clock(yi_tui::motion::Clock::Fixed(std::time::Instant::now()));
    let mut scroll = 0;
    let (mut before, _) = paint_in(&mut app, area, &mut scroll);
    let code: Vec<String> = (0..lines).map(|i| format!("let x{i:03} = {i};")).collect();
    app.commit_cell(&Cell::Assistant {
        markdown: format!("```rust\n{}\n```\n", code.join("\n")),
    });
    let last = format!("let x{:03}", lines - 1);
    let (mut moves, mut asked, mut phase) = (Vec::new(), true, 0);
    for _ in 0..lines.saturating_mul(2) {
        let _ = app.take_redraw(&mut phase);
        let (now, _) = paint_in(&mut app, area, &mut scroll);
        moves.push(
            shift(&before, &now).ok_or(
                "a frame shares no row with the one before: the column jumped past a screen",
            )?,
        );
        if now.iter().any(|row| row.contains(&last)) {
            return Ok((moves, asked));
        }
        asked &= app.take_redraw(&mut phase);
        before = now;
    }
    Err(format!("`{last}` never reached the screen: {moves:?}").into())
}

#[test]
fn slide_keeps_the_pre_change_frame_sequence() -> TestResult {
    let (moves, _) = roll_in_sized(20, 30)?;
    assert_eq!(moves, vec![1; 22]);
    Ok(())
}

#[test]
fn page_moves_only_in_half_page_jumps() -> TestResult {
    let (moves, _) = roll_in_mode(20, 30, FollowMode::Page)?;
    assert_eq!(moves, vec![15, 15]);
    Ok(())
}

#[test]
fn page_moves_a_ten_row_pane_five_rows_at_a_time() -> TestResult {
    let (moves, _) = roll_in_mode(20, 10, FollowMode::Page)?;
    assert!(moves.iter().all(|rows| matches!(rows, 0 | 5)), "{moves:?}");
    assert!(moves.contains(&5), "no page jump: {moves:?}");
    Ok(())
}

#[test]
fn still_holds_the_view_and_marks_hidden_rows_at_a_nonzero_offset() -> TestResult {
    let area = Rect::new(7, 5, AREA.width, 30);
    let mut app = seeded();
    app.set_follow(FollowMode::Still);
    app.set_clock(yi_tui::motion::Clock::Fixed(std::time::Instant::now()));
    let mut scroll = 0;
    let (mut before, _) = paint_in(&mut app, area, &mut scroll);
    let code: Vec<String> = (0..20).map(|i| format!("let x{i:03} = {i};")).collect();
    app.commit_cell(&Cell::Assistant {
        markdown: format!("```rust\n{}\n```\n", code.join("\n")),
    });
    let mut moves = Vec::new();
    let mut after = before.clone();
    for _ in 0..5 {
        let (now, _) = paint_in(&mut app, area, &mut scroll);
        moves.push(shift(&before, &now));
        before = now.clone();
        after = now;
    }
    assert_eq!(moves, vec![Some(0); 5], "{after:#?}");
    assert!(
        after.iter().any(|row| row.contains("more rows · PgDn")),
        "{after:#?}"
    );
    assert!(
        !after.iter().any(|row| row.contains("let x019")),
        "{after:#?}"
    );
    assert_eq!(scroll, 0);
    Ok(())
}

#[test]
fn still_marker_lands_on_vt100_at_a_nonzero_offset() -> TestResult {
    let backend = common::VT100Backend::with_scrollback(80, 40, 200);
    let mut terminal = yi_tui::terminal::Terminal::new(backend, 3)?;
    terminal.set_viewport_area(Rect::new(0, 0, 80, 40));
    let mut app = seeded();
    app.set_follow(FollowMode::Still);
    let area = Rect::new(7, 5, AREA.width, 30);
    let mut scroll = 0;
    terminal.draw(|frame| {
        let _ = yi_tui::render::paint_pane(&mut app, None, frame.buffer_mut(), area, &mut scroll);
    })?;
    let code: Vec<String> = (0..20).map(|i| format!("let x{i:03} = {i};")).collect();
    app.commit_cell(&Cell::Assistant {
        markdown: format!("```rust\n{}\n```\n", code.join("\n")),
    });
    terminal.draw(|frame| {
        let _ = yi_tui::render::paint_pane(&mut app, None, frame.buffer_mut(), area, &mut scroll);
    })?;
    let rows: Vec<String> = (0..40)
        .map(|row| terminal.backend().row_text(row))
        .collect();
    assert!(
        rows.iter().any(|row| row.contains("more rows · PgDn")),
        "{rows:#?}"
    );
    Ok(())
}

#[test]
fn still_page_down_reveals_rows_and_a_prompt_returns_to_bottom() -> TestResult {
    let mut app = seeded();
    app.set_follow(FollowMode::Still);
    let mut scroll = 0;
    let _ = paint(&mut app, &mut scroll);
    let code: Vec<String> = (0..20).map(|i| format!("let x{i:03} = {i};")).collect();
    app.commit_cell(&Cell::Assistant {
        markdown: format!("```rust\n{}\n```\n", code.join("\n")),
    });
    let (held, _) = paint(&mut app, &mut scroll);
    assert!(!held.iter().any(|row| row.contains("let x")), "{held:#?}");
    app.advance_follow(20);
    let (paged, _) = paint(&mut app, &mut scroll);
    assert!(paged.iter().any(|row| row.contains("let x")), "{paged:#?}");
    app.follow_bottom();
    let (bottom, _) = paint(&mut app, &mut scroll);
    assert!(
        bottom.iter().any(|row| row.contains("let x019")),
        "{bottom:#?}"
    );
    assert!(
        !bottom.iter().any(|row| row.contains("more rows")),
        "{bottom:#?}"
    );
    Ok(())
}

/// Dies with a block that landed at once moving the column by its height in one frame; the
/// measured console reply jumped two or three rows at every paragraph and fence.
#[test]
fn a_block_that_lands_at_once_rolls_in_a_row_a_frame() -> TestResult {
    let (moves, asked) = roll_in(20)?;
    assert!(moves.iter().all(|&rows| rows <= 1), "{moves:?}");
    assert!(
        moves.len() >= 20,
        "the block rolled in, not jumped: {moves:?}"
    );
    assert!(asked, "a pane behind its bottom asks for the next frame");
    Ok(())
}

/// Dies with a long block crawling a row a frame for seconds, or landing in one jump.
#[test]
fn a_long_block_catches_up_without_jumping() -> TestResult {
    let (moves, _) = roll_in(200)?;
    assert!(
        moves.len() <= 70,
        "200 rows took {} frames: {moves:?}",
        moves.len()
    );
    assert!(moves.iter().all(|&rows| rows <= 9), "{moves:?}");
    Ok(())
}

/// Dies with the turn's first prompt landing whole in an empty pane: with no transcript cell to
/// pin the window to yet, its rows and the floor under them jumped together.
#[test]
fn a_first_prompt_rolls_into_an_empty_pane() -> TestResult {
    let mut app = app();
    let mut scroll = 0;
    app.reduce_agent(AgentEvent::AgentStart);
    let status = |rows: &[String]| rows.iter().position(|row| row.contains("faux-1"));
    let (empty, _) = paint(&mut app, &mut scroll);
    let composer = status(&empty).ok_or("no status row")?;
    let text: Vec<String> = (0..6).map(|i| format!("prompt line {i}")).collect();
    app.commit_cell(&Cell::User {
        text: text.join("\n"),
    });
    let mut moves = Vec::new();
    let mut shown = 0;
    for _ in 0..20 {
        let (rows, _) = paint(&mut app, &mut scroll);
        assert_eq!(status(&rows), Some(composer), "the composer moved");
        let now = rows
            .iter()
            .filter(|row| row.contains("prompt line"))
            .count();
        moves.push(now.saturating_sub(shown));
        shown = now;
    }
    assert!(moves.iter().all(|&rows| rows <= 1), "{moves:?}");
    assert!(
        moves.iter().sum::<usize>() >= 5,
        "the prompt never rolled in: {moves:?}"
    );
    Ok(())
}

/// Dies with the first prompt rolling in again after a scroll away and back: the scroll dropped
/// the follow pin, and the empty-pane fallback re-armed for a cell already on screen.
#[test]
fn a_first_prompt_scrolled_away_and_back_stays_where_it_was() -> TestResult {
    let mut app = app();
    let mut scroll = 0;
    app.reduce_agent(AgentEvent::AgentStart);
    paint(&mut app, &mut scroll);
    let text: Vec<String> = (0..6).map(|i| format!("prompt line {i}")).collect();
    app.commit_cell(&Cell::User {
        text: text.join("\n"),
    });
    for _ in 0..20 {
        paint(&mut app, &mut scroll);
    }
    let (settled, _) = paint(&mut app, &mut scroll);
    scroll = 3;
    paint(&mut app, &mut scroll);
    scroll = 0;
    let (back, _) = paint(&mut app, &mut scroll);
    assert_eq!(back, settled, "the prompt rolled in again");
    Ok(())
}

#[test]
fn composer_bottom_uses_the_panes_bottom_at_a_nonzero_offset() -> TestResult {
    let area = Rect::new(7, 5, 60, 24);
    let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 40));
    let mut app = app();
    let _ = yi_tui::render::paint_pane(&mut app, None, &mut buffer, area, &mut 0);
    let y = area.bottom().saturating_sub(1);
    let row: String = (0..buffer.area.width)
        .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol().to_owned()))
        .collect();
    assert!(
        row.contains('╰') && row.contains('╯'),
        "composer bottom is not on row {y}: {row:?}"
    );
    Ok(())
}
