mod common;

use std::error::Error;

use ratatui::style::Modifier;
use ratatui::text::Line;
use serde_json::json;
use yi_tui::app::{App, TuiOptions};
use yi_tui::cell::{Cell, TaskCell, TaskStatus, TranscriptMode, tail_bounded};
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::highlight::Token;
use yi_tui::keymap::default_keymap;
use yi_tui::tree::{TreeFilter, TreeView};
use yi_types::event::{AgentEvent, ToolResult};
use yi_types::message::{AgentMessage, Content, Cost, StopReason, Usage};
use yi_types::subagent::ChildActivity;

type TestResult = Result<(), Box<dyn Error>>;

fn theme() -> Theme {
    Theme::new(ColorTier::TrueColor, true)
}

fn app() -> App {
    App::new(
        TuiOptions {
            model: common::test_model("faux-1"),
            session_name: "cards".to_owned(),
            cwd: "/tmp".to_owned(),
            context_window: 128_000,
            session_dir: String::new(),
            keys: Vec::new(),
            initial_prompt: None,
            pace: 0,
        },
        theme(),
        default_keymap(),
        80,
    )
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

fn ran(app: &mut App, id: &str, text: &str) {
    app.reduce_agent(AgentEvent::ToolExecutionStart {
        tool_call_id: id.to_owned(),
        tool_name: "bash".to_owned(),
        args: json!({ "cmd": "grid scope" }),
    });
    app.reduce_agent(AgentEvent::ToolExecutionEnd {
        tool_call_id: id.to_owned(),
        tool_name: "bash".to_owned(),
        result: ToolResult {
            content: vec![Content::Text {
                text: text.to_owned(),
                text_signature: None,
            }],
            details: serde_json::Value::Null,
            usage: None,
            added_tool_names: None,
            terminate: None,
        },
        is_error: false,
    });
}

/// The verbose body of one bash call, rendered at 80 columns.
fn verbose_rows(text: &str) -> Vec<String> {
    let mut app = app();
    ran(&mut app, "c1", text);
    app.cycle_mode();
    assert_eq!(app.mode(), TranscriptMode::Verbose);
    flat(&app.reflowed(80))
}

fn task(status: TaskStatus, answer: &str) -> TaskCell {
    TaskCell {
        child_id: "sub-c4bc6302".to_owned(),
        description: "reply-with-the-single-sub-c4bc6302".to_owned(),
        status,
        last_tool: Some("bash ls".to_owned()),
        toolcalls: 2,
        tokens: 1500,
        elapsed_ms: 3000,
        error: None,
        spawn: None,
        answer: Some(answer.to_owned()),
        activity: ChildActivity::Writing,
    }
}

#[test]
fn palette_maps_tokens_and_headings() -> TestResult {
    let theme = theme();
    assert_eq!(
        theme.syntax_style(Token::Keyword).fg,
        Some(ratatui::style::Color::Rgb(0xc0, 0x99, 0xff))
    );
    let lines = yi_tui::markdown::render("# Title\n\nsome `code` here", 40, &theme);
    let title = lines
        .iter()
        .find(|l| l.spans.iter().any(|s| s.content.contains("Title")))
        .ok_or("no heading row")?;
    assert_eq!(title.spans[0].style.fg, Some(theme.accent));
    let code = lines
        .iter()
        .flat_map(|l| &l.spans)
        .find(|s| s.content.as_ref() == "code")
        .ok_or("no inline code span")?;
    assert_eq!(code.style.fg, Some(theme.orange));
    let ansi = Theme::new(ColorTier::Ansi16, false);
    assert_eq!(ansi.magenta, ratatui::style::Color::Magenta);
    Ok(())
}

#[test]
fn a_thought_row_is_a_purple_glyph_and_a_count() -> TestResult {
    let theme = theme();
    let cell = Cell::Thought {
        markdown: "one line".to_owned(),
    };
    let lines = cell.lines(80, &theme, TranscriptMode::Normal, 0);
    let row = lines.first().ok_or("no row")?;
    assert_eq!(row.spans[0].content.as_ref(), "  ∴");
    assert_eq!(row.spans[0].style.fg, Some(theme.purple));
    let text = flat(&lines).join("\n");
    assert_eq!(text, "  ∴ 1 lines");
    Ok(())
}

#[test]
fn user_rows_carry_the_bar_and_no_caret() -> TestResult {
    let theme = theme();
    let prompt = "x".repeat(76);
    let cell = Cell::User {
        text: prompt.clone(),
    };
    let rows = flat(&cell.lines(80, &theme, TranscriptMode::Normal, 0));
    assert_eq!(rows.len(), 3, "blank, prompt, blank: {rows:?}");
    assert_eq!(rows[1], format!("┃   {prompt}"));
    assert!(!rows.iter().any(|r| r.contains('›')), "{rows:?}");
    Ok(())
}

#[test]
fn a_tool_only_assistant_entry_summarizes_its_calls() -> TestResult {
    let mut arguments = serde_json::Map::new();
    arguments.insert("cmd".to_owned(), json!("grid roots; echo ---"));
    let entries = vec![yi_types::entry::Entry::Message {
        id: "a1".to_owned(),
        message: AgentMessage::Assistant {
            content: vec![Content::ToolCall {
                id: "t1".to_owned(),
                name: "bash".to_owned(),
                arguments,
                thought_signature: None,
                namespace: None,
            }],
            api: "faux".to_owned(),
            provider: "faux".to_owned(),
            model: "faux-1".to_owned(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::zero(),
            stop_reason: StopReason::ToolUse,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: 0,
        },
        terminate: None,
        parent_id: None,
        seq: 1,
        timestamp: 0,
    }];
    let view = TreeView::new(&entries, Some("a1"), TreeFilter::Default);
    let rows = flat(&view.lines(80, &theme(), 10));
    assert!(
        rows.iter()
            .any(|r| r.contains("⚙ bash grid roots; echo ---")),
        "{rows:?}"
    );
    assert!(!rows.iter().any(|r| r.contains("(no content)")), "{rows:?}");
    Ok(())
}

#[test]
fn a_long_result_keeps_head_and_tail_in_verbose() -> TestResult {
    let text: Vec<String> = (1..=40).map(|n| format!("line {n}")).collect();
    let rows = verbose_rows(&text.join("\n"));
    for needle in ["line 1", "line 12", "… 22 more lines", "line 35", "line 40"] {
        assert!(
            rows.iter().any(|r| r.contains(needle)),
            "{needle}: {rows:?}"
        );
    }
    assert!(!rows.iter().any(|r| r.contains("line 20")), "{rows:?}");
    Ok(())
}

#[test]
fn a_json_blob_is_pretty_printed_before_capping() -> TestResult {
    let rows = verbose_rows(r#"{"a":1,"b":{"c":2}}"#);
    assert!(rows.iter().any(|r| r.contains("\"a\": 1")), "{rows:?}");
    Ok(())
}

#[test]
fn an_over_wide_line_folds_to_two_rows_and_a_size() -> TestResult {
    let rows = verbose_rows(&"x".repeat(600));
    let wide = rows.iter().filter(|r| r.contains("xxxx")).count();
    assert_eq!(wide, 2, "one digest row and one body row survive: {rows:?}");
    assert_eq!(
        rows.iter().filter(|r| r.contains("… 1 KB")).count(),
        2,
        "{rows:?}"
    );
    Ok(())
}

#[test]
fn an_oversized_blob_skips_pretty_printing() -> TestResult {
    let blob = format!("{{\"a\":\"{}\"}}", "y".repeat(300 * 1024));
    let rows = verbose_rows(&blob);
    assert!(!rows.iter().any(|r| r.contains("\"a\": ")), "{rows:?}");
    assert!(rows.iter().any(|r| r.contains("… 301 KB")), "{rows:?}");
    Ok(())
}

#[test]
fn a_finished_card_is_titled_boxed_and_collapsed_in_normal_mode() -> TestResult {
    let cell = task(TaskStatus::Done, "- one\n- two\n- three\n- four\n- five");
    let rows = flat(&cell.lines(80, &theme(), TranscriptMode::Normal, 0));
    let title = rows
        .iter()
        .find(|r| r.starts_with("  ╭─"))
        .ok_or("no top")?;
    assert!(
        title.contains("↳ Reply with the single sub · c4bc6302 · done 3s · 2 tool calls"),
        "{title}"
    );
    assert_eq!(
        rows.iter().filter(|r| r.starts_with("  │ ")).count(),
        4,
        "{rows:?}"
    );
    assert!(
        rows.iter().any(|r| r.contains("… 2 more lines")),
        "{rows:?}"
    );
    assert!(rows.iter().any(|r| r.starts_with("  ╰")), "{rows:?}");
    let verbose = flat(&cell.lines(80, &theme(), TranscriptMode::Verbose, 0));
    assert!(verbose.iter().any(|r| r.contains("five")), "{verbose:?}");
    Ok(())
}

#[test]
fn a_running_child_card_fades_toward_its_top() -> TestResult {
    let cell = task(TaskStatus::Running, "- a\n- b\n- c\n- d\n- e");
    let lines = cell.lines(60, &theme(), TranscriptMode::Verbose, 0);
    let rows = flat(&lines);
    assert!(rows.iter().any(|r| r.contains("writing")), "{rows:?}");
    assert!(
        rows.iter().any(|r| r.contains("⚙ bash ls · 1K tokens")),
        "{rows:?}"
    );
    assert!(
        !rows.iter().any(|r| r.contains("• a")),
        "the oldest row scrolled off: {rows:?}"
    );
    let fg = |needle: &str| {
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.as_ref() == needle)
            .map(|s| s.style.fg)
    };
    assert_ne!(fg("b"), fg("e"), "the top row is faded, the newest is not");
    let ansi = task(TaskStatus::Running, "- a\n- b\n- c\n- d\n- e");
    let lines = ansi.lines(
        60,
        &Theme::new(ColorTier::Ansi16, true),
        TranscriptMode::Verbose,
        0,
    );
    let dimmed = lines
        .iter()
        .find(|l| l.spans.iter().any(|s| s.content.as_ref() == "b"))
        .ok_or("no b row")?;
    assert!(
        dimmed
            .spans
            .iter()
            .any(|s| s.style.add_modifier.contains(Modifier::DIM)),
        "16 colours fall back to DIM"
    );
    Ok(())
}

#[test]
fn a_child_answer_is_bounded_to_sixteen_kilobytes() -> TestResult {
    let text = format!("{}tail", "x".repeat(20 * 1024));
    let kept = tail_bounded(text);
    assert!(kept.len() <= 16 * 1024, "{}", kept.len());
    assert!(kept.ends_with("tail"));
    assert_eq!(tail_bounded("short".to_owned()), "short");
    Ok(())
}

#[test]
fn a_turn_ends_with_a_dim_footer() -> TestResult {
    let mut app = app();
    app.reduce_agent(AgentEvent::AgentStart);
    ran(&mut app, "c1", "ok");
    let zero = serde_json::Number::from(0);
    app.reduce_agent(AgentEvent::MessageEnd {
        message: AgentMessage::Assistant {
            content: vec![Content::Text {
                text: "done".to_owned(),
                text_signature: None,
            }],
            api: "faux".to_owned(),
            provider: "faux".to_owned(),
            model: "faux-1".to_owned(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage {
                input: 3100,
                output: 620,
                cache_read: 0,
                cache_write: 0,
                cache_write1h: None,
                reasoning: None,
                total_tokens: 3720,
                cost: Cost {
                    input: zero.clone(),
                    output: zero.clone(),
                    cache_read: zero.clone(),
                    cache_write: zero.clone(),
                    total: zero,
                },
                unknown: false,
            },
            stop_reason: StopReason::Stop,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: 0,
        },
    });
    app.reduce_agent(AgentEvent::AgentEnd {
        messages: Vec::new(),
    });
    let rows = flat(&app.take_commits());
    let footer = rows
        .iter()
        .find(|r| r.starts_with("  ↳ 1 tool · "))
        .ok_or_else(|| format!("no footer: {rows:?}"))?;
    assert!(footer.contains("3K in / 620 out"), "{footer}");
    Ok(())
}
