//! A streamed answer renders past its settled prefix only, and the rows it builds that way
//! are the rows a whole render of the same text gives.

use std::error::Error;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use yi_tui::app::{App, TuiOptions};
use yi_tui::cell::Cell;
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::markdown::{Settled, StableScan, render, stable_stream, work};
use yi_types::event::{AgentEvent, AssistantMessageEvent};
use yi_types::message::StopReason;

mod common;

type TestResult = Result<(), Box<dyn Error>>;

const TRICKY: &[&str] = &[
    "# Title",
    "",
    "Para one",
    "continues here.",
    "",
    "Setext heading",
    "--------------",
    "",
    "- tight a",
    "- tight b",
    "  - nested",
    "1. one",
    "2. two",
    "",
    "3. loose three",
    "",
    "> quote line",
    "lazy line",
    "",
    "    indented code",
    "",
    "~~~python",
    "def f():",
    "    return \"s\"",
    "~~~",
    "",
    "| a | b |",
    "|---|---|",
    "| 1 | `2` |",
    "after table",
    "",
    "***",
    "",
    "<div>",
    "html block",
    "</div>",
    "",
    "see [link](http://x.y), `code`, **bold**",
    "hard\\",
    "break",
    "",
    "```rust",
    "fn main() {",
    "    let s = \"a string",
    "spanning lines\";",
    "}",
    "```",
    "- item with a fence",
    "  ```ts",
    "  const x = 1;",
    "  ```",
    "- after",
    "right before a fence",
    "```",
    "bare",
    "```",
    "#",
    "#x",
    "para",
    "Term",
    "=",
    "",
    "````md",
    "```",
    "nested",
    "```",
    "````",
    "```",
    "    ```",
    "indented close is code",
    "```",
    "tail words",
];

fn theme() -> Theme {
    Theme::new(ColorTier::TrueColor, true)
}

fn answer(sections: usize) -> String {
    let mut text = String::new();
    for i in 0..sections {
        text.push_str(&format!(
            "## Section {i}\n\nThe pane paints `paint_pane` from crates/tui/src/render.rs, and a \
             paragraph long enough to wrap keeps **bold** and _emphasis_ on each row.\n\n\
             - one\n- two with src/lib.rs:4\n  - nested\n\n```rust\n"
        ));
        for j in 0..12 {
            text.push_str(&format!(
                "fn f{j}(x: &str) -> Vec<String> {{ vec![x.to_owned()] }} // {j}\n"
            ));
        }
        text.push_str("```\n\n| k | v |\n|---|---:|\n| a | 1 |\n\n```python\n");
        for j in 0..6 {
            text.push_str(&format!(
                "def g{j}(v):\n    return {{\"n\": len(v)}}  # {j}\n"
            ));
        }
        text.push_str("```\n\n");
    }
    text
}

/// Every prefix: the rows settled so far plus the rest's rows are the whole render's.
fn assert_settles(source: &str, step: usize, width: usize) -> TestResult {
    let theme = theme();
    let mut settled = Settled::default();
    let mut rows: Vec<Line<'static>> = Vec::new();
    let mut end = 0;
    while end < source.len() {
        end = (end + step).min(source.len());
        while !source.is_char_boundary(end) {
            end += 1;
        }
        let prefix = source.get(..end).ok_or("prefix")?;
        let (fresh, tail) = settled
            .advance(prefix, width, &theme)
            .ok_or("went opaque")?;
        rows.extend(fresh);
        let mut shown = rows.clone();
        shown.extend(tail);
        assert_eq!(
            shown,
            render(prefix, width, &theme),
            "at byte {end} of {prefix:?}"
        );
    }
    Ok(())
}

#[test]
fn settled_rows_are_the_whole_render_at_every_prefix() -> TestResult {
    let tricky = TRICKY.join("\n");
    for width in [24, 80] {
        for step in [1, 16] {
            assert_settles(&tricky, step, width)?;
        }
        assert_settles(&answer(2), 16, width)?;
    }
    Ok(())
}

#[test]
fn a_link_definition_makes_the_rows_global() {
    let theme = theme();
    let mut settled = Settled::default();
    assert!(settled.advance("see [x]\n\nmore\n\n", 40, &theme).is_some());
    assert!(
        settled
            .advance("see [x]\n\nmore\n\n[x]: http://a\n", 40, &theme)
            .is_none()
    );
}

#[test]
fn a_resumed_scan_cuts_where_a_whole_scan_does() -> TestResult {
    let tricky = TRICKY.join("\n");
    let mut scan = StableScan::default();
    for end in 0..=tricky.len() {
        let Some(prefix) = tricky.get(..end) else {
            continue;
        };
        let (resumed, whole) = (scan.scan(prefix), stable_stream(prefix));
        assert_eq!(
            (resumed.cut, resumed.reopen),
            (whole.cut, whole.reopen),
            "at {end}"
        );
    }
    Ok(())
}

fn app() -> App {
    let mut app = App::new(
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
        theme(),
        yi_tui::keymap::default_keymap(),
        80,
    );
    app.set_pane();
    app
}

fn paint(app: &mut App, area: Rect) -> Vec<String> {
    let mut buffer = Buffer::empty(area);
    let mut scroll = 0;
    let _ = yi_tui::render::paint_pane(app, None, &mut buffer, area, &mut scroll);
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol().to_owned()))
                .collect()
        })
        .collect()
}

fn message(text: &str) -> yi_types::message::AgentMessage {
    yi_runtime::faux::faux_assistant_message(
        vec![yi_runtime::faux::faux_text(text)],
        StopReason::Stop,
    )
}

/// Streams `text` in `chunk`-byte deltas into a pane painted after each; returns the pane.
fn stream(text: &str, chunk: usize) -> App {
    let mut app = app();
    app.reduce_agent(AgentEvent::AgentStart);
    app.reduce_agent(AgentEvent::MessageStart {
        message: message(""),
    });
    let chars: Vec<char> = text.chars().collect();
    for delta in chars.chunks(chunk) {
        app.reduce_agent(AgentEvent::MessageUpdate {
            assistant_message_event: AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: delta.iter().collect(),
            },
        });
        let _ = paint(&mut app, Rect::new(0, 0, 70, 22));
    }
    app.reduce_agent(AgentEvent::MessageEnd {
        message: message(text),
    });
    app.reduce_agent(AgentEvent::AgentEnd {
        messages: Vec::new(),
    });
    app
}

/// Incident: re-rendering the whole answer at every commit made a 30 KB answer cost
/// 15 s of CPU; each streamed byte is now scanned and rendered a bounded number of times.
#[test]
fn a_streamed_answer_costs_work_linear_in_its_length() {
    let text = answer(12);
    let (scanned, rendered) = work();
    let _ = stream(&text, 16);
    let (scanned, rendered) = (work().0 - scanned, work().1 - rendered);
    assert!(
        scanned <= 4 * text.len(),
        "scanned {scanned} bytes of {}",
        text.len()
    );
    assert!(
        rendered <= 12 * text.len(),
        "rendered {rendered} bytes of {}",
        text.len()
    );
}

/// Incident: a closing fence committed alone rendered no rows and was dropped from the
/// transcript, so a pane showed everything after it as code.
#[test]
fn a_streamed_pane_matches_the_whole_answer_committed_at_once() {
    let text = answer(2);
    let area = Rect::new(0, 0, 70, 400);
    for chunk in [5, 16] {
        let mut streamed = stream(&text, chunk);
        let mut whole = app();
        whole.commit_cell(&Cell::Assistant {
            markdown: text.clone(),
        });
        assert_eq!(
            paint(&mut streamed, area),
            paint(&mut whole, area),
            "{chunk}-byte deltas"
        );
    }
}
