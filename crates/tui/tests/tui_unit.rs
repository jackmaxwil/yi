use std::error::Error;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use yi_tui::cell::{Cell, ToolCell, ToolStatus, TranscriptMode};
use yi_tui::colors::{ColorTier, Theme, detect_dark, detect_tier, name_accent};
use yi_tui::composer::{Composer, sanitize_paste};
use yi_tui::frame::FrameScheduler;
use yi_tui::hud::{BoardCard, CardKind, CardStatus, HudInput};
use yi_tui::keymap::{Action, EvalContext, KeyCodeValue, KeyInput, SingleKey, default_keymap};
use yi_tui::markdown::commit_complete_source;
use yi_tui::status::StatusInput;
use yi_tui::tree::{TreeFilter, TreeResult, TreeView};
use yi_tui::wrap::wrap_line;

type TestResult = Result<(), Box<dyn Error>>;

fn theme() -> Theme {
    Theme::new(ColorTier::TrueColor, true)
}

fn flat(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

#[test]
fn wrap_never_splits_a_url_token() -> TestResult {
    let line = Line::from(Span::raw(
        "see https://example.com/a/very/long/path/that/exceeds/the/width here",
    ));
    let wrapped = wrap_line(&line, 20, "");
    let joined: Vec<String> = wrapped.iter().map(flat).collect();
    assert!(
        joined
            .iter()
            .any(|l| l.contains("https://example.com/a/very/long/path/that/exceeds/the/width")),
        "URL must stay one intact token even past the width: {joined:?}"
    );
    Ok(())
}

#[test]
fn wrap_splits_plain_overlong_tokens_and_indents_continuations() -> TestResult {
    let line = Line::from(Span::raw(format!("{} tail", "x".repeat(30))));
    let wrapped = wrap_line(&line, 10, "  ");
    assert!(wrapped.len() > 2);
    for continuation in wrapped.iter().skip(1) {
        assert!(
            flat(continuation).starts_with("  "),
            "continuation lines carry the indent"
        );
    }
    Ok(())
}

#[test]
fn wrap_is_unicode_width_aware() -> TestResult {
    let line = Line::from(Span::raw("汉字汉字汉字"));
    let wrapped = wrap_line(&line, 4, "");
    assert!(
        wrapped.len() >= 3,
        "double-width chars wrap by cells, not chars: {wrapped:?}"
    );
    Ok(())
}

#[test]
fn keymap_round_trips_and_overrides() -> TestResult {
    let mut map = default_keymap();
    map.apply_overrides(vec![("ctrl-x", "abort")])?;
    let key = KeyInput::parse("ctrl-x")?;
    assert_eq!(key.to_string(), "ctrl-x");
    let resolved = map.resolve(&key, &EvalContext::default());
    assert_eq!(resolved, Some(Action::Abort));
    assert!(
        map.apply_overrides(vec![("ctrl-x", "no-such-action")])
            .is_err()
    );
    Ok(())
}

#[test]
fn paste_atoms_collapse_and_expand_exactly() -> TestResult {
    let mut composer = Composer::default();
    let body = (0..30)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    composer.handle_paste(&body);
    let marker_text = composer.text();
    assert!(
        marker_text.contains("[Paste #1, +30 lines]"),
        "a >10-line paste collapses to a marker: {marker_text}"
    );
    let submitted = composer.take_submission().ok_or("submission missing")?;
    assert!(
        submitted.contains(&body),
        "submit expands the marker back to the exact pasted content"
    );
    Ok(())
}

#[test]
fn paste_expansion_is_single_pass_and_longest_label_first() -> TestResult {
    let mut composer = Composer::default();
    for _ in 0..11 {
        composer.handle_paste(&"a\n".repeat(11));
    }
    let text = composer.text();
    assert!(text.contains("[Paste #11, +12 lines]"));
    let expanded = composer.expand_markers("[Paste #11, +12 lines] [Paste #1, +12 lines]");
    assert!(
        !expanded.contains("[Paste #"),
        "#1 must not shadow #11: {expanded}"
    );
    Ok(())
}

#[test]
fn sanitize_paste_strips_control_keeps_newlines() -> TestResult {
    let out = sanitize_paste("a\r\nb\x1b[31mc\td");
    assert_eq!(out, "a\nb[31mc   d");
    Ok(())
}

#[test]
fn commit_complete_source_gates_on_newline() -> TestResult {
    let (stable, tail) = commit_complete_source("done line\npartial");
    assert_eq!(stable, "done line\n");
    assert_eq!(tail, "partial");
    let (stable, tail) = commit_complete_source("no newline yet");
    assert_eq!(stable, "");
    assert_eq!(tail, "no newline yet");
    Ok(())
}

#[test]
fn frame_scheduler_honors_floor_and_ceiling() -> TestResult {
    use std::time::{Duration, Instant};
    let mut scheduler = FrameScheduler::default();
    let t0 = Instant::now();
    scheduler.request();
    assert!(scheduler.should_draw(t0));
    scheduler.mark_drawn(t0, t0 + Duration::from_millis(50));
    scheduler.request();
    assert!(
        !scheduler.should_draw(t0 + Duration::from_millis(60)),
        "a 50 ms draw pushes the floor to start + 100 ms"
    );
    assert!(scheduler.should_draw(t0 + Duration::from_millis(101)));
    Ok(())
}

fn card(status: CardStatus) -> BoardCard {
    BoardCard {
        title: "abc".to_owned(),
        kind: CardKind::Subagent,
        status,
        detail: "work".to_owned(),
    }
}

#[test]
fn hud_spine_lights_progress_with_clamps() -> TestResult {
    let theme = theme();
    let input = HudInput {
        goal: None,
        cards: vec![card(CardStatus::Done), card(CardStatus::Running)],
        steering: Vec::new(),
        follow_up: Vec::new(),
    };
    let lines = yi_tui::hud::render(&input, &theme, 0);
    assert!(flat(&lines[0]).contains("Subagents"));
    let accent = Style::default().fg(theme.accent);
    let first_spine = lines[1].spans.first().ok_or("spine span missing")?;
    assert_eq!(
        first_spine.style, accent,
        "some progress lights at least one spine cell"
    );
    let tail = lines.last().ok_or("tail missing")?;
    assert_ne!(
        tail.spans.first().map(|s| s.style),
        Some(accent),
        "the spine never fully lights while work remains"
    );
    Ok(())
}

#[test]
fn hud_empty_input_renders_nothing() -> TestResult {
    let lines = yi_tui::hud::render(&HudInput::default(), &theme(), 0);
    assert!(lines.is_empty());
    Ok(())
}

#[test]
fn status_cascade_keeps_path_and_model_when_narrow() -> TestResult {
    let input = StatusInput {
        model: "faux-1".to_owned(),
        cwd: "/very/long/path/to/somewhere/deep/project".to_owned(),
        cost: Some("$1.23".to_owned()),
        session_name: "a-quite-long-session-name".to_owned(),
        subagents: 2,
        context_used: 50_000,
        context_window: 128_000,
        threshold_pct: Some(80),
        ..StatusInput::default()
    };
    let row = yi_tui::status::render(&input, 40, &theme());
    let text = flat(&row);
    assert!(text.contains("faux-1"), "model survives: {text}");
    assert!(text.contains("…"), "path shrank by middle-ellipsis: {text}");
    Ok(())
}

#[test]
fn status_context_segment_is_compact() -> TestResult {
    let input = StatusInput {
        model: "m".to_owned(),
        cwd: "/p".to_owned(),
        session_name: "s".to_owned(),
        context_used: 64_000,
        context_window: 128_000,
        threshold_pct: Some(80),
        ..StatusInput::default()
    };
    let row = yi_tui::status::render(&input, 80, &theme());
    let text = flat(&row);
    assert!(
        text.contains("50% of 128K"),
        "compact context segment: {text}"
    );
    let wide = StatusInput {
        context_window: 1_048_576,
        ..input
    };
    let text = flat(&yi_tui::status::render(&wide, 80, &theme()));
    assert!(
        text.contains("of 1M"),
        "megatoken windows collapse to 1M: {text}"
    );
    assert!(!text.contains('┃'), "the gauge bar is gone: {text}");
    Ok(())
}

#[test]
fn a_streaming_table_keeps_its_header_on_a_normal_screen() -> TestResult {
    let source = "| crate | role |\n| --- | --- |\n| yi-types | shapes |\n| yi-loop | turns |\n| yi-ai | providers |\n| yi-session | transcripts |\n| yi-tools | tools |\n| yi-tui | screen |\n";
    let rendered = yi_tui::markdown::render(source, 60, &theme());
    assert!(rendered.len() > 6, "the table is taller than the old tail");
    let shown: Vec<String> = yi_tui::render::live_tail(rendered.clone(), 48)
        .iter()
        .map(flat)
        .collect();
    assert!(
        shown.iter().any(|line| line.contains("crate")),
        "a table that fits the screen streams whole: {shown:?}"
    );
    let cramped: Vec<String> = yi_tui::render::live_tail(rendered, 8)
        .iter()
        .map(flat)
        .collect();
    assert!(
        !cramped.iter().any(|line| line.contains("crate")),
        "and only a screen with no room drops its head: {cramped:?}"
    );
    Ok(())
}

/// Feed `full` through the streaming path in small deltas, the way a provider
/// delivers it, and hand back the app that rendered it.
fn streamed(full: &str) -> yi_tui::app::App {
    use yi_tui::app::{App, TuiOptions};
    use yi_tui::keymap::default_keymap;
    let mut app = App::new(
        TuiOptions {
            model_label: "faux-1".to_owned(),
            session_name: "s".to_owned(),
            cwd: "/tmp".to_owned(),
            context_window: 128_000,
            session_dir: String::new(),
            keys: Vec::new(),
            initial_prompt: None,
        },
        theme(),
        default_keymap(),
        80,
    );
    let assistant = |text: &str| yi_types::message::AgentMessage::Assistant {
        content: vec![yi_types::message::Content::Text {
            text: text.to_owned(),
            text_signature: None,
        }],
        api: String::new(),
        provider: String::new(),
        model: String::new(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: yi_types::message::Usage::zero(),
        stop_reason: yi_types::message::StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    };
    app.reduce_agent(yi_types::event::AgentEvent::AgentStart);
    let mut end = 8;
    while end < full.len() {
        while !full.is_char_boundary(end.min(full.len())) {
            end += 1;
        }
        let message = assistant(full.get(..end.min(full.len())).unwrap_or(full));
        app.reduce_agent(yi_types::event::AgentEvent::MessageUpdate {
            message: message.clone(),
            assistant_message_event: yi_types::event::AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: String::new(),
                partial: message,
            },
        });
        end += 17;
    }
    app.reduce_agent(yi_types::event::AgentEvent::MessageEnd {
        message: assistant(full),
    });
    app
}

#[test]
fn a_reflow_repaint_bullets_the_message_not_every_paragraph() -> TestResult {
    let mut app = streamed("One paragraph.\n\nTwo paragraph.\n\nThree paragraph.\n");
    let live = app
        .take_commits()
        .iter()
        .map(flat)
        .filter(|line| line.trim_start().starts_with("\u{2022} "))
        .count();
    let reflowed = app
        .reflowed(200)
        .iter()
        .map(flat)
        .filter(|line| line.trim_start().starts_with("\u{2022} "))
        .count();
    assert_eq!(live, 1, "the live paint bullets the message once");
    assert_eq!(
        reflowed, live,
        "a resize repaint draws what the live paint drew"
    );
    Ok(())
}

#[test]
fn a_callout_line_does_not_also_take_the_bullet() -> TestResult {
    let mut app = streamed("> the premise, quoted\n\nAnd the prose after it.\n");
    let doubled: Vec<String> = app
        .take_commits()
        .iter()
        .map(flat)
        .filter(|line| line.contains('\u{2022}') && line.contains('\u{258c}'))
        .collect();
    assert!(
        doubled.is_empty(),
        "a callout is marked once, by its rail: {doubled:?}"
    );
    Ok(())
}

#[test]
fn the_status_bar_does_not_repeat_the_program_name() -> TestResult {
    let input = StatusInput {
        model: "deepseek/deepseek-v4".to_owned(),
        cwd: "/home/dev/yi".to_owned(),
        session_name: "01a03a5e".to_owned(),
        ..StatusInput::default()
    };
    let text = flat(&yi_tui::status::render(&input, 80, &theme()));
    let segments: Vec<&str> = text.split('\u{b7}').map(str::trim).collect();
    assert!(
        !segments.contains(&"yi"),
        "the terminal already says whose window this is: {segments:?}"
    );
    assert!(text.contains("deepseek/deepseek-v4"), "{text}");
    Ok(())
}

fn entry(id: &str, parent: Option<&str>, seq: u64, text: &str) -> yi_types::entry::Entry {
    yi_types::entry::Entry::Message {
        id: id.to_owned(),
        message: yi_types::message::AgentMessage::User {
            content: yi_types::message::UserContent::Text(text.to_owned()),
            timestamp: 0,
        },
        terminate: None,
        parent_id: parent.map(str::to_owned),
        seq,
        timestamp: 0,
    }
}

#[test]
fn tree_flattens_branches_and_rewinds_to_selection() -> TestResult {
    let entries = vec![
        entry("a", None, 1, "root prompt"),
        entry("b", Some("a"), 2, "first follow-up"),
        entry("c", Some("a"), 3, "branched follow-up"),
    ];
    let mut view = TreeView::new(&entries, Some("c"), TreeFilter::Default);
    let lines = view.lines(80, &theme(), 10);
    let text: Vec<String> = lines.iter().map(flat).collect();
    assert!(text.iter().any(|l| l.contains("root prompt")));
    assert!(
        text.iter().any(|l| l.contains("├─")) && text.iter().any(|l| l.contains("└─")),
        "forked children get connectors: {text:?}"
    );
    let key = yi_tui::keymap::SingleKey::parse("enter")?;
    match view.handle_key(&key) {
        TreeResult::Rewind(id) => assert_eq!(id, "c", "enter rewinds to the selected entry"),
        _ => return Err("enter must produce a rewind".into()),
    }
    Ok(())
}

#[test]
fn color_detection_defaults_dark_and_names_are_stable() -> TestResult {
    assert!(detect_dark(None));
    assert!(!detect_dark(Some("0;15")));
    assert!(detect_dark(Some("15;0")));
    assert_eq!(detect_tier(Some("truecolor"), None), ColorTier::TrueColor);
    assert_eq!(
        detect_tier(None, Some("xterm-256color")),
        ColorTier::Ansi256
    );
    assert_eq!(name_accent("abc"), name_accent("abc"));
    Ok(())
}

#[test]
fn markdown_renders_fences_dim_and_headings_bold() -> TestResult {
    let theme = theme();
    let lines = yi_tui::markdown::render("# Title\n\n```rust\nlet x = 1;\n```", 60, &theme);
    let text: Vec<String> = lines.iter().map(flat).collect();
    assert!(text.iter().any(|l| l.contains("Title")));
    assert!(
        !text.iter().any(|l| l.contains("```")),
        "the author's fence markers are chrome, not content: {text:?}"
    );
    assert!(
        text.iter().any(|l| l.contains("│ rust")),
        "the rail carries the language instead: {text:?}"
    );
    assert!(
        text.iter().any(|l| l.contains("│ let x = 1;")),
        "fenced body hangs off the rail: {text:?}"
    );
    let title_line = lines
        .iter()
        .find(|l| flat(l).contains("Title"))
        .ok_or("missing title")?;
    assert!(
        title_line
            .spans
            .iter()
            .any(|s| s.style.add_modifier.contains(Modifier::BOLD)),
        "headings render bold"
    );
    Ok(())
}

#[test]
fn thought_cells_are_labeled_and_dim_while_prose_stays_bright() -> TestResult {
    let theme = theme();
    let thought = yi_tui::cell::Cell::Thought {
        markdown: "weighing the options".to_owned(),
    };
    let lines = thought.lines(80, &theme, yi_tui::cell::TranscriptMode::Thinking, 0);
    let text: Vec<String> = lines.iter().map(flat).collect();
    assert!(
        text.iter().any(|l| l.contains("∴ thinking")),
        "reasoning carries its label: {text:?}"
    );
    assert!(
        lines
            .iter()
            .flat_map(|l| &l.spans)
            .filter(|s| !s.content.trim().is_empty())
            .all(|s| s.style.add_modifier.contains(Modifier::ITALIC)),
        "reasoning body renders dim italic"
    );
    let prose = yi_tui::cell::Cell::Assistant {
        markdown: "the answer".to_owned(),
    };
    let lines = prose.lines(80, &theme, yi_tui::cell::TranscriptMode::Normal, 0);
    assert!(
        lines
            .iter()
            .flat_map(|l| &l.spans)
            .filter(|s| s.content.contains("the answer"))
            .all(|s| !s.style.add_modifier.contains(Modifier::ITALIC)
                && s.style.fg == Some(ratatui::style::Color::Reset)),
        "prose stays bright terminal fg, never italic"
    );
    Ok(())
}

#[test]
fn markdown_tables_render_as_grids_not_raw_pipes() -> TestResult {
    let theme = theme();
    let source = "| Tool | Status |\n|---|---|\n| bash | ok |\n| grep | ok |";
    let lines = yi_tui::markdown::render(source, 60, &theme);
    let text: Vec<String> = lines.iter().map(flat).collect();
    assert!(
        text.iter()
            .any(|l| l.contains("Tool") && l.contains("Status")),
        "header row renders both columns: {text:?}"
    );
    assert!(
        text.iter().any(|l| l.starts_with('┌') && l.ends_with('┐')),
        "the table is boxed: {text:?}"
    );
    assert!(
        text.iter().any(|l| l.starts_with('├') && l.contains('┼')),
        "header rule has real junctions: {text:?}"
    );
    assert!(
        text.iter().any(|l| l.starts_with('└') && l.ends_with('┘')),
        "the box closes: {text:?}"
    );
    assert_eq!(
        text.iter().filter(|l| l.starts_with('├')).count(),
        1,
        "one rule under the header, none between body rows: {text:?}"
    );
    assert!(
        !text.iter().any(|l| l.contains("|---|")),
        "raw markdown pipes never leak: {text:?}"
    );
    Ok(())
}

#[test]
fn stable_cut_stops_at_blank_lines_outside_fences() -> TestResult {
    use yi_tui::markdown::stable_cut;
    let cut = stable_cut("para one\n\npara two streaming");
    assert_eq!(cut, "para one\n\n".len());
    let fenced = "```\ncode\n\nstill code\n";
    assert_eq!(stable_cut(fenced), 0, "blank lines inside fences never cut");
    assert_eq!(stable_cut("no boundary yet"), 0);
    Ok(())
}

#[test]
fn advisory_tags_are_stripped_for_display() -> TestResult {
    let clean = yi_tui::cell::strip_tags(
        "<advisory severity=\"note\" guidance=\"weigh\">pending bash call is irreversible.</advisory>",
    );
    assert_eq!(clean, "pending bash call is irreversible.");
    Ok(())
}

#[test]
fn status_path_truncates_from_the_left() -> TestResult {
    let input = StatusInput {
        model: "faux-1".to_owned(),
        cwd: "/Users/someone/Development/some/deep/project".to_owned(),
        session_name: "s".to_owned(),
        context_window: 128_000,
        ..StatusInput::default()
    };
    let row = yi_tui::status::render(&input, 60, &theme());
    let text = flat(&row);
    assert!(
        text.contains("project"),
        "the path tail (project dir) survives: {text}"
    );
    assert!(
        !text.contains("/Users/someone"),
        "the head is dropped: {text}"
    );
    Ok(())
}

#[test]
fn orb_engine_feeds_the_kitty_painter() -> TestResult {
    use yi_tui::orb::{OrbState, evaluate, kitty};
    let frame = evaluate(OrbState::Working, 64, 1.3).ok_or("preset missing")?;
    assert!(!frame.dots.is_empty());
    let rgba = kitty::paint_rgba(&frame, 64.0, 96);
    assert_eq!(rgba.len(), 96 * 96 * 4);
    let lit = rgba.chunks(4).filter(|px| px[3] > 0).count();
    assert!(
        lit > 200,
        "a working orb must light pixels with alpha depth: {lit}"
    );
    let background = rgba.chunks(4).filter(|px| px[3] == 0).count();
    assert!(
        background > 1000,
        "the background stays transparent for the terminal ground: {background}"
    );
    Ok(())
}

#[test]
fn table_cells_keep_inline_code_and_leak_nothing() -> TestResult {
    let theme = theme();
    let source = "| Tool | Does |\n|---|---|\n| `read` | Reads `files` |\n| `bash` | Runs |";
    let lines = yi_tui::markdown::render(source, 60, &theme);
    let text: Vec<String> = lines.iter().map(flat).collect();
    assert!(
        text.iter()
            .any(|l| l.contains("read") && l.contains("Reads")),
        "code-formatted cells stay in their row: {text:?}"
    );
    assert!(
        !text
            .iter()
            .any(|l| l.contains("readbash") || l.contains("filesRuns")),
        "no concatenated leak after the table: {text:?}"
    );
    Ok(())
}

#[test]
fn streaming_commits_each_list_item_exactly_once() -> TestResult {
    use yi_tui::app::{App, TuiOptions};
    use yi_tui::keymap::default_keymap;
    let mut app = App::new(
        TuiOptions {
            model_label: "faux-1".to_owned(),
            session_name: "s".to_owned(),
            cwd: "/tmp".to_owned(),
            context_window: 128_000,
            session_dir: String::new(),
            keys: Vec::new(),
            initial_prompt: None,
        },
        theme(),
        default_keymap(),
        80,
    );
    let full =
        "Tools:\n\n1. read — reads files\n\n2. edit — patches files\n\n3. bash — runs commands\n";
    let assistant = |text: &str| yi_types::message::AgentMessage::Assistant {
        content: vec![yi_types::message::Content::Text {
            text: text.to_owned(),
            text_signature: None,
        }],
        api: String::new(),
        provider: String::new(),
        model: String::new(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: yi_types::message::Usage::zero(),
        stop_reason: yi_types::message::StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    };
    app.reduce_agent(yi_types::event::AgentEvent::AgentStart);
    for end in [10, 34, 60, full.len()] {
        let message = assistant(full.get(..end).unwrap_or(full));
        app.reduce_agent(yi_types::event::AgentEvent::MessageUpdate {
            message: message.clone(),
            assistant_message_event: yi_types::event::AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: String::new(),
                partial: message,
            },
        });
    }
    app.reduce_agent(yi_types::event::AgentEvent::MessageEnd {
        message: assistant(full),
    });
    let committed: Vec<String> = app
        .take_commits()
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect();
    let joined = committed.join("\n");
    for needle in [
        "read — reads files",
        "edit — patches files",
        "bash — runs commands",
    ] {
        assert_eq!(
            joined.matches(needle).count(),
            1,
            "each streamed item commits exactly once: {needle}\n{joined}"
        );
    }
    Ok(())
}

#[test]
fn wrapped_paragraph_continuation_is_not_indented() {
    let theme = Theme::new(ColorTier::TrueColor, true);
    let source = "I can also spin up sub-agents (rlm) to work on parts of a task in parallel, \
                  and I have a persistent Python kernel for scratch work.";
    let lines = yi_tui::markdown::render(source, 60, &theme);
    let texts: Vec<String> = lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();
    let first = texts
        .first()
        .map_or(0, |t: &String| t.len() - t.trim_start().len());
    for (i, text) in texts.iter().enumerate().skip(1) {
        if text.trim().is_empty() {
            continue;
        }
        let lead = text.len() - text.trim_start().len();
        assert_eq!(
            lead, first,
            "continuation line {i} of a plain paragraph must align with its first line: {texts:?}"
        );
    }
}

#[test]
fn logo_dots_travel_between_the_wordmark_and_the_orb() -> TestResult {
    let rest = yi_tui::logo::frame(0.0, 0.0, 64).ok_or("no resting mark")?;
    let working = yi_tui::logo::frame(1.0, 0.0, 64).ok_or("no working orb")?;
    let mid = yi_tui::logo::frame(0.5, 0.0, 64).ok_or("no mid-morph frame")?;

    // The mark at rest is the `Yi` strokes: every dot sits on one of the five
    // segments, so the shape is letters and not a cloud.
    assert!(
        rest.dots.len() > 40,
        "the mark has body: {}",
        rest.dots.len()
    );
    // `Y` and `i` each end in a vertical stem, so the mark has two dense
    // columns — a cloud or a ring would have none.
    let mut columns: std::collections::BTreeMap<i64, usize> = std::collections::BTreeMap::new();
    for dot in &rest.dots {
        *columns.entry(dot.x.round() as i64).or_default() += 1;
    }
    let stems = columns.values().filter(|count| **count >= 6).count();
    assert!(
        stems >= 2,
        "two stems stand in the mark: columns {columns:?}"
    );

    // Half way, the dots are in neither shape — that is the whole contract:
    // they visibly rearrange rather than cutting between two pictures.
    let count = mid.dots.len();
    assert!(count > 0, "mid-morph renders dots");
    let moved = mid
        .dots
        .iter()
        .zip(&rest.dots)
        .filter(|(m, r)| (m.x - r.x).abs() > 0.5 || (m.y - r.y).abs() > 0.5)
        .count();
    assert!(
        moved * 2 > count,
        "most dots have left the wordmark by half way: {moved}/{count}"
    );
    assert!(
        !working.dots.is_empty(),
        "the working end of the morph is the orb engine's own frame"
    );
    Ok(())
}

#[test]
fn a_long_idle_stretch_does_not_consume_the_whole_morph() {
    let frame = std::time::Duration::from_millis(yi_tui::logo::FRAME_MS);
    let one = yi_tui::logo::advance(0.0, 1.0, frame);
    assert!(one > 0.0 && one < 0.2, "one frame is a small step: {one}");

    // The mark stops repainting once it settles, so the gap since the last
    // paint can be minutes. Turning that into progress skipped the animation
    // entirely and the wordmark snapped straight to the orb.
    let after_idle = yi_tui::logo::advance(0.0, 1.0, std::time::Duration::from_secs(90));
    assert!(
        after_idle <= one,
        "a long idle gap still advances by at most one frame: {after_idle}"
    );

    let mut phase = 0.0;
    let mut frames = 0;
    while phase < 1.0 && frames < 1000 {
        phase = yi_tui::logo::advance(phase, 1.0, frame);
        frames += 1;
    }
    assert!(
        (10..=40).contains(&frames),
        "the morph takes a visible number of frames, not one: {frames}"
    );
}

#[test]
fn tree_panel_matches_the_omp_layout() -> TestResult {
    let entries = vec![
        entry("a", None, 1, "root prompt"),
        assistant_entry("r", Some("a"), 2, "root answer"),
        entry("b", Some("r"), 3, "follow-up"),
    ];
    let view = TreeView::new(&entries, Some("b"), TreeFilter::Default);
    let lines = view.lines(60, &theme(), 5);
    let text: Vec<String> = lines.iter().map(flat).collect();
    let first = text.first().ok_or("no lines")?;
    assert!(
        first.starts_with("╭─ Session Tree ") && first.ends_with('╮'),
        "titled top border: {first}"
    );
    assert!(
        text.iter().any(|line| line.contains("Enter: rewind.")),
        "help row: {text:?}"
    );
    assert!(
        text.iter().any(|line| line.contains("Search:")),
        "search row: {text:?}"
    );
    assert!(
        text.iter()
            .any(|line| line.starts_with('├') && line.ends_with('┤')),
        "section divider: {text:?}"
    );
    assert!(
        text.iter().any(|line| line.contains("› ● user: follow-up")),
        "cursor, active-path bullet and role prefix: {text:?}"
    );
    assert!(
        text.iter().any(|line| line.contains("● assistant: ")),
        "assistant rows carry their role: {text:?}"
    );
    assert_eq!(
        text.last()
            .map(|line| line.starts_with('╰') && line.ends_with('╯')),
        Some(true),
        "closed panel: {text:?}"
    );
    for line in &text {
        assert_eq!(
            line.chars().count(),
            60,
            "every panel row spans the width: {line}"
        );
    }
    Ok(())
}

#[test]
fn tree_scrolls_around_the_selection_with_a_scrollbar() -> TestResult {
    let mut entries = vec![entry("e0", None, 1, "prompt 0")];
    for index in 1..12 {
        entries.push(entry(
            &format!("e{index}"),
            Some(&format!("e{}", index - 1)),
            index + 1,
            &format!("prompt {index}"),
        ));
    }
    let view = TreeView::new(&entries, Some("e11"), TreeFilter::Default);
    let lines = view.lines(60, &theme(), 4);
    let text: Vec<String> = lines.iter().map(flat).collect();
    let rows: Vec<&String> = text
        .iter()
        .filter(|line| line.contains("prompt "))
        .collect();
    assert_eq!(rows.len(), 4, "the window holds max_rows rows: {text:?}");
    assert!(
        rows.iter().any(|line| line.contains("prompt 11")),
        "the selection stays visible: {rows:?}"
    );
    assert!(
        rows.iter().any(|line| line.contains('█')),
        "an overflowing list gets a scrollbar thumb: {rows:?}"
    );
    Ok(())
}

#[test]
fn alt_arrows_step_whole_turns() -> TestResult {
    let entries = vec![
        entry("u1", None, 1, "first ask"),
        assistant_entry("a1", Some("u1"), 2, "first answer"),
        entry("u2", Some("a1"), 3, "second ask"),
        assistant_entry("a2", Some("u2"), 4, "second answer"),
    ];
    let mut view = TreeView::new(&entries, Some("a2"), TreeFilter::Default);
    let up = yi_tui::keymap::SingleKey::parse("alt-up")?;
    view.handle_key(&up);
    let enter = yi_tui::keymap::SingleKey::parse("enter")?;
    match view.handle_key(&enter) {
        TreeResult::Rewind(id) => assert_eq!(id, "u2", "alt-up lands on the turn's user message"),
        _ => return Err("enter must rewind".into()),
    }
    Ok(())
}

fn assistant_entry(id: &str, parent: Option<&str>, seq: u64, text: &str) -> yi_types::entry::Entry {
    yi_types::entry::Entry::Message {
        id: id.to_owned(),
        message: yi_types::message::AgentMessage::Assistant {
            content: vec![yi_types::message::Content::Text {
                text: text.to_owned(),
                text_signature: None,
            }],
            api: "faux".to_owned(),
            provider: "faux".to_owned(),
            model: "faux-1".to_owned(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: yi_types::message::Usage::zero(),
            stop_reason: yi_types::message::StopReason::Stop,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: 0,
        },
        terminate: None,
        parent_id: parent.map(str::to_owned),
        seq,
        timestamp: 0,
    }
}

#[test]
fn loose_list_keeps_each_marker_on_its_item_line() -> TestResult {
    let theme = theme();
    let source = "1. **read** — Read a file.\n\n2. **edit** — Patch a file.\n";
    let text: Vec<String> = yi_tui::markdown::render(source, 60, &theme)
        .iter()
        .map(flat)
        .collect();
    assert!(
        text.iter()
            .any(|l| l.starts_with("1. ") && l.contains("read")),
        "the ordered marker leads its own text: {text:?}"
    );
    assert!(
        text.iter()
            .any(|l| l.starts_with("2. ") && l.contains("edit")),
        "every item keeps its marker: {text:?}"
    );
    assert!(
        !text.iter().any(|l| l.trim() == "1." || l.trim() == "2."),
        "no marker is stranded on a line of its own: {text:?}"
    );
    Ok(())
}

#[test]
fn loose_bullets_keep_their_glyph_and_tight_ones_stay_dense() -> TestResult {
    let theme = theme();
    let loose: Vec<String> = yi_tui::markdown::render("- alpha\n\n- beta\n", 60, &theme)
        .iter()
        .map(flat)
        .collect();
    assert_eq!(
        loose,
        vec!["• alpha".to_owned(), String::new(), "• beta".to_owned()],
        "a loose list keeps its author's breathing room"
    );
    let tight: Vec<String> = yi_tui::markdown::render("- alpha\n- beta\n", 60, &theme)
        .iter()
        .map(flat)
        .collect();
    assert_eq!(
        tight,
        vec!["• alpha".to_owned(), "• beta".to_owned()],
        "a tight list stays dense"
    );
    Ok(())
}

#[test]
fn wrapped_items_hang_under_their_marker_text() -> TestResult {
    let theme = theme();
    let source = "10. a much longer item that must wrap across the line\n";
    let text: Vec<String> = yi_tui::markdown::render(source, 40, &theme)
        .iter()
        .map(flat)
        .collect();
    let continuation = text
        .get(1)
        .ok_or("a 40-column render of this item wraps to two lines")?;
    assert!(
        continuation.starts_with("    ") && !continuation.starts_with("     "),
        "continuation clears the four-column `10. ` marker: {text:?}"
    );
    Ok(())
}

#[test]
fn nested_items_hang_under_their_own_marker() -> TestResult {
    let theme = theme();
    let source = "- outer\n  - nested bullet with enough text that it wraps onto another line\n";
    let text: Vec<String> = yi_tui::markdown::render(source, 40, &theme)
        .iter()
        .map(flat)
        .collect();
    let nested = text
        .iter()
        .position(|l| l.contains("nested bullet"))
        .ok_or("the nested item renders")?;
    let continuation = text
        .get(nested + 1)
        .ok_or("the nested item wraps at 40 columns")?;
    assert!(
        continuation.starts_with("    ") && !continuation.starts_with("     "),
        "a nested continuation clears both markers: {text:?}"
    );
    Ok(())
}

fn tool_cell(name: &str, result: &str, failed: bool) -> ToolCell {
    ToolCell {
        name: name.to_owned(),
        status: if failed {
            ToolStatus::Failed
        } else {
            ToolStatus::Done
        },
        summary: ToolCell::summary_of(name, ""),
        digest: ToolCell::digest_of(name, result, failed),
        preview: result.lines().map(str::to_owned).collect(),
        ..ToolCell::default()
    }
}

fn rendered(cell: &ToolCell, mode: TranscriptMode) -> Vec<String> {
    Cell::Tool(cell.clone())
        .lines(80, &theme(), mode, 0)
        .iter()
        .map(flat)
        .collect()
}

#[test]
fn a_finished_tool_states_its_outcome_without_switching_modes() -> TestResult {
    let theme = theme();
    let cases = [
        ("read", "[a.rs#CF8C]\n1:one\n2:two\n3:three", "3 lines"),
        (
            "edit",
            "[a.rs#77BC]\nupdated; first change at line 210\n210:new",
            "updated; first change at line 210",
        ),
        (
            "grep",
            "src/a.rs:3:hit\nsrc/a.rs-4-ctx\nsrc/b.rs:9:hit",
            "2 hits",
        ),
        ("glob", "src/a.rs\nsrc/b.rs", "2 files"),
        (
            "write",
            "Wrote 36 bytes to /tmp/x",
            "Wrote 36 bytes to /tmp/x",
        ),
    ];
    for (name, result, expected) in cases {
        let lines = rendered(&tool_cell(name, result, false), TranscriptMode::Normal);
        assert!(
            lines
                .iter()
                .any(|line| line.contains(&format!("└ {expected}"))),
            "{name} normal mode should state `{expected}`: {lines:?}"
        );
    }
    // A single hit and a single line read as singular, not "1 hits".
    let one = tool_cell("grep", "src/a.rs:3:hit", false);
    assert_eq!(one.digest.as_deref(), Some("1 hit"));
    let _ = theme;
    Ok(())
}

#[test]
fn a_failed_call_shows_why_in_every_mode() -> TestResult {
    let cell = tool_cell("bash", "error[E0308]: mismatched types\n  --> a.rs:1", true);
    for mode in [
        TranscriptMode::Normal,
        TranscriptMode::Thinking,
        TranscriptMode::Verbose,
    ] {
        let lines = rendered(&cell, mode);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("error[E0308]: mismatched types")),
            "a failure must be legible in {mode:?}: {lines:?}"
        );
    }
    Ok(())
}

#[test]
fn verbose_reads_hang_off_a_line_number_gutter_and_drop_the_anchor_header() -> TestResult {
    let cell = tool_cell(
        "read",
        "[a.rs#CF8C]\n9:fn main() {\n10:    body()\n11:}",
        false,
    );
    let lines = rendered(&cell, TranscriptMode::Verbose);
    let body = lines.join("\n");
    assert!(
        !body.contains("CF8C"),
        "anchor header must not reach the reader: {body}"
    );
    assert!(body.contains("   9 fn main() {"), "{body}");
    assert!(body.contains("  10     body()"), "{body}");
    Ok(())
}

#[test]
fn verbose_grep_prints_each_path_once() -> TestResult {
    let cell = tool_cell(
        "grep",
        "src/a.rs:3:one\nsrc/a.rs:7:two\nsrc/b.rs:9:three",
        false,
    );
    let lines = rendered(&cell, TranscriptMode::Verbose);
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.trim() == "src/a.rs")
            .count(),
        1,
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("   3 one")),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("   7 two")),
        "{lines:?}"
    );
    Ok(())
}

#[test]
fn cycling_the_transcript_mode_rewrites_what_is_already_on_screen() -> TestResult {
    let mut app = streamed("A paragraph.\n");
    let result = yi_types::event::ToolResult {
        content: vec![yi_types::message::Content::Text {
            text: "[a.rs#CF8C]\n1:one\n2:two".to_owned(),
            text_signature: None,
        }],
        details: serde_json::Value::Null,
        usage: None,
        added_tool_names: None,
        terminate: None,
    };
    app.reduce_agent(yi_types::event::AgentEvent::ToolExecutionEnd {
        tool_call_id: "t1".to_owned(),
        tool_name: "read".to_owned(),
        result,
        is_error: false,
    });
    // A read-only call is held until its run closes, so the turn has to end
    // before the cell is in the committed history this test reads.
    app.reduce_agent(yi_types::event::AgentEvent::AgentEnd {
        messages: Vec::new(),
    });
    let normal = app.reflowed(200).iter().map(flat).collect::<Vec<_>>();
    assert!(
        normal.iter().any(|line| line.contains("└ 2 lines")),
        "{normal:?}"
    );
    assert!(
        !normal.iter().any(|line| line.contains("one")),
        "{normal:?}"
    );

    // Ctrl+O must both change the mode and ask for the rows above the viewport
    // to be rebuilt; without the repaint request the cells already committed
    // keep the body they were drawn with and the toggle looks inert.
    let key = KeyInput::Single(SingleKey {
        code: KeyCodeValue::Char('o'),
        ctrl: true,
        alt: false,
        shift: false,
    });
    assert_eq!(
        default_keymap().resolve(&key, &EvalContext::default()),
        Some(Action::ToggleExpand),
        "ctrl-o must still be the mode toggle"
    );
    app.cycle_mode();
    app.cycle_mode();
    assert_eq!(app.mode(), TranscriptMode::Verbose);
    assert!(
        app.take_pending_repaint(),
        "the toggle must request a repaint"
    );

    let verbose = app.reflowed(200).iter().map(flat).collect::<Vec<_>>();
    assert!(
        verbose.iter().any(|line| line.contains("   1 one")),
        "{verbose:?}"
    );
    assert!(
        !verbose.iter().any(|line| line.contains("CF8C")),
        "{verbose:?}"
    );
    Ok(())
}

#[test]
fn the_reflow_debounce_rebuilds_once_at_the_settled_width() -> TestResult {
    use std::time::Instant;
    use yi_tui::reflow::{REFLOW_DEBOUNCE, ReflowState};

    let mut state = ReflowState::default();
    let now = Instant::now();

    // The first width observed has no old-width transcript to repair.
    let first = state.note_width(100);
    assert!(first.initialized && !first.changed);
    assert!(!state.pending_is_due(now));

    // A drag: three widths in quick succession, each pushing the deadline out,
    // so the rebuild runs once at the width the drag settled on.
    for width in [90u16, 80, 72] {
        assert!(state.note_width(width).changed);
        state.schedule_debounced(Some(width), now);
    }
    assert!(!state.pending_is_due(now + REFLOW_DEBOUNCE / 2));
    assert!(state.pending_is_due(now + REFLOW_DEBOUNCE));

    // While the rebuild is pending, that width does not need scheduling again.
    assert!(!state.reflow_needed_for_width(72));
    // Once the pending work is taken but not yet done, it does.
    state.clear_pending_reflow();
    assert!(state.reflow_needed_for_width(72));
    // Only the width that actually rebuilt counts as repaired.
    state.mark_reflowed_width(72);
    assert!(!state.reflow_needed_for_width(72));
    // A terminal that settles on its real size after the rebuild still gets one.
    assert!(state.reflow_needed_for_width(74));
    Ok(())
}

#[test]
fn a_resize_during_a_stream_forces_one_repair_after_it_settles() -> TestResult {
    use std::time::Instant;
    use yi_tui::reflow::ReflowState;

    let mut state = ReflowState::default();
    assert!(!state.take_stream_finish_needed());

    // A rebuild that ran mid-stream could only render the partial that existed
    // then, so the finished text has to be rebuilt once more.
    state.note_width(100);
    state.schedule_debounced(Some(80), Instant::now());
    state.mark_ran_during_stream();
    assert!(state.take_stream_finish_needed());
    assert!(
        !state.take_stream_finish_needed(),
        "draining: one episode forces at most one repair"
    );

    // The other half: the width changed while streaming but the debounce never
    // fired, so nothing rebuilt and the flag is the only record.
    state.mark_resize_requested_during_stream();
    assert!(state.take_stream_finish_needed());
    Ok(())
}

#[test]
fn the_row_cap_is_enforced_while_rendering_from_source() -> TestResult {
    use yi_tui::cell::TranscriptMode;
    use yi_tui::history::History;

    let mut history = History::default();
    for n in 1..=200 {
        history.retain(Cell::Notice {
            text: format!("notice {n}"),
        });
    }
    let theme = theme();
    let capped = History::replay(&history, 80, &theme, TranscriptMode::Normal, 20);
    assert!(
        capped.len() <= 21,
        "the cap bounds what is rendered, not what is written afterwards: {}",
        capped.len()
    );
    let text: Vec<String> = capped.iter().map(flat).collect();
    assert!(
        text.iter().any(|line| line.contains("notice 200")),
        "the newest rows are the ones kept: {text:?}"
    );
    assert!(
        !text.iter().any(|line| line.contains("notice 1 ")),
        "the oldest rows fall outside the cap: {text:?}"
    );
    Ok(())
}

/// The blanket per-iteration request made every turn draw at the 16 ms frame
/// ceiling — five frames per visible spinner step, four of them byte-identical.
/// Waking on the spinner's own boundary is what lets the request be dropped
/// without the glyph stepping unevenly: a fixed interval beats against the
/// 80 ms period and the step lands late by a drifting amount.
#[test]
fn the_turn_timer_wakes_on_the_spinner_boundary_not_a_fixed_interval() -> TestResult {
    use std::time::Duration;
    use yi_tui::app::next_spinner_wake;

    // Just past a step: nearly a whole period left.
    assert_eq!(next_spinner_wake(81), Duration::from_millis(79));
    // Just before the next: a sliver.
    assert_eq!(next_spinner_wake(159), Duration::from_millis(1));
    // Exactly on a boundary: a full period, never zero — a zero wake would spin
    // the loop instead of sleeping.
    assert_eq!(next_spinner_wake(160), Duration::from_millis(80));
    assert_eq!(next_spinner_wake(0), Duration::from_millis(80));

    // Every wake lands inside one period and none is zero, at any offset.
    for elapsed in 0u128..500 {
        let wake = next_spinner_wake(elapsed);
        assert!(
            wake > Duration::ZERO && wake <= Duration::from_millis(80),
            "elapsed {elapsed} produced {wake:?}"
        );
        // The wake must land exactly on a step boundary, or the glyph drifts.
        let landed = elapsed + wake.as_millis();
        assert_eq!(landed % 80, 0, "elapsed {elapsed} wakes off-boundary");
    }
    Ok(())
}
