use std::error::Error;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use yi_tui::colors::{ColorTier, Theme, detect_dark, detect_tier, name_accent};
use yi_tui::composer::{Composer, sanitize_paste};
use yi_tui::frame::FrameScheduler;
use yi_tui::hud::{BoardCard, CardKind, CardStatus, HudInput};
use yi_tui::keymap::{Action, EvalContext, KeyInput, default_keymap};
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
fn status_gauge_shows_percent_and_window() -> TestResult {
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
    assert!(text.contains("50%"), "gauge carries the percent: {text}");
    assert!(text.contains("128K"), "gauge carries the window: {text}");
    assert!(text.contains('┃'), "threshold tick rendered: {text}");
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
    assert!(text.iter().filter(|l| l.contains("```")).count() >= 2);
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
        text.iter().any(|l| l.contains("Tool") && l.contains("│")),
        "header row renders with column separators: {text:?}"
    );
    assert!(
        text.iter().any(|l| l.contains("┼")),
        "header rule renders: {text:?}"
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
