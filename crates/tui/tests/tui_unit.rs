mod common;

use std::error::Error;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use yi_tui::cell::{Cell, ToolCell, ToolStatus, TranscriptMode};
use yi_tui::colors::{ColorTier, Theme, detect_dark, detect_tier, name_accent};
use yi_tui::composer::{Composer, sanitize_paste};
use yi_tui::frame::FrameScheduler;
use yi_tui::keymap::{Action, EvalContext, KeyCodeValue, KeyInput, SingleKey, default_keymap};
use yi_tui::status::StatusInput;
use yi_tui::tree::{TreeFilter, TreeResult, TreeView};
use yi_tui::wrap::wrap_line;

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

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

fn composer_with_history(entries: &[&str]) -> Composer {
    let mut composer = Composer::default();
    for entry in entries {
        composer.set_text(entry);
        let _ = composer.take_submission();
    }
    composer
}

#[test]
fn reverse_search_does_not_preview_until_the_query_has_text() -> TestResult {
    let mut composer = composer_with_history(&["alpha", "beta"]);
    composer.set_text("draft");
    composer.begin_search();
    assert_eq!(composer.text(), "draft");
    assert_eq!(composer.search_title().as_deref(), Some("reverse-i-search"));
    Ok(())
}

#[test]
fn reverse_search_previews_the_newest_case_insensitive_hit() -> TestResult {
    let mut composer = composer_with_history(&["Alpha one", "beta"]);
    composer.set_text("draft");
    composer.begin_search();
    composer.handle_search_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Char('a'),
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    assert_eq!(composer.text(), "beta");
    composer.handle_search_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Char('l'),
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    assert_eq!(composer.text(), "Alpha one");
    assert_eq!(
        composer.search_title().as_deref(),
        Some("reverse-i-search: al")
    );
    Ok(())
}

#[test]
fn reverse_search_miss_and_escape_restore_the_draft() -> TestResult {
    let mut composer = composer_with_history(&["alpha"]);
    composer.set_text("draft");
    composer.begin_search();
    composer.handle_search_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Char('z'),
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    assert_eq!(composer.text(), "draft");
    assert_eq!(
        composer.search_title().as_deref(),
        Some("failing reverse-i-search: z")
    );
    assert!(composer.cancel_search());
    assert_eq!(composer.text(), "draft");
    assert!(!composer.search_active());
    Ok(())
}

#[test]
fn reverse_search_enter_accepts_without_submitting_and_skips_duplicate_text() -> TestResult {
    let mut composer = composer_with_history(&["alpha", "alphabet", "alpha"]);
    composer.set_text("draft");
    composer.begin_search();
    composer.handle_search_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Char('a'),
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    composer.handle_search_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Char('l'),
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    assert_eq!(composer.text(), "alpha");
    composer.search_older();
    assert_eq!(composer.text(), "alphabet");
    composer.search_older();
    assert_eq!(composer.text(), "alphabet");
    assert!(composer.accept_search());
    assert_eq!(composer.text(), "alphabet");
    assert!(!composer.search_active());
    Ok(())
}

#[test]
fn keymap_binds_ctrl_r_to_history_search() -> TestResult {
    let key = KeyInput::parse("ctrl-r")?;
    assert_eq!(
        default_keymap().resolve(&key, &EvalContext::default()),
        Some(Action::HistorySearch)
    );
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

#[test]
fn hud_shows_the_checklist_count_without_a_goal() -> TestResult {
    let input = yi_tui::hud::HudInput {
        plan: Some(yi_tui::hud::PlanProgress {
            done: 1,
            total: 3,
            running: Some("rebase".to_owned()),
        }),
        ..yi_tui::hud::HudInput::default()
    };
    let lines = yi_tui::hud::render(&input, &theme());
    let text: String = lines
        .iter()
        .flat_map(|line| line.spans.iter().map(|span| span.content.to_string()))
        .collect();
    assert!(text.contains("Plan 1/3 · now: rebase"), "{text}");
    Ok(())
}

#[test]
fn hud_shows_memory_saves_without_a_spine() -> TestResult {
    let input = yi_tui::hud::HudInput {
        memory: Some("saved 1".to_owned()),
        ..yi_tui::hud::HudInput::default()
    };
    let text: String = yi_tui::hud::render(&input, &theme())
        .iter()
        .flat_map(|line| line.spans.iter().map(|span| span.content.to_string()))
        .collect();
    assert!(text.contains("   Memory · saved 1"), "{text}");
    assert!(!text.contains('├') && !text.contains('└'), "{text}");
    Ok(())
}

#[test]
fn hud_empty_input_renders_nothing() -> TestResult {
    let lines = yi_tui::hud::render(&yi_tui::hud::HudInput::default(), &theme());
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
        ..StatusInput::default()
    };
    let row = yi_tui::status::render(&input, 40, &theme());
    let text = flat(&row);
    assert!(text.contains("faux-1"), "model survives: {text}");
    assert!(text.contains("…"), "path shrank by middle-ellipsis: {text}");
    Ok(())
}

/// Incident: `@branch` was appended after the cwd had been shrunk, so a long worktree path
/// plus a long branch held a 60-column segment the cascade kept while it evicted the model.
#[test]
fn status_keeps_the_model_under_a_long_path_and_branch() -> TestResult {
    let input = StatusInput {
        model: "faux-1".to_owned(),
        cwd: "/Users/someone/Development/yi/.claude/worktrees/feature-4567".to_owned(),
        branch: Some("claude/decoupled-deceleration-text-streaming-466ff9".to_owned()),
        session_name: "01a06f8d".to_owned(),
        context_used: 0,
        context_window: 128_000,
        ..StatusInput::default()
    };
    assert_eq!(input.cwd.chars().count(), 60);
    assert_eq!(input.branch.as_deref().map(str::len), Some(51));
    let text = flat(&yi_tui::status::render(&input, 80, &theme()));
    assert!(text.contains("faux-1"), "model survives: {text}");
    assert!(text.contains("@…"), "branch shrinks from the left: {text}");
    assert!(text.contains("01a06f8d"), "session id survives: {text}");
    Ok(())
}

/// A lane session's row names the checkout and the slot, not the pool's hash path.
#[test]
fn status_names_the_lane_instead_of_the_slot_path() -> TestResult {
    let input = StatusInput {
        model: "faux-1".to_owned(),
        cwd: "/Users/someone/.yi/lanes/897d6e9162485667/1".to_owned(),
        lane: Some("yi ⎇ lane 1".to_owned()),
        branch: Some("yi/01a06f8d-1234".to_owned()),
        session_name: "01a06f8d".to_owned(),
        context_window: 128_000,
        ..StatusInput::default()
    };
    let text = flat(&yi_tui::status::render(&input, 80, &theme()));
    assert!(text.contains("yi ⎇ lane 1"), "{text}");
    assert!(
        !text.contains("897d6e9162485667"),
        "the hash stays off the row: {text}"
    );
    assert!(
        !text.contains("@yi/"),
        "the branch repeats the session id: {text}"
    );
    Ok(())
}

/// A landing older than the poll period carries its age; a fresh one does not.
#[test]
fn landing_segment_ages_once_the_poll_may_have_stopped() {
    use yi_types::lane::{JobState, Landing, LandingJob, PrNumber};
    let landing = Landing::Open {
        pr: PrNumber(191),
        jobs: vec![LandingJob {
            name: "lint".to_owned(),
            state: JobState::Green,
        }],
        behind: 4,
    };
    let fresh = yi_tui::status::landing_segment(&landing, Some(std::time::Duration::from_secs(10)));
    assert_eq!(fresh.as_deref(), Some("PR #191 ● · main +4"));
    let old =
        yi_tui::status::landing_segment(&landing, Some(std::time::Duration::from_secs(3 * 3600)));
    assert_eq!(old.as_deref(), Some("PR #191 ● · main +4 · 3 h ago"));
    assert_eq!(
        yi_tui::status::landing_segment(&Landing::Unlanded, None),
        None
    );
}

#[test]
fn status_context_segment_is_compact() -> TestResult {
    let input = StatusInput {
        model: "m".to_owned(),
        cwd: "/p".to_owned(),
        session_name: "s".to_owned(),
        context_used: 64_000,
        context_window: 128_000,
        ..StatusInput::default()
    };
    let row = yi_tui::status::render(&input, 80, &theme());
    let text = flat(&row);
    assert!(
        text.contains("64,000 / 128K"),
        "compact context segment: {text}"
    );
    let wide = StatusInput {
        context_window: 1_048_576,
        ..input
    };
    let text = flat(&yi_tui::status::render(&wide, 80, &theme()));
    assert!(
        text.contains("/ 1M"),
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
        default_keymap(),
        80,
    );
    let assistant = |text: &str| {
        yi_runtime::faux::faux_assistant_message(
            vec![yi_runtime::faux::faux_text(text)],
            yi_types::message::StopReason::Stop,
        )
    };
    app.reduce_agent(yi_types::event::AgentEvent::AgentStart);
    let mut end = 8;
    while end < full.len() {
        while !full.is_char_boundary(end.min(full.len())) {
            end += 1;
        }
        let message = assistant(full.get(..end.min(full.len())).unwrap_or(full));
        app.reduce_agent(yi_types::event::AgentEvent::MessageUpdate {
            // A delta is a delta (D145): a test hands the reducer a whole message as `Done`.
            assistant_message_event: yi_types::event::AssistantMessageEvent::Done {
                reason: yi_types::message::StopReason::Stop,
                message,
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
        message: yi_types::message::AgentMessage::host_user(
            yi_types::message::UserContent::Text(text.to_owned()),
            0,
        ),
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

/// A4: a bare fence still opens the rail, so its body has a header to hang off
/// instead of a rail starting mid-air. An indented block gets none: a stream
/// cuts it at a blank line and cannot reopen it, so a header would repaint.
#[test]
fn a_bare_fence_opens_the_rail_and_an_indented_block_does_not() -> TestResult {
    let theme = theme();
    for (source, expected) in [
        ("```\nplain body\n```", vec!["│", "│ plain body"]),
        ("    indented body\n", vec!["│ indented body"]),
    ] {
        let text: Vec<String> = yi_tui::markdown::render(source, 60, &theme)
            .iter()
            .map(flat)
            .filter(|line| !line.is_empty())
            .collect();
        assert_eq!(text, expected, "{source:?}");
    }
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
fn fenced_code_body_reads_bright_while_the_rail_stays_dim() -> TestResult {
    for dark in [true, false] {
        let theme = Theme::new(ColorTier::TrueColor, dark);
        let lines = yi_tui::markdown::render("```rust\nlet x = 1;\n```", 60, &theme);
        let body = lines
            .iter()
            .find(|l| flat(l).contains("let x = 1;"))
            .ok_or("missing fenced body")?;
        let rail = body.spans.first().ok_or("missing rail span")?;
        assert_eq!(
            rail.style.fg,
            Some(theme.dim),
            "rail stays dim (dark={dark})"
        );
        let gap = body
            .spans
            .iter()
            .find(|s| s.content.contains('x'))
            .ok_or("missing untokenized span")?;
        assert_eq!(
            gap.style.fg,
            Some(theme.text),
            "code syntect leaves untokenized is the payload, not chrome (dark={dark})"
        );
        let keyword = body
            .spans
            .iter()
            .find(|s| s.content.contains("let"))
            .ok_or("missing keyword span")?;
        assert_eq!(keyword.style.fg, Some(theme.magenta), "dark={dark}");
        assert!(keyword.style.add_modifier.contains(Modifier::BOLD));
    }
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
        text.iter().any(|l| l.contains("weighing the options"))
            && !text.iter().any(|l| l.contains('∴')),
        "reasoning is its own style, not a glyph: {text:?}"
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
fn stable_stream_stops_at_blank_lines_outside_fences() -> TestResult {
    use yi_tui::markdown::stable_stream;
    let stream = stable_stream("para one\n\npara two streaming");
    assert_eq!(stream.cut, "para one\n\n".len());
    assert!(stream.reopen.is_none());
    assert_eq!(stable_stream("no boundary yet").cut, 0);
    Ok(())
}

/// P10: a top-level fence streams line by line — each completed code row is
/// stable, carries the fence's opening line for standalone rendering, and
/// the close releases the fence.
#[test]
fn stable_stream_commits_fence_interiors_line_by_line() -> TestResult {
    use yi_tui::markdown::stable_stream;
    let streaming = "```rust\nlet a = 1;\nlet b = ";
    let stream = stable_stream(streaming);
    assert_eq!(stream.cut, "```rust\nlet a = 1;\n".len());
    assert_eq!(stream.reopen.as_deref(), Some("```rust"));
    let closed = "```rust\nlet a = 1;\n```\n";
    let stream = stable_stream(closed);
    assert_eq!(stream.cut, closed.len());
    assert!(stream.reopen.is_none());
    // An indented fence (list item) stays opaque: no cut inside it.
    let listed = "- item\n  ```\n  code\n";
    assert_eq!(stable_stream(listed).cut, 0);
    Ok(())
}

/// P10: streaming a fenced block one line at a time must paint what the
/// whole message paints. The rail header belongs to the slice that opened the
/// block; a continuation slice reopens the fence without redrawing it.
#[test]
fn streamed_fence_paints_the_language_rail_once() -> TestResult {
    use yi_tui::markdown::{render, render_stream, stable_stream};
    let whole = "intro\n\n```rust\nlet a = 1;\nlet b = 2;\n```\n";
    let mut cut = 0;
    let mut reopen: Option<String> = None;
    let mut lang = None;
    let mut painted: Vec<String> = Vec::new();
    for end in 1..=whole.len() {
        let Some(source) = whole.get(..end) else {
            continue;
        };
        let stream = stable_stream(source);
        if stream.cut <= cut {
            continue;
        }
        let slice = source.get(cut..stream.cut).unwrap_or_default();
        let (text, continued) = match &reopen {
            Some(open) => (format!("{open}\n{slice}"), true),
            None => (slice.to_owned(), false),
        };
        let lines = render_stream(&text, 60, &theme(), continued, &mut lang);
        painted.extend(lines.iter().map(flat));
        (cut, reopen) = (stream.cut, stream.reopen);
    }
    // Blank separators are the caller's (commit_stable_prefix pushes one per
    // non-continuing slice), so the comparison is over the content rows.
    let batch: Vec<String> = render(whole, 60, &theme())
        .iter()
        .map(flat)
        .filter(|line| !line.is_empty())
        .collect();
    painted.retain(|line| !line.is_empty());
    assert_eq!(
        painted, batch,
        "streamed paint diverged from the batch paint"
    );
    assert_eq!(
        painted.iter().filter(|line| line.contains("rust")).count(),
        1,
        "the language rail is drawn once: {painted:?}"
    );
    Ok(())
}

/// A triple-quoted string spanning a commit seam stays one string: the parse
/// state rides the seam instead of the fence being re-lexed from its reopen.
#[test]
fn a_streamed_fence_keeps_string_colour_across_the_seam() -> TestResult {
    let whole = r#"```python
def f():
    s = """
    def not_real(x, y):
        return x + y
    """
    return s
```
"#;
    let mut app = streamed(whole);
    let committed = app.take_commits();
    let row = committed
        .iter()
        .find(|line| flat(line).contains("return x + y"))
        .ok_or("no committed row for the string body")?;
    let body: Vec<_> = row
        .spans
        .iter()
        .skip_while(|span| !span.content.contains("return"))
        .map(|span| (span.content.as_ref(), span.style.fg))
        .collect();
    let string_fg = theme().syntax_style(yi_tui::highlight::Token::Str).fg;
    assert_eq!(
        body,
        vec![("        return x + y", string_fg)],
        "the string body was re-lexed as code or lost its colour: {row:?}"
    );
    Ok(())
}

/// Fences are tracked by marker and run length: `~~~` closes only on `~~~`,
/// and a four-backtick fence swallows the ``` example inside it.
#[test]
fn stable_stream_matches_fence_markers_exactly() -> TestResult {
    use yi_tui::markdown::stable_stream;
    let tilde = "~~~\ncode\n~~~\nafter\n\n";
    let stream = stable_stream(tilde);
    assert_eq!(stream.cut, tilde.len());
    let nested = "````md\n```\ninner\n```\n";
    let stream = stable_stream(nested);
    assert_eq!(stream.cut, nested.len());
    assert_eq!(stream.reopen.as_deref(), Some("````md"));
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
            // A delta is a delta (D145): a test hands the reducer a whole message as `Done`.
            assistant_message_event: yi_types::event::AssistantMessageEvent::Done {
                reason: yi_types::message::StopReason::Stop,
                message,
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
fn tree_panel_matches_the_reference_layout() -> TestResult {
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
fn a_query_that_hides_the_selection_moves_it_to_a_visible_row() -> TestResult {
    let entries = vec![
        entry("u1", None, 1, "alpha ask"),
        assistant_entry("a1", Some("u1"), 2, "alpha answer"),
        entry("u2", Some("a1"), 3, "beta ask"),
    ];
    let mut view = TreeView::new(&entries, Some("u2"), TreeFilter::Default);
    for c in "alpha".chars() {
        view.handle_key(&yi_tui::keymap::SingleKey {
            code: yi_tui::keymap::KeyCodeValue::Char(c),
            ctrl: false,
            alt: false,
            shift: false,
        });
    }
    let enter = yi_tui::keymap::SingleKey::parse("enter")?;
    match view.handle_key(&enter) {
        TreeResult::Rewind(id) => assert_ne!(id, "u2", "enter acted on a row the query had hidden"),
        _ => return Err("enter must rewind".into()),
    }
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
        vec!["‣ alpha".to_owned(), String::new(), "‣ beta".to_owned()],
        "a loose list keeps its author's breathing room"
    );
    let tight: Vec<String> = yi_tui::markdown::render("- alpha\n- beta\n", 60, &theme)
        .iter()
        .map(flat)
        .collect();
    assert_eq!(
        tight,
        vec!["‣ alpha".to_owned(), "‣ beta".to_owned()],
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
        call_id: String::new(),
        intent: None,
        status: if failed {
            ToolStatus::Failed
        } else {
            ToolStatus::Done
        },
        summary: ToolCell::summary_of(name, ""),
        digest: ToolCell::digest_of(name, result, failed),
        preview: result.lines().map(str::to_owned).collect(),
        elapsed_ms: 0,
        calls: 1,
        details: serde_json::Value::Null,
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
            lines.iter().any(|line| line.contains(expected)),
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
            .filter(|line| line.trim_matches(['│', ' ']) == "src/a.rs")
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

/// Feed `thought` and then `prose` through the streaming path the way a
/// reasoning model delivers them, and hand back the app that rendered them.
/// `finish` false stops before `MessageEnd`, which is where a turn commits
/// whatever it was still holding — the one place mid-stream loss is visible.
fn streamed_thought(thought: &str, prose: &str, finish: bool) -> yi_tui::app::App {
    streamed_thought_in(TranscriptMode::Thinking, thought, prose, finish)
}

/// The same stream under `mode`, reached by cycling from the default the way a
/// reader does, so the mode-specific commit paths are the ones exercised.
fn streamed_thought_in(
    mode: TranscriptMode,
    thought: &str,
    prose: &str,
    finish: bool,
) -> yi_tui::app::App {
    use yi_tui::app::{App, TuiOptions};
    use yi_tui::keymap::default_keymap;
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
        default_keymap(),
        80,
    );
    while app.mode() != mode {
        app.cycle_mode();
    }
    let message = |thinking: &str, text: &str| {
        let mut content = vec![yi_runtime::faux::faux_thinking(thinking)];
        if !text.is_empty() {
            content.push(yi_runtime::faux::faux_text(text));
        }
        yi_runtime::faux::faux_assistant_message(content, yi_types::message::StopReason::Stop)
    };
    let update =
        |partial: yi_types::message::AgentMessage| yi_types::event::AgentEvent::MessageUpdate {
            // A delta is a delta (D145): a test hands the reducer a whole message as `Done`.
            assistant_message_event: yi_types::event::AssistantMessageEvent::Done {
                reason: yi_types::message::StopReason::Stop,
                message: partial,
            },
        };
    app.reduce_agent(yi_types::event::AgentEvent::AgentStart);
    let mut cut = 8;
    while cut < thought.len() {
        while !thought.is_char_boundary(cut.min(thought.len())) {
            cut += 1;
        }
        let partial = message(thought.get(..cut.min(thought.len())).unwrap_or(thought), "");
        app.reduce_agent(update(partial));
        cut += 17;
    }
    app.reduce_agent(update(message(thought, prose)));
    if finish {
        app.reduce_agent(yi_types::event::AgentEvent::MessageEnd {
            message: message(thought, prose),
        });
    }
    app
}

#[test]
fn a_long_thought_reaches_scrollback_and_outlives_the_prose_that_follows() -> TestResult {
    // Three paragraphs so the head is well past the live region's tail, which
    // keeps only half a screen and drops everything above it.
    let thought = "First step of it.\n\nSecond step of it.\n\nThird step of it.\n";
    // Two paragraphs of prose so the answer's own stable prefix commits
    // mid-stream: a thought that waits for the end of the turn lands under it.
    let mut app = streamed_thought(thought, "The answer.\n\nAnd the rest of it.\n", true);
    let committed: Vec<String> = app.take_commits().iter().map(flat).collect();
    for step in ["First step", "Second step", "Third step"] {
        assert!(
            committed.iter().any(|line| line.contains(step)),
            "every paragraph of the thought commits, not just the tail: {committed:?}"
        );
    }
    assert_eq!(
        committed
            .iter()
            .filter(|line| line.contains("First step"))
            .count(),
        1,
        "the thought commits once, not once per paragraph: {committed:?}"
    );
    let label = committed
        .iter()
        .position(|line| line.contains("First step"))
        .ok_or("no thought row")?;
    let answer = committed
        .iter()
        .position(|line| line.contains("The answer."))
        .ok_or("no prose")?;
    assert!(
        label < answer,
        "reasoning lands above the prose it preceded: {committed:?}"
    );
    Ok(())
}

/// The default shape a model produces: one short paragraph with no newline, so
/// no stable cut ever fires for it. Two prose paragraphs make the answer's own
/// first paragraph commit mid-stream — a thought that waits for `MessageEnd`
/// lands under the answer it preceded.
#[test]
fn a_short_unterminated_thought_still_lands_above_the_answer() -> TestResult {
    let mut app = streamed_thought("I should just say ok.", "ok.\n\nThat is all.\n", true);
    let committed: Vec<String> = app.take_commits().iter().map(flat).collect();
    let label = committed
        .iter()
        .position(|line| line.contains("I should just say ok."))
        .ok_or("no thought row")?;
    let answer = committed
        .iter()
        .position(|line| line.contains("ok.") && !line.contains("say ok."))
        .ok_or("no prose")?;
    assert!(
        label < answer,
        "a one-line thought lands above the prose it preceded: {committed:?}"
    );
    assert_eq!(
        committed
            .iter()
            .filter(|line| line.contains("I should just say ok."))
            .count(),
        1,
        "the thought commits once: {committed:?}"
    );
    Ok(())
}

/// `normal` never commits a thought on its own cuts, so the whole of it used to
/// wait for `MessageEnd` — and its count row followed the answer.
#[test]
fn a_short_unterminated_thought_still_lands_above_the_answer_in_normal_mode() -> TestResult {
    let mut app = streamed_thought_in(
        TranscriptMode::Normal,
        "I should just say ok.",
        "ok.\n\nThat is all.\n",
        true,
    );
    let committed: Vec<String> = app.take_commits().iter().map(flat).collect();
    let label = committed
        .iter()
        .position(|line| line.contains("thought · 1 line"))
        .ok_or_else(|| format!("no one-line count row: {committed:?}"))?;
    let answer = committed
        .iter()
        .position(|line| line.contains("ok."))
        .ok_or("no prose")?;
    assert!(
        label < answer,
        "the count row lands above the prose it preceded: {committed:?}"
    );
    Ok(())
}

/// M3 (D107): every `Cell` family that commits to scrollback names the pair
/// test where its dependent finishes first, or says it has none. The match is
/// exhaustive, so a new variant fails to compile until it does.
fn antecedent(cell: &Cell) -> Option<&'static str> {
    match cell {
        Cell::Assistant { .. } => Some("a_short_unterminated_thought_still_lands_above_the_answer"),
        Cell::Task(_) => Some("a_child_that_finishes_inside_its_spawning_cell_lands_under_it"),
        Cell::User { .. }
        | Cell::Thought { .. }
        | Cell::Tool(_)
        | Cell::Explored(_)
        | Cell::Advisory { .. }
        | Cell::Notice { .. }
        | Cell::Footer { .. }
        | Cell::Rule { .. }
        | Cell::Divider => None,
    }
}

#[test]
fn every_cell_family_names_its_antecedent() -> TestResult {
    let prose = Cell::Assistant {
        markdown: String::new(),
    };
    assert!(
        antecedent(&prose).is_some_and(|name| name.contains("thought")),
        "prose depends on the thought before it"
    );
    assert_eq!(antecedent(&Cell::Divider), None);
    Ok(())
}

#[test]
fn one_unbroken_paragraph_still_reaches_scrollback() -> TestResult {
    // No blank line anywhere, so `stable_cut` never fires and every row past
    // the live tail used to be dropped with nothing scrollable behind it.
    let thought = "one two three four five six seven eight nine ten ".repeat(40);
    let prose = "alpha bravo charlie delta echo foxtrot golf hotel india ".repeat(40);
    let mut app = streamed_thought(&thought, &prose, false);
    let committed: Vec<String> = app.take_commits().iter().map(flat).collect();
    let rows = committed.len();
    assert!(
        rows > yi_tui::render::live_tail_rows(24),
        "more than the live region can hold reached scrollback: {rows} rows"
    );
    for line in &committed {
        assert!(
            !line.contains("onetwo") && !line.contains("alphabravo"),
            "a forced cut lands on a word boundary: {line:?}"
        );
    }
    // A short body row is a seam showing through, unless it is the thought's own
    // last row: the forced cut snaps to the last word of the row it lands on.
    let short: Vec<&String> = committed
        .windows(2)
        .filter(|pair| {
            pair.iter()
                .all(|l| l.trim_start().starts_with(char::is_alphabetic))
        })
        .filter_map(|pair| pair.first())
        .filter(|line| line.chars().count() < 60)
        .collect();
    assert!(short.is_empty(), "the seams wrap flush: {short:?}");
    // The forced cut suppresses the blank line between its own slices, but the
    // one that opens the answer is a block break and keeps its air.
    let opens = committed
        .iter()
        .position(|line| line.starts_with('\u{2022}'))
        .ok_or("no prose block")?;
    assert_eq!(
        committed.get(opens.wrapping_sub(1)).map(String::as_str),
        Some(""),
        "the answer opens under a blank line: {:?}",
        committed.get(opens.saturating_sub(2)..=opens)
    );
    Ok(())
}

#[test]
fn normal_mode_still_collapses_a_thought_to_its_line_count() -> TestResult {
    let mut app = streamed_thought("One.\n\nTwo.\n\nThree.\n", "The answer.\n", true);
    app.cycle_mode();
    app.cycle_mode();
    assert_eq!(app.mode(), TranscriptMode::Normal);
    let lines: Vec<String> = app.reflowed(200).iter().map(flat).collect();
    assert!(
        lines.iter().any(|line| line.contains("thought · 5 lines")),
        "the count covers the whole thought, not its last slice: {lines:?}"
    );
    assert!(
        !lines.iter().any(|line| line.contains("Two.")),
        "normal keeps the body folded away: {lines:?}"
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
    let shown = app.reflowed(200).iter().map(flat).collect::<Vec<_>>();
    assert!(
        shown.iter().any(|line| line.contains("2 lines")),
        "{shown:?}"
    );
    // The default view carries the first rows of a result; `normal` is the fold.
    assert!(
        shown.iter().any(|line| line.contains("   1 one")),
        "{shown:?}"
    );
    app.cycle_mode();
    app.cycle_mode();
    assert_eq!(app.mode(), TranscriptMode::Normal);
    let normal = app.reflowed(200).iter().map(flat).collect::<Vec<_>>();
    assert!(
        !normal.iter().any(|line| line.contains("one")),
        "normal keeps the body folded away: {normal:?}"
    );
    app.cycle_mode();

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
    assert_eq!(
        app.mode(),
        TranscriptMode::Thinking,
        "reasoning is on by default; normal is what a reader opts into"
    );
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

/// The still image a reviewer sees is rendered from this dump, so it has to
/// carry the styling — a frame that serializes to bare text proves nothing
/// about how the UI looks.
#[test]
fn a_frame_dump_carries_style_not_just_text() -> TestResult {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Color;

    let mut buffer = Buffer::empty(Rect::new(0, 0, 6, 2));
    buffer.set_string(
        0,
        0,
        "yi",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );
    buffer.set_string(0, 1, "ok", Style::default().fg(Color::Red));
    let ansi = String::from_utf8(yi_tui::capture::buffer_to_ansi(&buffer)?)?;

    assert!(
        ansi.starts_with("\x1b[0m\x1b[H"),
        "the dump must open from a clean style at the origin: {ansi:?}"
    );
    // Rows are placed, never newline-terminated: 24 newlines on a 24-row
    // screen scroll the frame off itself and the still renders blank.
    assert!(
        !ansi.contains('\n'),
        "a placed frame carries no newlines: {ansi:?}"
    );
    assert!(
        ansi.contains("\x1b[2;1H"),
        "the second row is placed, not flowed: {ansi:?}"
    );
    assert!(ansi.contains("yi") && ansi.contains("ok"), "{ansi:?}");
    assert!(
        ansi.contains("\x1b[1m"),
        "bold must survive the dump: {ansi:?}"
    );
    // Crossterm writes the ANSI palette in indexed form: cyan is 6, red 1.
    assert!(
        ansi.contains("\x1b[38;5;6") && ansi.contains("\x1b[38;5;1"),
        "both foreground colours must reach the dump: {ansi:?}"
    );
    Ok(())
}

fn cast_payloads(path: &std::path::Path) -> Result<Vec<String>, Box<dyn Error>> {
    let text = std::fs::read_to_string(path)?;
    let mut payloads = Vec::new();
    for line in text.lines().skip(1) {
        let event: serde_json::Value = serde_json::from_str(line)?;
        payloads.push(event[2].as_str().unwrap_or_default().to_owned());
    }
    Ok(payloads)
}

/// asciicast payloads are JSON strings, and crossterm hands the writer
/// arbitrary byte chunks — a box-drawing character split across two of them
/// must reassemble rather than corrupt the line or vanish.
#[test]
fn a_character_split_across_writes_survives_the_cast() -> TestResult {
    use std::io::Write;

    let dir = Scratch::new("yi-cast-utf8")?;
    let path = dir.join("run.cast");
    let mut cast = yi_tui::capture::CastWriter::create(&path, 80, 24)?;
    // "─" is e2 94 80: two bytes now, the third only after a flush.
    cast.write_all(b"a\xe2\x94")?;
    cast.flush()?;
    let first = cast_payloads(&path)?;
    assert_eq!(
        first,
        vec!["a".to_owned()],
        "the partial character must wait"
    );

    cast.write_all(b"\x80b")?;
    cast.flush()?;
    let second = cast_payloads(&path)?;
    assert_eq!(
        second.concat(),
        "a─b",
        "the completed character belongs to the next event: {second:?}"
    );

    // A byte that can never complete is dropped, not held forever.
    cast.write_all(b"\xffok")?;
    cast.flush()?;
    let third = cast_payloads(&path)?;
    assert_eq!(
        third.concat(),
        "a─bok",
        "invalid bytes stall nothing: {third:?}"
    );
    Ok(())
}

/// The drive loop redraws every couple of milliseconds. Every call that
/// carries no change — an empty diff, a cursor already hidden, the
/// synchronized-update bracket — must leave the recording untouched, or a
/// minute of idling buries the frames a reviewer came for.
#[test]
fn an_unchanged_frame_records_nothing() -> TestResult {
    use ratatui::backend::Backend;
    use std::io::Write;

    let dir = Scratch::new("yi-cast-idle")?;
    let path = dir.join("run.cast");
    let mut backend = yi_tui::capture::RecordingBackend::new(80, 24, Some(&path))?;
    // Exactly what `render::draw` does on a tick that changed nothing.
    let idle_tick = |backend: &mut yi_tui::capture::RecordingBackend| -> TestResult {
        backend.write_all(b"\x1b[?2026h")?;
        Write::flush(backend)?;
        backend.draw(std::iter::empty())?;
        backend.hide_cursor()?;
        Backend::flush(backend)?;
        backend.write_all(b"\x1b[?2026l")?;
        Write::flush(backend)?;
        Ok(())
    };
    // The first tick hides the cursor for real, so it earns one event.
    idle_tick(&mut backend)?;
    let first = cast_payloads(&path)?;
    assert_eq!(first, vec!["\x1b[?25l".to_owned()], "{first:?}");

    for _ in 0..200 {
        idle_tick(&mut backend)?;
    }
    let idle = cast_payloads(&path)?;
    assert_eq!(idle, first, "200 more idle ticks wrote {:?}", &idle[1..]);

    // One real change still lands, so the filter is not simply off.
    let mut cell = ratatui::buffer::Cell::default();
    cell.set_symbol("x");
    backend.draw(std::iter::once((0, 0, &cell)))?;
    Backend::flush(&mut backend)?;
    let changed = cast_payloads(&path)?;
    assert_eq!(
        changed.len(),
        first.len() + 1,
        "one changed frame, one event: {changed:?}"
    );
    assert!(
        changed.last().is_some_and(|last| last.contains('x')),
        "{changed:?}"
    );
    Ok(())
}

/// `RecordingBackend` replaced `HeadlessBackend`, and a second drive loop
/// (yi-console) constructs its screen through ratatui's own `Terminal`
/// rather than Yi's. Recording off, it must still be a drop-in there.
#[test]
fn the_recording_backend_drops_into_a_plain_ratatui_terminal() -> TestResult {
    let backend = yi_tui::capture::RecordingBackend::new(80, 24, None)?;
    let mut terminal = ratatui::Terminal::new(backend)?;
    terminal.draw(|frame| {
        frame.render_widget(ratatui::widgets::Paragraph::new("console"), frame.area());
    })?;
    assert!(
        terminal.backend().screen().contains("console"),
        "the screen accessor replaces the old tuple field: {}",
        terminal.backend().screen()
    );
    Ok(())
}

/// Incident: `/plantree` shadowed `/plan` the moment it entered the table —
/// the popup preserved table order, so typing a whole command's name and
/// pressing Enter ran the longer one that merely started with it.
#[test]
fn typing_a_commands_whole_name_selects_it_over_a_longer_one()
-> Result<(), Box<dyn std::error::Error>> {
    use yi_tui::popup::ListPopup;
    let mut popup = ListPopup::new(
        '/',
        vec![
            "plantree".to_owned(),
            "plan".to_owned(),
            "planner".to_owned(),
        ],
    );
    popup.query = "plan".to_owned();
    let first = popup
        .filtered()
        .first()
        .map(|item| (*item).clone())
        .ok_or("the query matches three commands")?;
    assert_eq!(first, "plan");
    popup.query = "plant".to_owned();
    let narrowed: Vec<String> = popup.filtered().into_iter().cloned().collect();
    assert_eq!(narrowed, vec!["plantree".to_owned()]);
    Ok(())
}

/// Incident: a child error landing on a non-ASCII byte 80 panicked the render
/// thread — `String::truncate` cuts at a byte index, not a char boundary.
#[test]
fn a_multibyte_error_never_panics_the_task_cell() -> TestResult {
    use yi_tui::cell::{TaskCell, TaskStatus};
    let cell = TaskCell {
        child_id: "c1".to_owned(),
        description: "trace".to_owned(),
        status: TaskStatus::Failed,
        last_tool: None,
        toolcalls: 0,
        tokens: 0,
        elapsed_ms: 0,
        error: Some("€".repeat(100)),
        spawn: None,
        answer: None,
        activity: yi_types::subagent::ChildActivity::Waiting,
        flag: None,
    };
    let lines = cell.lines(120, &theme(), yi_tui::cell::TranscriptMode::Normal, 0);
    let joined: String = lines.iter().map(flat).collect();
    let kept = joined.chars().filter(|c| *c == '€').count();
    assert_eq!(kept, 80, "80 chars should survive in the card: {joined:?}");
    Ok(())
}

/// Incident: the same class in the popup's list item — a string truncated to
/// a terminal width must never cut mid-character.
#[test]
fn a_multibyte_popup_item_never_panics() -> TestResult {
    use yi_tui::popup::{BottomView, ListPopup};
    let popup = ListPopup::new('/', vec!["€".repeat(200)]);
    for width in 5..60 {
        let _ = popup.lines(width, &theme());
    }
    Ok(())
}

/// A bold paragraph wore the heading's colour, a third-level heading matched the second, and
/// every code span was one orange: weight is emphasis, hue is level, and a path reads as one.
#[test]
fn emphasis_is_weight_and_a_path_in_code_takes_the_read_hue() -> TestResult {
    let theme = theme();
    let lines = yi_tui::markdown::render(
        "# Title\n\n**Yi is bold.** See `crates/x.rs` and `foo()`.\n\n### Third\n\n- one\n",
        60,
        &theme,
    );
    let span = |needle: &str| {
        lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.content.contains(needle))
            .cloned()
            .ok_or_else(|| format!("no span {needle:?}"))
    };
    let bold = span("Yi is bold.")?;
    assert!(bold.style.add_modifier.contains(Modifier::BOLD));
    assert_ne!(
        bold.style.fg,
        Some(theme.accent),
        "bold prose is not heading-coloured"
    );
    let title_at = lines
        .iter()
        .position(|l| flat(l).contains("Title"))
        .ok_or("title")?;
    assert!(
        flat(&lines[title_at + 1]).starts_with('━'),
        "a rule under the top heading: {:?}",
        flat(&lines[title_at + 1])
    );
    assert_eq!(span("Third")?.style.fg, Some(theme.text));
    assert_eq!(span("crates/x.rs")?.style.fg, Some(theme.cyan));
    assert_eq!(span("foo()")?.style.fg, Some(theme.orange));
    assert!(
        lines
            .iter()
            .any(|l| flat(l).trim_start().starts_with("‣ one")),
        "the list marker is not the assistant gutter"
    );
    Ok(())
}

/// Each cell padded its own seam, so blocks met across two or three empty rows.
#[test]
fn blocks_meet_across_one_blank_row_on_replay() -> TestResult {
    use yi_tui::history::History;

    let mut history = History::default();
    history.retain(Cell::Assistant {
        markdown: "A paragraph.\n".to_owned(),
    });
    history.retain(Cell::Thought {
        markdown: "a thought".to_owned(),
    });
    history.retain(Cell::Assistant {
        markdown: "# Heading\n\nMore.\n".to_owned(),
    });
    let rows: Vec<String> = History::replay(&history, 80, &theme(), TranscriptMode::Thinking, 100)
        .iter()
        .map(flat)
        .collect();
    let mut run = 0;
    for row in &rows {
        run = if row.trim().is_empty() { run + 1 } else { 0 };
        assert!(run <= 1, "two blank rows in a row: {rows:?}");
    }
    Ok(())
}

/// Incident: a table taller than the live region was force-cut through its body while it
/// streamed, and every row below the cut reached scrollback as raw pipes.
#[test]
fn a_streaming_table_is_never_cut_through_its_body() -> TestResult {
    let mut source = "Here it is.\n\n| crate | role |\n| --- | --- |\n".to_owned();
    for index in 0..30 {
        source.push_str(&format!("| crate-{index} | role {index} |\n"));
    }
    let mut app = streamed(&source);
    let committed: Vec<String> = app.take_commits().iter().map(flat).collect();
    assert!(
        committed.iter().any(|line| line.contains("crate-29")),
        "{committed:?}"
    );
    assert!(
        !committed.iter().any(|line| line.contains("| ")),
        "no row reached scrollback as raw pipes: {committed:?}"
    );
    Ok(())
}

fn read_result(text: &str) -> yi_types::event::ToolResult {
    yi_types::event::ToolResult {
        content: vec![yi_types::message::Content::Text {
            text: text.to_owned(),
            text_signature: None,
        }],
        details: serde_json::Value::Null,
        usage: None,
        added_tool_names: None,
        terminate: None,
    }
}

/// Incident: a run of reads was held back until something else was said, and reasoning
/// committed without saying it, so the calls landed under the thought that followed them.
#[test]
fn a_run_of_reads_commits_above_the_reasoning_that_follows_it() -> TestResult {
    let mut app = streamed("Looking.\n");
    for id in ["t1", "t2"] {
        app.reduce_agent(yi_types::event::AgentEvent::ToolExecutionEnd {
            tool_call_id: id.to_owned(),
            tool_name: "read".to_owned(),
            result: read_result("[a.rs#CF8C]\n1:one"),
            is_error: false,
        });
    }
    let message = yi_runtime::faux::faux_assistant_message(
        vec![yi_runtime::faux::faux_thinking(
            "Pondering the reads.\n\nStill pondering.\n",
        )],
        yi_types::message::StopReason::Stop,
    );
    app.reduce_agent(yi_types::event::AgentEvent::MessageUpdate {
        assistant_message_event: yi_types::event::AssistantMessageEvent::Done {
            reason: yi_types::message::StopReason::Stop,
            message: message.clone(),
        },
    });
    app.reduce_agent(yi_types::event::AgentEvent::MessageEnd { message });
    let committed: Vec<String> = app.take_commits().iter().map(flat).collect();
    let at = |needle: &str| committed.iter().position(|line| line.contains(needle));
    let explored = at("Explored").ok_or(format!("no run: {committed:?}"))?;
    let thought = at("Pondering").ok_or(format!("no thought: {committed:?}"))?;
    assert!(explored < thought, "{committed:?}");
    Ok(())
}

/// Incident: every streamed event returned a scrolled pane to the bottom. Held above it,
/// the rows a reader is on stay put while the turn writes below; at zero the view follows.
#[test]
fn a_scrolled_pane_holds_still_while_the_transcript_grows() -> TestResult {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    let mut app = streamed("Start.\n");
    for index in 0..40 {
        app.commit_cell(&Cell::Notice {
            text: format!("note {index}"),
        });
    }
    let area = Rect::new(0, 0, 60, 16);
    let top = |app: &mut yi_tui::app::App, scroll: &mut usize| {
        let mut buffer = Buffer::empty(area);
        let _ = yi_tui::render::paint_pane(app, None, &mut buffer, area, scroll);
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol().to_owned()))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
    };
    let mut scroll = 0;
    let _ = top(&mut app, &mut scroll);
    scroll = 6;
    let before = top(&mut app, &mut scroll);
    for index in 40..45 {
        app.commit_cell(&Cell::Notice {
            text: format!("note {index}"),
        });
    }
    let after = top(&mut app, &mut scroll);
    assert_eq!(before.first(), after.first(), "{before:?}\n{after:?}");
    scroll = 0;
    let bottom = top(&mut app, &mut scroll);
    assert!(
        bottom.iter().any(|row| row.contains("note 44")),
        "at zero the view follows the turn: {bottom:?}"
    );
    Ok(())
}

/// A bare path or URL in running prose marks itself; `and/or` is still a word.
#[test]
fn prose_marks_its_paths_and_urls() -> TestResult {
    let theme = theme();
    let lines = yi_tui::markdown::render(
        "Edited crates/tui/src/app.rs:42 and/or Cargo.toml, see https://x.dev/a.",
        80,
        &theme,
    );
    let span = |needle: &str| {
        lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.content.as_ref() == needle)
            .cloned()
            .ok_or_else(|| format!("no span {needle:?} in {lines:?}"))
    };
    assert_eq!(span("crates/tui/src/app.rs:42")?.style.fg, Some(theme.cyan));
    assert_eq!(span("Cargo.toml")?.style.fg, Some(theme.cyan));
    assert_eq!(span("https://x.dev/a")?.style.fg, Some(theme.blue5));
    assert!(
        !lines
            .iter()
            .flat_map(|l| &l.spans)
            .any(|s| s.content.as_ref() == "and/or"),
        "a slash alone is not a path"
    );
    Ok(())
}

/// Incident: the space after a call's name kept its subject from ever reading as a path.
#[test]
fn a_call_head_splits_its_path_into_context_and_name() -> TestResult {
    let theme = theme();
    let cell = ToolCell {
        name: "read".to_owned(),
        call_id: "t1".to_owned(),
        intent: None,
        status: ToolStatus::Done,
        summary: ToolCell::summary_of("read", "crates/tui/src/app.rs"),
        digest: None,
        preview: Vec::new(),
        elapsed_ms: 0,
        calls: 1,
        details: serde_json::Value::Null,
    };
    let lines = cell.lines(80, &theme, TranscriptMode::Thinking, 0);
    let spans: Vec<&Span<'_>> = lines.iter().flat_map(|l| &l.spans).collect();
    let dir = spans
        .iter()
        .find(|s| s.content.as_ref() == "crates/tui/src/")
        .ok_or(format!("{spans:?}"))?;
    assert_eq!(dir.style, theme.dim_style());
    assert!(spans.iter().any(|s| s.content.as_ref() == "app.rs"));
    Ok(())
}
