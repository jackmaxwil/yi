//! The streaming commit path against its one invariant: what a message commits to scrollback,
//! slice by slice, is what the history rebuilds on a resize.

mod common;

use ratatui::text::Line;
use serde_json::json;
use yi_tui::app::{App, TuiOptions};
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::keymap::default_keymap;
use yi_types::event::{AgentEvent, AssistantMessageEvent, ToolResult};
use yi_types::message::{AgentMessage, Content, StopReason, Usage};

fn app(rows: usize) -> App {
    let mut app = App::new(
        TuiOptions {
            model: common::test_model("faux-1"),
            session_name: "streaming".to_owned(),
            cwd: "/tmp".to_owned(),
            lane: None,
            context_window: 128_000,
            session_dir: String::new(),
            keys: Vec::new(),
            initial_prompt: None,
            pace: 0,
        },
        Theme::new(ColorTier::TrueColor, true),
        default_keymap(),
        80,
    );
    app.set_rows(rows);
    app
}

fn flat(lines: &[Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

fn text(text: &str) -> Content {
    Content::Text {
        text: text.to_owned(),
        text_signature: None,
    }
}

fn thinking(text: &str) -> Content {
    Content::Thinking {
        thinking: text.to_owned(),
        thinking_signature: None,
        redacted: None,
    }
}

fn assistant(content: Vec<Content>, stop_reason: StopReason) -> AgentMessage {
    AgentMessage::Assistant {
        content,
        api: "faux".to_owned(),
        provider: "faux".to_owned(),
        model: "faux-1".to_owned(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::zero(),
        stop_reason,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    }
}

/// Grows `blocks` a character at a time, the way deltas arrive, then ends the message with
/// `last`; returns every row committed to scrollback.
fn stream_blocks(app: &mut App, blocks: &[Content], last: Vec<Content>) -> Vec<String> {
    let mut scroll = Vec::new();
    app.reduce_agent(AgentEvent::MessageStart {
        message: assistant(Vec::new(), StopReason::Stop),
    });
    let mut grown: Vec<Content> = Vec::new();
    for block in blocks {
        let (whole, make): (&str, fn(&str) -> Content) = match block {
            Content::Text { text: t, .. } => (t, text),
            Content::Thinking { thinking: t, .. } => (t, thinking),
            _ => continue,
        };
        grown.push(make(""));
        for (at, ch) in whole.char_indices() {
            if let Some(slot) = grown.last_mut() {
                *slot = make(&whole[..at + ch.len_utf8()]);
            }
            app.reduce_agent(AgentEvent::MessageUpdate {
                assistant_message_event: AssistantMessageEvent::Start {
                    partial: assistant(grown.clone(), StopReason::Stop),
                },
            });
            scroll.extend(flat(&app.take_commits()));
        }
    }
    app.reduce_agent(AgentEvent::MessageEnd {
        message: assistant(last, StopReason::Stop),
    });
    scroll.extend(flat(&app.take_commits()));
    scroll
}

fn stream(app: &mut App, source: &str) -> Vec<String> {
    stream_blocks(app, &[text(source)], vec![text(source)])
}

fn squeeze(rows: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for row in rows {
        if row.is_empty() && out.last().is_none_or(String::is_empty) {
            continue;
        }
        out.push(row);
    }
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out
}

/// Scrollback as committed, and the history's rebuild of it.
fn both(app: &App, scroll: Vec<String>) -> (Vec<String>, Vec<String>) {
    (squeeze(scroll), squeeze(flat(&app.reflowed(2_000))))
}

#[track_caller]
fn agree(app: &App, scroll: Vec<String>) -> Vec<String> {
    let (scroll, reflow) = both(app, scroll);
    let at = scroll
        .iter()
        .zip(&reflow)
        .position(|(a, b)| a != b)
        .unwrap_or(scroll.len().min(reflow.len()));
    let window = |rows: &[String]| {
        rows.get(at.saturating_sub(2)..(at + 3).min(rows.len()))
            .map(<[String]>::to_vec)
    };
    assert!(
        scroll == reflow,
        "scrollback and its rebuild disagree at row {at}:\n scroll {:?}\n reflow {:?}",
        window(&scroll),
        window(&reflow)
    );
    scroll
}

#[test]
fn text_after_a_fence_rebuilds_as_prose() {
    let mut app = app(40);
    let scroll = stream(
        &mut app,
        "Look:\n\n```rust\nfn a() {}\nfn b() {}\n```\n\nAfter the fence.\n",
    );
    let rows = agree(&app, scroll);
    assert_eq!(rows.last().map(String::as_str), Some("  After the fence."));
}

#[test]
fn a_forced_cut_keeps_bullets_and_nesting() {
    let mut app = app(12);
    let source: String = std::iter::once("Plan:\n\n".to_owned())
        .chain(
            (0..4).map(|i| format!("- top {i}\n  - child a{i}\n  - child b{i}\n  - child c{i}\n")),
        )
        .collect();
    let scroll = stream(&mut app, &source);
    let rows = agree(&app, scroll);
    assert!(
        rows.iter().any(|row| row.contains("◦ child a0")),
        "{rows:?}"
    );
}

#[test]
fn a_forced_cut_never_splits_bold_or_renumbers_a_list() {
    let mut app = app(12);
    let bold = format!(
        "{} **this bold phrase spans several words and should stay bold across rows** {}\n",
        "word ".repeat(60),
        "tail ".repeat(40)
    );
    let scroll = stream(&mut app, &bold);
    let rows = agree(&app, scroll);
    assert!(!rows.iter().any(|row| row.contains("**")), "{rows:?}");

    let mut app = self::app(12);
    let list: String = std::iter::once("Steps:\n\n".to_owned())
        .chain((0..10).map(|i| format!("1. step {i}\n")))
        .collect();
    let scroll = stream(&mut app, &list);
    let rows = agree(&app, scroll);
    assert!(
        rows.iter().any(|row| row.contains("10. step 9")),
        "{rows:?}"
    );
}

#[test]
fn an_inline_triple_backtick_is_not_a_fence() {
    let mut app = app(40);
    let scroll = stream(
        &mut app,
        "```rust``` marks the block language.\n\nNext paragraph here.\n\nLast one.\n",
    );
    let rows = agree(&app, scroll);
    let count = rows
        .iter()
        .filter(|row| row.contains("marks the block"))
        .count();
    assert_eq!(count, 1, "{rows:?}");
}

#[test]
fn a_blank_line_inside_a_list_item_is_not_a_cut() {
    let mut app = app(40);
    let scroll = stream(
        &mut app,
        "1. Install the tool.\n\n   Run it once to create the config.\n\n   - check the path\n   - check the key\n\n2. Start the server.\n",
    );
    let rows = agree(&app, scroll);
    assert!(
        rows.iter().any(|row| row.contains("◦ check the path")),
        "{rows:?}"
    );
}

#[test]
fn an_empty_final_message_keeps_what_streamed() {
    let mut app = app(40);
    let streamed = "First paragraph is done.\n\nSecond paragraph was on screen";
    let scroll = stream_blocks(&mut app, &[text(streamed)], Vec::new());
    let rows = agree(&app, scroll);
    assert!(
        rows.iter()
            .any(|row| row.contains("Second paragraph was on screen")),
        "{rows:?}"
    );
}

#[test]
fn thought_paragraphs_keep_their_blank_rows() {
    let mut app = app(40);
    app.cycle_mode();
    let thought = "First I read the file.\n\nThen I check the tests.\n\nFinally I decide.";
    let scroll = stream_blocks(
        &mut app,
        &[thinking(thought), text("Done.")],
        vec![thinking(thought), text("Done.")],
    );
    let rows = agree(&app, scroll);
    assert!(rows.iter().any(String::is_empty), "{rows:?}");
}

#[test]
fn a_thought_after_prose_commits_below_it() {
    let mut app = app(40);
    let blocks = [
        text("Let me check the config."),
        thinking("The config has a typo."),
        text("Found it: a typo."),
    ];
    app.cycle_mode();
    let scroll = stream_blocks(&mut app, &blocks, blocks.to_vec());
    let rows = agree(&app, scroll);
    let at = |needle: &str| rows.iter().position(|row| row.contains(needle));
    assert!(at("Let me check") < at("config has a typo"), "{rows:?}");
    assert!(at("config has a typo") < at("Found it"), "{rows:?}");
}

#[test]
fn two_text_blocks_are_two_paragraphs() {
    let mut app = app(40);
    let blocks = [text("One block."), text("Another block.")];
    let scroll = stream_blocks(&mut app, &blocks, blocks.to_vec());
    let rows = agree(&app, scroll);
    assert!(
        !rows.iter().any(|row| row.contains("block. Another")),
        "{rows:?}"
    );
}

fn todo_step(app: &mut App) {
    app.reduce_agent(AgentEvent::ToolExecutionEnd {
        tool_call_id: "t1".to_owned(),
        tool_name: "todo".to_owned(),
        result: ToolResult {
            content: vec![text("Todos 0/3\n1. parse")],
            details: json!(null),
            usage: None,
            added_tool_names: None,
            terminate: None,
        },
        is_error: false,
    });
}

#[test]
fn two_messages_never_merge_in_the_rebuild() {
    let mut app = app(40);
    let mut scroll = stream(&mut app, "Let me plan the work.");
    todo_step(&mut app);
    scroll.extend(flat(&app.take_commits()));
    scroll.extend(stream(&mut app, "Starting with the parser."));
    let rows = agree(&app, scroll);
    assert!(
        !rows.iter().any(|row| row.contains("work.Starting")),
        "{rows:?}"
    );
}

#[test]
fn prose_then_a_tool_card_is_spaced_the_same_in_the_rebuild() {
    let mut app = app(40);
    app.reduce_agent(AgentEvent::ToolExecutionEnd {
        tool_call_id: "t0".to_owned(),
        tool_name: "todo".to_owned(),
        result: ToolResult {
            content: vec![text("Todos 3/3")],
            details: json!(null),
            usage: None,
            added_tool_names: None,
            terminate: None,
        },
        is_error: false,
    });
    let mut scroll = flat(&app.take_commits());
    scroll.extend(stream(
        &mut app,
        "Here is a longer answer paragraph.\n\nWith a second paragraph.",
    ));
    app.reduce_agent(AgentEvent::ToolExecutionStart {
        tool_call_id: "t1".to_owned(),
        tool_name: "bash".to_owned(),
        args: json!({"cmd": "cargo test"}),
    });
    app.reduce_agent(AgentEvent::ToolExecutionEnd {
        tool_call_id: "t1".to_owned(),
        tool_name: "bash".to_owned(),
        result: ToolResult {
            content: vec![text("ok\nok\nok")],
            details: json!(null),
            usage: None,
            added_tool_names: None,
            terminate: None,
        },
        is_error: false,
    });
    scroll.extend(flat(&app.take_commits()));
    let rows = agree(&app, scroll);
    let prose = rows
        .iter()
        .position(|row| row.contains("With a second paragraph."))
        .unwrap_or(0);
    assert_eq!(
        rows.get(prose + 1).map(String::as_str),
        Some(""),
        "{rows:?}"
    );
}

/// A fence nested in a list item commits line by line like a top-level one, so the live tail
/// never holds the whole block.
#[test]
fn a_fence_inside_a_list_item_commits_as_it_streams() {
    let mut app = app(40);
    let code: String = (0..60).map(|i| format!("   let x{i} = {i};\n")).collect();
    let source = format!("1. Run:\n\n   ```rust\n{code}   ```\n2. Then check it.\n");
    let mut scroll = Vec::new();
    app.reduce_agent(AgentEvent::MessageStart {
        message: assistant(Vec::new(), StopReason::Stop),
    });
    let half = source.find("x40").unwrap_or(source.len());
    for end in 1..=half {
        app.reduce_agent(AgentEvent::MessageUpdate {
            assistant_message_event: AssistantMessageEvent::Start {
                partial: assistant(vec![text(&source[..end])], StopReason::Stop),
            },
        });
        scroll.extend(flat(&app.take_commits()));
    }
    assert!(
        scroll.iter().any(|row| row.contains("let x30 = 30;")),
        "line 30 is committed before line 40 arrives: {scroll:?}"
    );
    for end in half + 1..=source.len() {
        app.reduce_agent(AgentEvent::MessageUpdate {
            assistant_message_event: AssistantMessageEvent::Start {
                partial: assistant(vec![text(&source[..end])], StopReason::Stop),
            },
        });
        scroll.extend(flat(&app.take_commits()));
    }
    app.reduce_agent(AgentEvent::MessageEnd {
        message: assistant(vec![text(&source)], StopReason::Stop),
    });
    scroll.extend(flat(&app.take_commits()));
    let rows = agree(&app, scroll);
    assert!(
        rows.iter().any(|row| row.contains("2. Then check it.")),
        "{rows:?}"
    );
}

/// The live region as painted: every row of the chat at 80 columns.
fn live_rows(app: &mut App) -> Vec<String> {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    let layout = yi_tui::render::layout_chat(app, None, None, 40);
    let area = Rect::new(0, 0, 80, layout.rows().min(40));
    let mut buffer = Buffer::empty(area);
    yi_tui::render::paint_chat(app, &layout, &mut buffer, area);
    (0..area.height)
        .map(|y| {
            (0..80)
                .map(|x| buffer.cell((x, y)).map_or(" ", |cell| cell.symbol()))
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

/// Incident: `**bol` drew its asterisks until the closer arrived, then the row shifted left.
#[test]
fn an_open_span_renders_closed_while_it_streams() {
    let mut app = app(40);
    app.reduce_agent(AgentEvent::MessageStart {
        message: assistant(Vec::new(), StopReason::Stop),
    });
    app.reduce_agent(AgentEvent::MessageUpdate {
        assistant_message_event: AssistantMessageEvent::Start {
            partial: assistant(vec![text("Body text **bol and `cod")], StopReason::Stop),
        },
    });
    let rows = live_rows(&mut app);
    assert!(
        rows.iter().any(|row| row.contains("Body text bol and cod")),
        "{rows:?}"
    );
}

/// A pane reads the history every frame; the rows a slice commits extend the message's
/// cached rows instead of re-rendering it, and must equal what a fresh render gives.
#[test]
fn a_pane_reading_every_frame_sees_the_fresh_render() {
    let source = "Intro paragraph.\n\n```rust\nfn a() {}\nfn b() {}\n```\n\nAfter **the** fence.\n\n- one\n- two\n";
    let mut app = app(40);
    app.reduce_agent(AgentEvent::MessageStart {
        message: assistant(Vec::new(), StopReason::Stop),
    });
    for end in 1..=source.len() {
        app.reduce_agent(AgentEvent::MessageUpdate {
            assistant_message_event: AssistantMessageEvent::Start {
                partial: assistant(vec![text(&source[..end])], StopReason::Stop),
            },
        });
        let _ = app.reflowed(2_000);
    }
    app.reduce_agent(AgentEvent::MessageEnd {
        message: assistant(vec![text(source)], StopReason::Stop),
    });
    let cached = flat(&app.reflowed(2_000));
    let mut fresh = self::app(40);
    let _ = stream(&mut fresh, source);
    assert_eq!(cached, flat(&fresh.reflowed(2_000)));
}

// Round two: the refute pass over the first fixes.

#[test]
fn a_nested_fence_keeps_its_item_around_it() {
    for source in [
        "1. Run:\n\n   ```sh\n   cargo test\n   ```\n\n   This runs the suite.\n\n2. Next step.\n",
        "- a\n\n  ```\n  x\n  ```\n\n  more item text\n\nafter\n",
        "- a\n\n  ```\n  x\n  ```\n\n  - sub\n\nafter\n",
        "1. first\n\n  ```\n  code line one\n  code line two\n  ```\n\nafter text\n",
        "- a\n\n   ```\n   code\n   ```\n\nafter\n",
        "1. Run:\n\n   ```\n\tfoo\n   bar\n   ```\n\nDone.\n",
        "1. Run:\n\n   ```\n   code\n   ```\n1. Then check it.\n",
    ] {
        let mut app = app(40);
        let scroll = stream(&mut app, source);
        let (scroll, reflow) = both(&app, scroll);
        assert_eq!(scroll, reflow, "{source:?}");
    }
}

#[test]
fn a_tab_indented_fence_is_indented_code() {
    let mut app = app(40);
    let scroll = stream(
        &mut app,
        "Code:\n\n\t```rust\n\tlet a = 1;\n\tlet b = 2;\n\t```\n\nDone here.\n",
    );
    let rows = agree(&app, scroll);
    let openers = rows.iter().filter(|row| row.contains("```rust")).count();
    assert_eq!(openers, 1, "{rows:?}");
}

#[test]
fn ordinary_text_never_reads_as_an_open_span() {
    for source in [
        "Search files matching *.rs in the crate",
        "Search every **/*.ts file under src",
        "The method (_private) is internal",
        "Type the ` character",
    ] {
        let mut app = app(40);
        app.reduce_agent(AgentEvent::MessageStart {
            message: assistant(Vec::new(), StopReason::Stop),
        });
        app.reduce_agent(AgentEvent::MessageUpdate {
            assistant_message_event: AssistantMessageEvent::Start {
                partial: assistant(vec![text(source)], StopReason::Stop),
            },
        });
        let rows = live_rows(&mut app);
        assert!(rows.iter().any(|row| row.contains(source)), "{rows:?}");
    }
}

/// Incident: a long item or quote had no break to cut at, so its top left the screen and
/// reached scrollback only when the block ended.
#[test]
fn a_long_list_item_or_quote_commits_while_it_streams() {
    for lead in ["- ", "> ", "1. "] {
        let source = format!(
            "{lead}{}\n\nafter\n",
            "lorem ipsum dolor sit amet ".repeat(60)
        );
        let mut app = app(12);
        app.reduce_agent(AgentEvent::MessageStart {
            message: assistant(Vec::new(), StopReason::Stop),
        });
        let mut scroll = Vec::new();
        let half = source.len() / 2;
        for end in 1..=half {
            app.reduce_agent(AgentEvent::MessageUpdate {
                assistant_message_event: AssistantMessageEvent::Start {
                    partial: assistant(vec![text(&source[..end])], StopReason::Stop),
                },
            });
            scroll.extend(flat(&app.take_commits()));
        }
        assert!(
            scroll.iter().any(|row| row.contains("lorem")),
            "{lead:?} committed nothing mid-block: {scroll:?}"
        );
        for end in half + 1..=source.len() {
            app.reduce_agent(AgentEvent::MessageUpdate {
                assistant_message_event: AssistantMessageEvent::Start {
                    partial: assistant(vec![text(&source[..end])], StopReason::Stop),
                },
            });
            scroll.extend(flat(&app.take_commits()));
        }
        app.reduce_agent(AgentEvent::MessageEnd {
            message: assistant(vec![text(&source)], StopReason::Stop),
        });
        scroll.extend(flat(&app.take_commits()));
        let (scroll, reflow) = both(&app, scroll);
        assert_eq!(scroll, reflow, "{lead:?}");
    }
}

#[test]
fn an_empty_thought_between_texts_keeps_them_apart() {
    for between in ["\n", ""] {
        let mut app = app(40);
        let blocks = [text("Said first."), thinking(between), text("Said second.")];
        let scroll = stream_blocks(&mut app, &blocks, blocks.to_vec());
        let rows = agree(&app, scroll);
        assert!(
            !rows.iter().any(|row| row.contains("first.Said")),
            "{rows:?}"
        );
    }
}

#[test]
fn a_replayed_session_keeps_messages_apart() {
    let mut app = app(40);
    let message = |id: &str, seq: u64, said: &str| yi_types::entry::Entry::Message {
        id: id.to_owned(),
        message: assistant(vec![text(said)], StopReason::Stop),
        terminate: None,
        parent_id: None,
        seq,
        timestamp: 0,
    };
    app.replay_entries(&[
        message("a1", 1, "Let me plan the work."),
        message("a2", 2, "Starting with the parser."),
    ]);
    let rows = flat(&app.reflowed(200));
    assert!(
        !rows.iter().any(|row| row.contains("work.Starting")),
        "{rows:?}"
    );
}

#[test]
fn a_forced_cut_never_opens_a_slice_on_a_block_marker() {
    let mut app = app(12);
    let source: String = (0..120)
        .map(|i| format!("r{i} - "))
        .chain(std::iter::once("end.\n".to_owned()))
        .collect();
    let scroll = stream(&mut app, &source);
    let rows = agree(&app, scroll);
    assert!(!rows.iter().any(|row| row.contains('‣')), "{rows:?}");
}

/// Only a retried stream's repeated error folds; a notice the host says twice shows twice.
#[test]
fn a_repeated_host_notice_still_shows() {
    let mut app = app(40);
    app.notice("/undo: the current turn is still running");
    app.notice("/undo: the current turn is still running");
    let rows = flat(&app.reflowed(200));
    let shown = rows.iter().filter(|row| row.contains("/undo")).count();
    assert_eq!(shown, 2, "{rows:?}");
}

/// Markdown shapes a model writes, joined at random: the generator behind the fixtures above.
const PIECES: &[&str] = &[
    "Plain words here and there. ",
    "More text with **bold words** inside. ",
    "A `code span` too. ",
    "\n\n",
    "\n",
    "- item one\n",
    "- item two with more words to wrap around the edge\n",
    "  - nested child\n",
    "1. first\n",
    "2. second\n",
    "1. lazy\n",
    "> quoted text line\n",
    "```rust\nfn main() {}\nlet x = 1;\n```\n",
    "   ```sh\n   cargo test\n   ```\n",
    "## Heading\n",
    "| a | b |\n|---|---|\n| 1 | 2 |\n",
    "word - word - word ",
    "\n\n   continued item text\n",
    "lorem ipsum dolor sit amet consectetur adipiscing elit ",
    "*.rs and **/*.ts globs ",
    "snake_case_name ",
    "---\n",
    "\t```\n\tcode\n\t```\n",
    "10. ten\n",
    "1) paren\n",
    "> - quoted item\n",
    "Vec<String> and <br> tags ",
    "line\r\n",
    "**open bold ",
    "`open code ",
    "# Title\n",
    "    indented code\n",
    "~~~\ntilde\n~~~\n",
    "- [ ] task\n",
    "***\n",
    "Setext\n===\n",
    "a | b\n-- | --\nc | d\n",
];

fn next(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *state >> 33
}

/// Three hundred generated streams, each at a random height and width: scrollback equals its
/// rebuild in every one. Text still to arrive can re-read earlier text (a span closed later,
/// a tab-indented fence in an item), which no cut can know; those seeds are past this range.
#[test]
fn generated_streams_rebuild_as_they_scrolled() {
    let pick = |state: &mut u64, len: usize| usize::try_from(next(state)).unwrap_or(0) % len;
    for seed in 0..300u64 {
        let mut state = seed + 1;
        let count = 3 + pick(&mut state, 14);
        let source: String = (0..count)
            .map(|_| PIECES[pick(&mut state, PIECES.len())])
            .collect();
        let rows = [6, 12, 40][pick(&mut state, 3)];
        let width = [30, 60, 80, 120][pick(&mut state, 4)];
        let mut app = app(rows);
        app.set_width(width);
        let scroll = stream(&mut app, &source);
        let (scroll, reflow) = both(&app, scroll);
        assert_eq!(
            scroll, reflow,
            "seed {seed}, {rows} rows, {width} wide: {source:?}"
        );
    }
}

/// Incident: a cut before `` `cargo test` `` escaped the backtick, and the stray one left
/// swallowed spaces into a code span (`cargo testand *emph* wordcargo`).
#[test]
fn a_cut_before_inline_code_or_emphasis_keeps_the_span() {
    for (width, rows) in [(30, 6), (40, 6), (50, 6), (60, 12)] {
        let mut app = app(rows);
        app.set_width(width);
        let source = format!("{}\n", "`cargo test` and *emph* word `a|b` ".repeat(40));
        let scroll = stream(&mut app, &source);
        let rows = agree(&app, scroll);
        assert!(
            !rows.iter().any(|row| row.contains(['`', '\\', '*'])),
            "{width}: {rows:?}"
        );
    }
}

/// A footer committed between two slices of one message splits it the same way in scrollback
/// and in the rebuild.
#[test]
fn a_cell_committed_mid_message_splits_it_alike() {
    let mut app = app(40);
    app.reduce_agent(AgentEvent::MessageStart {
        message: assistant(Vec::new(), StopReason::Stop),
    });
    let first = "First paragraph here.\n\nSecond";
    let mut scroll = Vec::new();
    for end in 1..=first.len() {
        app.reduce_agent(AgentEvent::MessageUpdate {
            assistant_message_event: AssistantMessageEvent::Start {
                partial: assistant(vec![text(&first[..end])], StopReason::Stop),
            },
        });
        scroll.extend(flat(&app.take_commits()));
    }
    app.notice("a footer between the slices");
    scroll.extend(flat(&app.take_commits()));
    let whole = "First paragraph here.\n\nSecond paragraph.\n\nThird one.";
    for end in first.len() + 1..=whole.len() {
        app.reduce_agent(AgentEvent::MessageUpdate {
            assistant_message_event: AssistantMessageEvent::Start {
                partial: assistant(vec![text(&whole[..end])], StopReason::Stop),
            },
        });
        scroll.extend(flat(&app.take_commits()));
    }
    app.reduce_agent(AgentEvent::MessageEnd {
        message: assistant(vec![text(whole)], StopReason::Stop),
    });
    scroll.extend(flat(&app.take_commits()));
    agree(&app, scroll);
}

/// A stream that never ended leaves nothing that hides the next message's blocks.
#[test]
fn an_unended_stream_does_not_hide_the_next_message() {
    let mut app = app(40);
    let blocks = [text("Said first."), thinking("then"), text("Said second.")];
    app.reduce_agent(AgentEvent::MessageStart {
        message: assistant(Vec::new(), StopReason::Stop),
    });
    app.reduce_agent(AgentEvent::MessageUpdate {
        assistant_message_event: AssistantMessageEvent::Start {
            partial: assistant(blocks.to_vec(), StopReason::Stop),
        },
    });
    let scroll = stream(&mut app, "The next answer.");
    assert!(
        scroll.iter().any(|row| row.contains("The next answer.")),
        "{scroll:?}"
    );
}

#[test]
fn a_nested_fence_closed_deeper_than_it_opened_ends_there() {
    let mut app = app(40);
    let source =
        "1. Run:\n\n   ```sh\n   cargo test\n     ```\n\n   After the fence.\n\n2. Next.\n";
    let scroll = stream(&mut app, source);
    let rows = agree(&app, scroll);
    assert!(
        rows.iter().any(|row| row.trim() == "After the fence."),
        "{rows:?}"
    );
}

/// A thought's committed rows extend its cached rows as prose's do, and match a fresh render.
#[test]
fn a_pane_reading_a_thought_every_frame_sees_the_fresh_render() {
    let thought = "First I read the file.\n\n- one\n- two\n\nThen I check the tests.\n\nDone.";
    let mut app = app(40);
    app.cycle_mode();
    app.reduce_agent(AgentEvent::MessageStart {
        message: assistant(Vec::new(), StopReason::Stop),
    });
    for end in 1..=thought.len() {
        app.reduce_agent(AgentEvent::MessageUpdate {
            assistant_message_event: AssistantMessageEvent::Start {
                partial: assistant(vec![thinking(&thought[..end])], StopReason::Stop),
            },
        });
        let _ = app.reflowed(2_000);
    }
    app.reduce_agent(AgentEvent::MessageEnd {
        message: assistant(vec![thinking(thought)], StopReason::Stop),
    });
    let cached = flat(&app.reflowed(2_000));
    let mut fresh = self::app(40);
    fresh.cycle_mode();
    let _ = stream_blocks(&mut fresh, &[thinking(thought)], vec![thinking(thought)]);
    assert_eq!(cached, flat(&fresh.reflowed(2_000)));
}

/// A thought streaming a long fence shows the fence's last lines, drawn from its tail alone.
#[test]
fn a_thought_in_an_open_fence_shows_its_last_lines() {
    let code: String = (0..300).map(|i| format!("let x{i} = {i};\n")).collect();
    let thought = format!("Plan:\n\n```rust\n{code}");
    let mut app = app(40);
    app.cycle_mode();
    app.reduce_agent(AgentEvent::MessageStart {
        message: assistant(Vec::new(), StopReason::Stop),
    });
    app.reduce_agent(AgentEvent::MessageUpdate {
        assistant_message_event: AssistantMessageEvent::Start {
            partial: assistant(vec![thinking(&thought)], StopReason::Stop),
        },
    });
    let rows = live_rows(&mut app);
    assert!(
        rows.iter().any(|row| row.contains("let x299 = 299;")),
        "{rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("let x285 = 285;")),
        "{rows:?}"
    );
}
