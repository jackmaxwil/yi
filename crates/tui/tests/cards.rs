use crate::common;

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
            lane: None,
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
        prev_tool: None,
        toolcalls: 2,
        tokens: 1500,
        elapsed_ms: 3000,
        error: None,
        spawn: None,
        answer: Some(answer.to_owned()),
        activity: ChildActivity::Writing,
        flag: None,
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
fn a_thought_folds_to_a_dim_count_and_carries_no_glyph() -> TestResult {
    let theme = theme();
    let cell = Cell::Thought {
        markdown: "one line".to_owned(),
    };
    let lines = cell.lines(80, &theme, TranscriptMode::Normal, 0);
    let row = lines.first().ok_or("no row")?;
    assert!(row.spans[0].style.add_modifier.contains(Modifier::ITALIC));
    assert_eq!(flat(&lines).join("\n"), "  thought · 1 line");
    let open = flat(&cell.lines(80, &theme, TranscriptMode::Thinking, 0)).join("\n");
    assert!(!open.contains('∴'), "{open}");
    assert!(open.contains("one line"), "{open}");
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
    assert_eq!(
        wide, 4,
        "two rows each for the digest and the body row: {rows:?}"
    );
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
                input: 1100,
                output: 620,
                cache_read: 2000,
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
        .find(|r| r.starts_with("  ↳ ran 1 · "))
        .ok_or_else(|| format!("no footer: {rows:?}"))?;
    assert!(footer.contains("3K in / 620 out · 64% cached"), "{footer}");
    Ok(())
}

/// C5: past the first request a read is expected, so a turn that read nothing says `0% cached`;
/// the first request of a conversation has nothing to read and says nothing.
#[test]
fn a_total_miss_after_the_first_request_shows_zero_cached() -> TestResult {
    let footer = |requests: usize| -> Result<String, Box<dyn std::error::Error>> {
        let mut app = app();
        app.reduce_agent(AgentEvent::AgentStart);
        for _ in 0..requests {
            let message: AgentMessage = serde_json::from_value(serde_json::json!({
                "role": "assistant", "content": [], "api": "faux", "provider": "faux",
                "model": "faux-1", "stopReason": "stop", "timestamp": 0,
                "usage": {"input": 30000, "output": 500, "cacheRead": 0, "cacheWrite": 0,
                    "totalTokens": 30500, "cost": {"input": 0, "output": 0, "cacheRead": 0,
                    "cacheWrite": 0, "total": 0}},
            }))?;
            app.reduce_agent(AgentEvent::MessageEnd { message });
        }
        app.reduce_agent(AgentEvent::AgentEnd {
            messages: Vec::new(),
        });
        let rows = flat(&app.take_commits());
        Ok(rows
            .into_iter()
            .find(|r| r.contains(" in / "))
            .unwrap_or_default())
    };
    let first = footer(1)?;
    assert!(!first.contains("cached"), "{first}");
    let second = footer(2)?;
    assert!(second.contains("60K in / 1K out · 0% cached"), "{second}");
    Ok(())
}

/// Dies with `model 100%`: the model's share of wall time read like a cache rate, and the owner
/// asked whether it was one. The footer times the model instead, and its only percentage is
/// the cache rate.
#[test]
fn a_turn_footer_times_the_model_and_its_only_percentage_is_the_cache_rate() -> TestResult {
    let reply = || -> Result<AgentMessage, serde_json::Error> {
        serde_json::from_value(json!({
            "role": "assistant", "content": [], "api": "openai-completions",
            "provider": "openrouter", "model": "anthropic/claude-opus-5.5",
            "stopReason": "stop", "timestamp": 0,
            "usage": {"input": 5000, "output": 200, "cacheRead": 0, "cacheWrite": 0,
                "totalTokens": 5200, "cost": {"input": 0, "output": 0, "cacheRead": 0,
                "cacheWrite": 0, "total": 0}},
        }))
    };
    let footer = |tools: bool| -> Result<String, Box<dyn Error>> {
        let mut app = app();
        app.reduce_agent(AgentEvent::AgentStart);
        for call in 0..2 {
            app.reduce_agent(AgentEvent::MessageStart { message: reply()? });
            std::thread::sleep(std::time::Duration::from_millis(5));
            app.reduce_agent(AgentEvent::MessageEnd { message: reply()? });
            if tools && call == 0 {
                ran(&mut app, "c1", "ok");
            }
        }
        app.reduce_agent(AgentEvent::AgentEnd {
            messages: Vec::new(),
        });
        Ok(footer_of(&mut app)?)
    };
    let with_tools = footer(true)?;
    assert!(with_tools.contains(" (model "), "{with_tools}");
    assert!(with_tools.contains(" · 0% cached"), "{with_tools}");
    assert_eq!(with_tools.matches('%').count(), 1, "{with_tools}");
    let talk = footer(false)?;
    assert!(!talk.contains("model"), "all of it was model time: {talk}");
    Ok(())
}

/// Three pointers in one turn landed as three padded blocks, one of them bare:
/// injected text takes one shape, and a run of it is one block.
#[test]
fn injected_text_takes_one_callout_shape_and_a_run_is_one_block() -> TestResult {
    use yi_tui::history::History;

    let theme = theme();
    let mut history = History::default();
    history.retain(Cell::Advisory {
        source: "reminder".to_owned(),
        text: "Relevant: skill://plan".to_owned(),
    });
    history.retain(Cell::Advisory {
        source: "reminder".to_owned(),
        text: "Relevant: skill://verify".to_owned(),
    });
    let rows = flat(&History::replay(
        &history,
        80,
        &theme,
        TranscriptMode::Thinking,
        100,
    ));
    assert_eq!(
        rows,
        vec![
            "  ▌ ⚑ reminder Relevant: skill://plan",
            "  ▌ ⚑ reminder Relevant: skill://verify",
        ]
    );
    let notice = Cell::Notice {
        text: "This task has outgrown one-shot handling; write the plan now.".to_owned(),
    };
    assert_eq!(
        flat(&notice.lines(80, &theme, TranscriptMode::Thinking, 0)),
        vec!["  ▌ ⚑ This task has outgrown one-shot handling; write the plan now."]
    );
    Ok(())
}

/// Dies with "3 tools" for a turn that read, edited and ran: the receipt now says which work
/// the turn did and how much of its time the model took.
#[test]
fn a_turn_receipt_names_its_kinds_of_work_and_the_model_time() -> TestResult {
    let mut app = app();
    app.reduce_agent(AgentEvent::AgentStart);
    let reply = || AgentMessage::Assistant {
        content: Vec::new(),
        api: "faux".to_owned(),
        provider: "faux".to_owned(),
        model: "faux-1".to_owned(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::zero(),
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    };
    app.reduce_agent(AgentEvent::MessageStart { message: reply() });
    std::thread::sleep(std::time::Duration::from_millis(20));
    app.reduce_agent(AgentEvent::MessageEnd { message: reply() });
    for (id, name) in [("c1", "read"), ("c2", "todo"), ("c3", "edit")] {
        app.reduce_agent(AgentEvent::ToolExecutionStart {
            tool_call_id: id.to_owned(),
            tool_name: name.to_owned(),
            args: json!({ "path": "src/lib.rs" }),
        });
        app.reduce_agent(AgentEvent::ToolExecutionEnd {
            tool_call_id: id.to_owned(),
            tool_name: name.to_owned(),
            result: ToolResult {
                content: Vec::new(),
                details: serde_json::Value::Null,
                usage: None,
                added_tool_names: None,
                terminate: None,
            },
            is_error: false,
        });
    }
    ran(&mut app, "c4", "ok");
    app.reduce_agent(AgentEvent::AgentEnd {
        messages: Vec::new(),
    });
    let rows = flat(&app.take_commits());
    let footer = rows
        .iter()
        .find(|r| r.starts_with("  ↳ "))
        .ok_or_else(|| format!("no footer: {rows:?}"))?;
    assert!(
        footer.starts_with("  ↳ read 1 · edited 1 · ran 1 · "),
        "{footer}"
    );
    assert!(footer.contains(" (model "), "{footer}");
    Ok(())
}

struct ClaimsPort(Vec<yi_types::todo::Claim>);

impl yi_tui::port::SessionPort for ClaimsPort {
    fn history(&mut self) -> yi_tui::port::Answer {
        yi_tui::port::Answer::Later
    }
    fn entries(&mut self) -> yi_tui::port::Answer {
        yi_tui::port::Answer::Later
    }
    fn rewind(&mut self, _: &str) -> yi_tui::port::Answer {
        yi_tui::port::Answer::Later
    }
    fn new_session(&mut self, _: &str, _: &str) -> yi_tui::port::Answer {
        yi_tui::port::Answer::Later
    }
    fn undo(&mut self, _: &str) -> yi_tui::port::Answer {
        yi_tui::port::Answer::Later
    }
    fn slash(&mut self, _: &str, _: &str, _: &str) -> yi_tui::port::Answer {
        yi_tui::port::Answer::Later
    }
    fn select(
        &mut self,
        _: yi_types::model::Model,
        _: yi_types::model::Effort,
    ) -> yi_tui::port::Answer {
        yi_tui::port::Answer::Later
    }
    fn plan(&mut self) -> yi_tui::port::Answer {
        yi_tui::port::Answer::Later
    }
    fn goal(&self) -> Option<yi_tui::hud::GoalView> {
        None
    }
    fn claims(&self, _: bool) -> Option<Vec<yi_types::todo::Claim>> {
        Some(self.0.clone())
    }
}

fn footer_of(app: &mut App) -> Result<String, String> {
    flat(&app.take_commits())
        .into_iter()
        .find(|row| row.starts_with("  ↳ "))
        .ok_or_else(|| "no footer".to_owned())
}

/// Dies with a turn's receipt counting todos a past turn closed, and with a turn that only
/// moved its list reading "1 tool", a count the receipt's own kinds leave out.
#[test]
fn a_receipt_counts_only_the_todos_its_turn_closed_and_no_list_calls() -> TestResult {
    let claim = |label: &str| yi_types::todo::Claim {
        label: label.to_owned(),
        observed: None,
    };
    let mut app = app();
    app.reduce_agent(AgentEvent::AgentStart);
    app.sync_port(Some(&ClaimsPort(vec![claim("write the report")])));
    ran(&mut app, "c1", "ok");
    app.reduce_agent(AgentEvent::AgentEnd {
        messages: Vec::new(),
    });
    let first = footer_of(&mut app)?;
    assert!(first.contains(" · done 0 observed, 1 claimed"), "{first}");
    let listed = |app: &mut App, id: &str| {
        app.reduce_agent(AgentEvent::ToolExecutionStart {
            tool_call_id: id.to_owned(),
            tool_name: "todo".to_owned(),
            args: json!({"op": "view"}),
        });
        app.reduce_agent(AgentEvent::ToolExecutionEnd {
            tool_call_id: id.to_owned(),
            tool_name: "todo".to_owned(),
            result: ToolResult {
                content: Vec::new(),
                details: serde_json::Value::Null,
                usage: None,
                added_tool_names: None,
                terminate: None,
            },
            is_error: false,
        });
    };
    app.reduce_agent(AgentEvent::AgentStart);
    listed(&mut app, "c2");
    ran(&mut app, "c3", "ok");
    app.reduce_agent(AgentEvent::AgentEnd {
        messages: Vec::new(),
    });
    let second = footer_of(&mut app)?;
    assert!(second.starts_with("  ↳ ran 1 · "), "{second}");
    assert!(!second.contains("claimed"), "{second}");
    app.reduce_agent(AgentEvent::AgentStart);
    listed(&mut app, "c4");
    app.reduce_agent(AgentEvent::AgentEnd {
        messages: Vec::new(),
    });
    let third = footer_of(&mut app);
    assert!(
        third.as_ref().is_err(),
        "a list-only turn used no tool: {third:?}"
    );
    Ok(())
}

/// The loop retries a stream that died before saying anything, so a 402 ends two turns
/// with the same words; the transcript says them once.
#[test]
fn a_retried_stream_error_draws_one_notice() {
    let failed = || AgentEvent::MessageEnd {
        message: AgentMessage::Assistant {
            content: Vec::new(),
            api: "faux".to_owned(),
            provider: "faux".to_owned(),
            model: "faux-1".to_owned(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::zero(),
            stop_reason: StopReason::Error,
            deferred: None,
            error_message: Some("HTTP 402: in-flight budget exhausted".to_owned()),
            raw_stop_reason: None,
            end_turn: None,
            timestamp: 0,
        },
    };
    let mut app = app();
    app.reduce_agent(failed());
    app.reduce_agent(failed());
    let rows = flat(&app.reflowed(80));
    let said = rows.iter().filter(|r| r.contains("HTTP 402")).count();
    assert_eq!(said, 1, "{rows:?}");
}

/// A first step has no previous one: its slot stays blank so the box does not grow a row when
/// the second call starts, and a box too narrow for its frame keeps both rows of steps.
#[test]
fn a_first_step_keeps_the_box_height_and_a_narrow_box_keeps_both_steps() -> TestResult {
    let mut cell = task(TaskStatus::Running, "");
    cell.description = "lecteur-é".to_owned();
    cell.last_tool = None;
    cell.step("read café.rs".to_owned());
    let first = flat(&cell.lines(60, &theme(), TranscriptMode::Normal, 0));
    let slot = format!("  │ {} │", " ".repeat(54));
    assert_eq!(first.get(2), Some(&slot), "{first:#?}");
    assert!(
        first
            .get(3)
            .is_some_and(|r| r.contains("⚙ read café.rs · 1K")),
        "{first:#?}"
    );
    cell.step("grep naïve".to_owned());
    let second = flat(&cell.lines(60, &theme(), TranscriptMode::Normal, 0));
    assert_eq!(first.len(), second.len(), "{first:#?}\n{second:#?}");
    let narrow = flat(&cell.lines(12, &theme(), TranscriptMode::Normal, 0));
    assert_eq!(
        narrow.len(),
        5,
        "blank, title, previous, current, blank: {narrow:#?}"
    );
    assert!(
        narrow.get(2).is_some_and(|r| r.contains("⚙ read café.rs")),
        "{narrow:#?}"
    );
    assert!(
        narrow.get(3).is_some_and(|r| r.contains("⚙ grep naïve")),
        "{narrow:#?}"
    );
    Ok(())
}
