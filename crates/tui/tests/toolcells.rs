use crate::common;

use crate::common::flat_trimmed as flat;
use crate::scratch::Scratch;
use std::error::Error;

use serde_json::json;
use unicode_width::UnicodeWidthStr;
use yi_tui::app::{App, TuiOptions};
use yi_tui::cell::{Cell, ToolCell, ToolStatus, TranscriptMode};
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::keymap::default_keymap;
use yi_types::event::{AgentEvent, ToolResult};
use yi_types::message::Content;

type TestResult = Result<(), Box<dyn Error>>;

const ANSWERED: &str = "tests/fixtures/sessions/01a1b9ef.jsonl";

fn theme() -> Theme {
    Theme::new(ColorTier::TrueColor, true)
}

fn app() -> App {
    App::new(
        TuiOptions {
            session_name: "cells".to_owned(),
            ..common::options()
        },
        theme(),
        default_keymap(),
        80,
    )
}

fn started(app: &mut App, id: &str, tool: &str, arg: &str) {
    app.reduce_agent(AgentEvent::ToolExecutionStart {
        tool_call_id: id.to_owned(),
        tool_name: tool.to_owned(),
        args: json!({ "path": arg, "pattern": arg, "command": arg, "query": arg }),
    });
}

fn ended(app: &mut App, id: &str, tool: &str, text: &str) {
    ended_with(app, id, tool, text, false, serde_json::Value::Null);
}

fn ended_with(
    app: &mut App,
    id: &str,
    tool: &str,
    text: &str,
    is_error: bool,
    details: serde_json::Value,
) {
    app.reduce_agent(AgentEvent::ToolExecutionEnd {
        tool_call_id: id.to_owned(),
        tool_name: tool.to_owned(),
        result: ToolResult {
            content: vec![Content::Text {
                text: text.to_owned(),
                text_signature: None,
            }],
            details,
            usage: None,
            added_tool_names: None,
            terminate: None,
        },
        is_error,
    });
}

#[test]
fn a_running_tool_holds_no_rows_for_a_result_that_has_not_come() -> TestResult {
    let mut cell = ToolCell {
        name: "get_context".to_owned(),
        call_id: "c1".to_owned(),
        status: ToolStatus::Running,
        summary: ToolCell::summary_of("get_context", ""),
        digest: None,
        preview: Vec::new(),
        elapsed_ms: 0,
        calls: 1,
        details: serde_json::Value::Null,
    };
    let running = cell.lines(80, &theme(), TranscriptMode::Thinking, 0).len();
    cell.status = ToolStatus::Done;
    cell.preview = vec!["one".to_owned()];
    let done = cell.lines(80, &theme(), TranscriptMode::Thinking, 0).len();
    assert_eq!(
        done,
        running + 1,
        "the card reserved or padded rows around its result"
    );
    Ok(())
}

/// Eight rows of looking crowds out the answer that follows it, so the sources of a run cost
/// the transcript one dim row.
#[test]
fn a_run_of_read_only_calls_commits_as_one_row() -> TestResult {
    let mut app = app();
    for (index, (tool, arg)) in [("read", "src/lib.rs"), ("grep", "ToolCell")]
        .into_iter()
        .enumerate()
    {
        let id = format!("t{index}");
        started(&mut app, &id, tool, arg);
        ended(&mut app, &id, tool, "1:one\n2:two");
    }
    // Nothing is committed while the run is still open.
    assert!(app.take_commits().is_empty());

    app.reduce_agent(AgentEvent::AgentEnd {
        messages: Vec::new(),
        reason: None,
    });
    let committed = flat(&app.take_commits());
    assert_eq!(
        committed.first().map(String::as_str),
        Some("→ src/lib.rs  ✱ ToolCell")
    );
    assert!(
        !committed.iter().any(|row| row.contains("Explored")),
        "{committed:?}"
    );
    Ok(())
}

/// A lone read shows while it runs as the cell it commits as, so its row does not shrink into a
/// citation line and then grow back into a card when the run closes.
#[test]
fn a_lone_read_shows_live_as_the_cell_it_commits() -> TestResult {
    let mut app = app();
    started(&mut app, "r", "read", "src/lib.rs");
    ended(&mut app, "r", "read", "1:fn main() {}");
    assert!(app.take_commits().is_empty(), "the run is still open");
    let live = common::rows_sized(&mut app, 80, 24)?;
    app.reduce_agent(AgentEvent::AgentEnd {
        messages: Vec::new(),
        reason: None,
    });
    let landed = flat(&app.take_commits());
    let card = landed.get(..2).ok_or(format!("no card: {landed:#?}"))?;
    assert!(card[0].starts_with("│ → Read src/lib.rs"), "{landed:#?}");
    assert_eq!(
        live.get(..2),
        Some(card),
        "the live row and the landed row differ: {live:#?} / {landed:#?}"
    );
    assert!(
        !live.iter().any(|row| row.trim() == "→ src/lib.rs"),
        "a lone read drew the citation row while it ran: {live:#?}"
    );
    Ok(())
}

/// The head, the rail and the per-call rows are still one key away: verbose mode is the
/// answer to "which call earned this".
#[test]
fn verbose_mode_still_lists_every_call_of_the_run() -> TestResult {
    let mut rows = two_sources();
    if let Some(read) = rows.first_mut() {
        read.digest = Some("2 lines".to_owned());
    }
    let sources = Cell::Explored(rows);
    let row = flat(&sources.lines(80, &theme(), TranscriptMode::Normal, 0));
    assert_eq!(row, ["→ src/lib.rs  ✱ ToolCell"]);
    assert!(!row.join("").contains("2 lines"), "{row:?}");
    let block = flat(&sources.lines(80, &theme(), TranscriptMode::Verbose, 0));
    let joined = block.join("\n");
    assert!(joined.contains("✱ Explored ×2"), "{joined}");
    assert!(joined.contains("Read   src/lib.rs"), "{joined}");
    assert!(joined.contains("Search"), "{joined}");
    assert!(
        joined.contains("2 lines"),
        "the digest the read earned: {joined}"
    );
    Ok(())
}

fn source(tool: &str, arg: &str) -> ToolCell {
    ToolCell {
        name: tool.to_owned(),
        call_id: format!("{tool}:{arg}"),
        status: ToolStatus::Done,
        summary: ToolCell::summary_of(tool, arg),
        digest: None,
        preview: Vec::new(),
        elapsed_ms: 0,
        calls: 0,
        details: serde_json::Value::Null,
    }
}

fn two_sources() -> Vec<ToolCell> {
    vec![source("read", "src/lib.rs"), source("grep", "ToolCell")]
}

/// Nine sources with long titles: one row, the ones that fit, and the count it could not, at
/// both widths the owner named.
#[test]
fn nine_sources_are_one_row_cut_with_the_count() -> TestResult {
    let long = |n: usize| format!("crates/yi-collector-{n}/src/a-rather-long-module-name-{n}.rs");
    let rows: Vec<ToolCell> = (1..=9).map(|n| source("read", &long(n))).collect();
    let cell = Cell::Explored(rows);
    let at = |width: usize| flat(&cell.lines(width, &theme(), TranscriptMode::Normal, 0));
    let one = ["→ crates/yi-collector-1/src/a-rather-long-module-name-1.rs +8"];
    let three = [
        "→ crates/yi-collector-1/src/a-rather-long-module-name-1.rs  → crates/yi-collector-2/src/a-rather-long-module-name-2.rs  → crates/yi-collector-3/src/a-rather-long-module-name-3.rs +6",
    ];
    assert_eq!(at(80), one);
    assert_eq!(at(120), one);
    assert_eq!(at(200), three);
    Ok(())
}

/// A source wider than the row is cut with `…` where it is cut, so a path that does not exist is
/// never read as the real one, and a lone one too wide still says it was cut.
#[test]
fn a_source_wider_than_the_row_is_cut_with_an_ellipsis() -> TestResult {
    let long = |n: usize| format!("crates/yi-collector-{n}/src/a-rather-long-module-name-{n}.rs");
    let at = |rows: Vec<ToolCell>, width: usize| {
        flat(&Cell::Explored(rows).lines(width, &theme(), TranscriptMode::Normal, 0)).join("")
    };
    let run: Vec<ToolCell> = (1..=9).map(|n| source("read", &long(n))).collect();
    let row = at(run, 40);
    assert!(row.ends_with("… +8"), "{row}");
    assert!(!row.contains("a-rather-long-module-name-1.rs"), "{row}");
    assert!(UnicodeWidthStr::width(row.as_str()) <= 40, "{row}");
    let lone = at(vec![source("read", &long(1))], 40);
    assert_eq!(lone.matches('…').count(), 1, "{lone}");
    assert!(!lone.contains("long-module-name-1.rs"), "{lone}");
    assert!(UnicodeWidthStr::width(lone.as_str()) <= 40, "{lone}");
    Ok(())
}
#[test]
fn one_source_is_a_row_and_no_source_is_none() -> TestResult {
    let one = Cell::Explored(vec![source("read", "src/lib.rs")]);
    assert_eq!(
        flat(&one.lines(80, &theme(), TranscriptMode::Normal, 0)),
        ["→ src/lib.rs"]
    );
    let none = Cell::Explored(Vec::new());
    assert!(
        flat(&none.lines(80, &theme(), TranscriptMode::Normal, 0)).is_empty(),
        "no source is no row"
    );
    Ok(())
}

/// A single call must not gain a group header it does not need.
#[test]
fn a_lone_read_still_renders_as_itself() -> TestResult {
    let mut app = app();
    started(&mut app, "t0", "read", "src/lib.rs");
    ended(&mut app, "t0", "read", "1:one");
    app.reduce_agent(AgentEvent::AgentEnd {
        messages: Vec::new(),
        reason: None,
    });
    let joined = flat(&app.take_commits()).join("\n");
    assert!(!joined.contains("Explored"), "{joined}");
    assert!(joined.contains("→ Read src/lib.rs"), "{joined}");
    Ok(())
}

/// A write is not read-only: it closes the run rather than joining it, and the
/// group must land before the write, in the order they happened.
#[test]
fn a_writing_call_closes_the_run_in_order() -> TestResult {
    let mut app = app();
    for index in 0..2 {
        let id = format!("r{index}");
        started(&mut app, &id, "read", "a.rs");
        ended(&mut app, &id, "read", "1:one");
    }
    started(&mut app, "w", "bash", "cargo test");
    ended(&mut app, "w", "bash", "ok");
    let committed = flat(&app.take_commits());
    let explored = committed.iter().position(|row| row.contains("→ a.rs"));
    let bash = committed
        .iter()
        .position(|row| row.contains("$ cargo test"));
    assert!(explored < bash, "{committed:?}");
    assert!(explored.is_some() && bash.is_some(), "{committed:?}");
    Ok(())
}

/// The user is being asked about one specific call; colouring only the prompt
/// leaves the transcript showing it as an ordinary running row.
#[test]
fn a_call_held_at_the_permission_gate_says_so() -> TestResult {
    let mut app = app();
    started(&mut app, "t0", "bash", "rm -rf build");
    let held = |app: &mut App| -> Result<bool, Box<dyn Error>> {
        Ok(common::live_rows(app)?
            .iter()
            .any(|row| row.contains("rm -rf build") && row.contains('△')))
    };
    assert!(!held(&mut app)?, "a running call is not yet held");
    app.reduce_agent(AgentEvent::PermissionRequested {
        tool_call_id: "t0".to_owned(),
        title: "bash".to_owned(),
        description: "rm -rf build".to_owned(),
    });
    assert!(held(&mut app)?, "{:#?}", common::live_rows(&mut app)?);

    app.reduce_agent(AgentEvent::PermissionResolved {
        tool_call_id: "t0".to_owned(),
        allowed: true,
    });
    assert!(!held(&mut app)?, "{:#?}", common::live_rows(&mut app)?);
    Ok(())
}

fn edit_cell(path: &str) -> ToolCell {
    ToolCell {
        name: "edit".to_owned(),
        call_id: String::new(),
        status: ToolStatus::Done,
        summary: ToolCell::summary_of("edit", path),
        digest: Some("updated".to_owned()),
        preview: Vec::new(),
        elapsed_ms: 0,
        calls: 1,
        details: json!({
            "patch": format!("--- a/{path}\n+++ b/{path}\n@@ -1,1 +1,1 @@\n-one\n+two\n"),
            "added": 1,
            "removed": 1,
        }),
    }
}

/// A blank separates blocks and a run of one-line calls packs flush. Both
/// halves matter: always blank is as wrong as never blank.
#[test]
fn spacing_separates_blocks_but_not_one_line_rows() -> TestResult {
    let mut rows = app();
    started(&mut rows, "b0", "bash", "true");
    ended(&mut rows, "b0", "bash", "");
    started(&mut rows, "b1", "bash", "false");
    ended(&mut rows, "b1", "bash", "");
    let packed = flat(&rows.take_commits());
    assert!(
        !packed.iter().any(|row| row.is_empty()),
        "one-line rows pack flush: {packed:?}"
    );

    let mut blocks = app();
    blocks.commit_cell(&Cell::Tool(edit_cell("a.rs")));
    blocks.commit_cell(&Cell::Tool(edit_cell("b.rs")));
    let spaced = flat(&blocks.take_commits());
    assert!(
        spaced.iter().any(|row| row.is_empty()),
        "two multi-line blocks are separated: {spaced:?}"
    );
    Ok(())
}

/// A finished card showed one line of a forty-line result and no sign of the rest.
#[test]
fn a_collapsed_card_shows_its_budget_and_counts_the_rest() -> TestResult {
    let mut app = app();
    let text: Vec<String> = (1..=40).map(|n| format!("line {n}")).collect();
    started(&mut app, "b0", "bash", "seq 40");
    ended_with(
        &mut app,
        "b0",
        "bash",
        &text.join("\n"),
        false,
        json!({ "exitCode": 0 }),
    );
    let rows = flat(&app.take_commits());
    let joined = rows.join("\n");
    for needle in ["line 1", "line 2", "line 3", "line 4", "… 36 more lines"] {
        assert!(joined.contains(needle), "{needle}: {joined}");
    }
    assert!(!joined.contains("line 5"), "{joined}");
    assert!(
        !joined.contains("exit"),
        "a zero exit says nothing: {joined}"
    );
    assert_eq!(
        rows.iter().filter(|row| row.contains("line 1")).count(),
        1,
        "the first row is not repeated as a digest: {joined}"
    );
    Ok(())
}

/// The old `⏎ 0` chip sat on every card; a pipeline's tail hid a `command not found` under it.
#[test]
fn a_nonzero_exit_and_a_masked_not_found_are_the_chips() -> TestResult {
    let mut app = app();
    started(&mut app, "b0", "bash", "false");
    ended_with(
        &mut app,
        "b0",
        "bash",
        "",
        false,
        json!({ "exitCode": 127 }),
    );
    let joined = flat(&app.take_commits()).join("\n");
    assert!(joined.contains("exit 127"), "{joined}");
    assert!(!joined.contains('⏎'), "{joined}");

    started(&mut app, "b1", "bash", "fgj pr ls | head -20; echo ---");
    ended_with(
        &mut app,
        "b1",
        "bash",
        "sh: fgj: command not found\n---",
        false,
        json!({ "exitCode": 0 }),
    );
    let joined = flat(&app.take_commits()).join("\n");
    assert!(joined.contains("not found"), "{joined}");
    assert!(!joined.contains("exit 0"), "{joined}");
    Ok(())
}

/// Dies with a command that ran and failed drawn as done: the model's flag no longer marks a
/// non-zero exit or a rule's verdict, so the card reads them from `details` instead.
#[test]
fn a_nonzero_exit_and_a_verdict_still_open_with_a_cross() -> TestResult {
    let mut app = app();
    started(&mut app, "b0", "bash", "python3 -m unittest -q");
    ended_with(
        &mut app,
        "b0",
        "bash",
        "FAILED (failures=1)",
        false,
        json!({ "exitCode": 1 }),
    );
    let rows = flat(&app.take_commits());
    assert!(
        rows.iter().any(|row| row.contains("✕ $ python3")),
        "{rows:?}"
    );
    started(&mut app, "p0", "plan", "add_edge");
    ended_with(
        &mut app,
        "p0",
        "plan",
        "plan update refused: ordering cycle through [\"a\", \"b\"]",
        false,
        json!({ "errorKind": "verdict" }),
    );
    let rows = flat(&app.take_commits());
    assert!(rows.iter().any(|row| row.contains('✕')), "{rows:?}");
    Ok(())
}

#[test]
fn a_failed_card_opens_with_a_cross_and_keeps_its_reason() -> TestResult {
    let mut app = app();
    started(&mut app, "t0", "todo", "done t2");
    ended_with(
        &mut app,
        "t0",
        "todo",
        "done needs evidence: the command you ran",
        true,
        json!({}),
    );
    let rows = flat(&app.take_commits());
    assert!(rows.iter().any(|row| row.contains("✕ ≡ Todo")), "{rows:?}");
    assert!(
        rows.iter().any(|row| row.contains("done needs evidence")),
        "{rows:?}"
    );
    Ok(())
}

/// The head cut at 60 characters while the row had room to 120 and wraps past it.
#[test]
fn a_long_command_keeps_its_head_to_one_hundred_and_twenty() -> TestResult {
    let mut app = app();
    let command = format!("grep -rn {} crates/*/src --include='*.rs'", "x".repeat(70));
    started(&mut app, "b0", "bash", &command);
    ended(&mut app, "b0", "bash", "");
    let joined = flat(&app.take_commits()).join("\n");
    assert!(
        joined.contains("crates/*/src --include='*.rs'"),
        "the head wraps instead of cutting: {joined}"
    );
    assert!(joined.contains(&"x".repeat(70)), "{joined}");
    assert!(!joined.contains('…'), "{joined}");
    Ok(())
}

fn ended_todo(app: &mut App, id: &str, text: &str, is_error: bool) {
    app.reduce_agent(AgentEvent::ToolExecutionEnd {
        tool_call_id: id.to_owned(),
        tool_name: "todo".to_owned(),
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
        is_error,
    });
}

/// Six todo steps in one turn were six cards; the HUD block above the composer carries
/// the list, so a step commits nothing, a refusal keeps its card, and the close is one row.
#[test]
fn todo_steps_live_in_the_hud_not_the_transcript() -> TestResult {
    let mut app = app();
    started(&mut app, "t0", "todo", "start t1");
    ended_todo(
        &mut app,
        "t0",
        "Todos 0/2 · running: read\n- [>] read\n- [ ] write",
        false,
    );
    assert!(
        app.take_commits().is_empty(),
        "a step that succeeded commits no card"
    );
    started(&mut app, "t1", "todo", "done t1");
    ended_todo(
        &mut app,
        "t1",
        "done needs evidence: the command you ran",
        true,
    );
    let refused = flat(&app.take_commits()).join("\n");
    assert!(refused.contains("done needs evidence"), "{refused}");
    started(&mut app, "t2", "todo", "done t2");
    ended_todo(&mut app, "t2", "Todos 2/2\n- [x] read\n- [x] write", false);
    let closed = flat(&app.take_commits()).join("\n");
    assert!(closed.contains("↳ Todos 2/2"), "{closed}");
    assert!(!closed.contains("⚙"), "{closed}");
    Ok(())
}

fn custom(app: &mut App, kind: &str, text: &str, display: bool) {
    app.reduce_agent(AgentEvent::MessageEnd {
        message: yi_types::message::AgentMessage::Custom {
            custom_type: kind.to_owned(),
            content: yi_types::message::UserContent::Text(text.to_owned()),
            display,
            details: None,
            timestamp: 0,
        },
    });
}

/// The todo prelude restated the whole checklist as a dozen flagged rows under the
/// user's prompt; a message sent `display: false` is the model's alone.
#[test]
fn a_hidden_custom_message_stays_out_of_the_transcript() -> TestResult {
    let mut app = app();
    custom(
        &mut app,
        "todo_prelude",
        "The todo list still holds 1 open item(s) from before.\n## Ground\n- [x] t1 read",
        false,
    );
    custom(&mut app, "reminder", "Relevant: skill://plan", true);
    let rows = flat(&app.take_commits()).join("\n");
    assert!(!rows.contains("todo_prelude"), "{rows}");
    assert!(!rows.contains("t1 read"), "{rows}");
    assert!(rows.contains("⚑ reminder Relevant: skill://plan"), "{rows}");
    Ok(())
}

/// #946 Q4: a compaction note is a custom message on the model's side, but it reads as the
/// plain host line it always was, with no `compaction_notice` source label.
#[test]
fn a_compaction_notice_reads_as_a_plain_host_line() -> TestResult {
    let mut app = app();
    let notice =
        "[compaction failed: upstream 529; history left uncompacted, the next prompt retries]";
    custom(
        &mut app,
        yi_runtime::compaction::COMPACTION_NOTICE,
        notice,
        true,
    );
    let rows = flat(&app.take_commits()).join("\n");
    assert!(rows.contains("[compaction failed: upstream 529;"), "{rows}");
    assert!(!rows.contains("compaction_notice"), "{rows}");
    Ok(())
}

/// Incident: a failed call's digest returned before the sanitizer, so the tab `diff -u` puts
/// before a header's timestamp reached the card. Producer: Apple diff (FreeBSD), exit 1.
#[test]
fn a_failed_digest_shows_no_control_character() {
    let digest = ToolCell::digest_of(
        "bash",
        include_str!("fixtures/diff-u.txt"),
        &serde_json::Value::Null,
        true,
    );
    assert_eq!(digest.as_deref(), Some("--- a.txt    2026-09-28 10:00:00"));
}

#[test]
fn a_control_character_shows_as_its_glyph_and_a_tab_as_four_spaces() {
    let raw = "a\tb\r\u{1b}[1m\u{7f}\u{85}";
    assert_eq!(
        yi_tui::transcript::show_controls(raw),
        "a    b␍␛[1m␡\u{fffd}"
    );
}

/// Invariant: whatever a body's text missed, the real terminal gets one visible cell and
/// never a control it would obey.
#[test]
fn a_control_character_in_a_cell_reaches_the_terminal_as_its_glyph() -> TestResult {
    use ratatui::backend::Backend;
    // ratatui keeps CRLF as one grapheme in one cell, so it must stay one glyph wide.
    let [tab, carriage, crlf, plain] = ["\t", "\r", "\r\n", "x"].map(|symbol| {
        let mut cell = ratatui::buffer::Cell::default();
        cell.set_symbol(symbol);
        cell
    });
    let cells = [
        (0, 0, &tab),
        (1, 0, &carriage),
        (2, 0, &crlf),
        (3, 0, &plain),
    ];
    let mut backend = yi_tui::term::ControlPictures(ratatui::backend::TestBackend::new(4, 1));
    backend.draw(cells.into_iter())?;
    backend.0.assert_buffer_lines(["␉␍␍x"]);
    Ok(())
}

/// Incident: a title holding `BEL ESC]52;…` ended OSC 2 early and had the terminal write the
/// clipboard; C1 ST (U+009C) ends an OSC on terminals that read 8-bit controls.
#[test]
fn a_window_title_carries_no_control_character() {
    let osc = yi_tui::term::window_title_osc("Fix\u{7}\u{1b}]52;c;ZWNobyBwd25lZA==\u{9c} now");
    assert_eq!(osc, "\u{1b}]2;Fix]52;c;ZWNobyBwd25lZA== now\u{7}");
}

/// The edit cell's rows on a real screen, top to bottom, from the diff's last row to the card's end.
fn rows_under_diff(app: &mut App, last_diff: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let rows = common::rows_at(app, 60)?;
    let at = (rows.iter())
        .position(|row| row.ends_with(last_diff))
        .ok_or_else(|| format!("no diff row {last_diff:?}: {rows:#?}"))?;
    Ok(rows
        .into_iter()
        .skip(at.saturating_add(1))
        .take_while(|row| row.starts_with('│'))
        .collect())
}

/// Incident (#1042): the model quoted ripwire's caller check while the edit cell showed only the
/// diff. Producer: `yi ask --faux` editing a committed a.py whose caller b.py, ripwire 0.6.5.
#[test]
fn an_edit_cell_shows_the_check_rows_the_model_read_under_its_diff() -> TestResult {
    let mut app = app();
    let session = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/sessions/01a129b6.jsonl"
    );
    common::replay_stream(&mut app, std::path::Path::new(session))?;
    let under = rows_under_diff(&mut app, "+  2 │     return f\"hi {name}\"")?;
    assert_eq!(
        under,
        [
            "│   syntax: ok",
            "│   [ripwire check]",
            "│   greet at a.py:1: params,broken-callers changed, params 2 -> 1; 1 callers,",
            "│     1 incompatible",
            "│     <file-scope> b.py:1 — incompatible at line 3",
        ]
    );
    Ok(())
}

/// A result with no verdict rows draws the diff and nothing else: the numbered window the model
/// anchors on is not a check row.
#[test]
fn an_edit_cell_without_check_rows_ends_at_its_diff() -> TestResult {
    let mut app = app();
    started(&mut app, "e0", "edit", "notes.md");
    let details = edit_cell("notes.md").details;
    let text = "[notes.md#5B1C]\nupdated; first change at line 1\n1:two";
    ended_with(&mut app, "e0", "edit", text, false, details);
    assert_eq!(
        rows_under_diff(&mut app, "+    │ two")?,
        Vec::<String>::new()
    );
    Ok(())
}

/// A multi-file edit prints one `[path#TAG]` per file; its verdict rows without them name no file.
#[test]
fn a_multi_file_edit_names_the_file_above_each_verdict() -> TestResult {
    let mut app = app();
    started(&mut app, "e0", "edit", "a.py");
    let details = edit_cell("a.py").details;
    let text = concat!(
        "[a.py#1A2B]\nupdated; first change at line 1\n1:two\nsyntax: ok\n\n",
        "[b.py#3C4D]\nupdated; first change at line 3\n3:x = (\nsyntax: error line 3: '(' was never closed",
    );
    ended_with(&mut app, "e0", "edit", text, false, details);
    assert_eq!(
        rows_under_diff(&mut app, "+    │ two")?,
        [
            "│   [a.py#1A2B]",
            "│   syntax: ok",
            "│   [b.py#3C4D]",
            "│   syntax: error line 3: '(' was never closed",
        ]
    );
    Ok(())
}

/// Invariant: a preview row's colour follows its own text, not details["syntax"]:
/// worst() keeps only the first failure, so a second file's error row must stay red.
#[test]
fn every_syntax_error_row_under_a_diff_carries_the_error_colour() -> TestResult {
    let theme = theme();
    let mut cell = edit_cell("a.py");
    cell.details["syntax"] = json!("syntax: error a.py:1: expected an expression");
    cell.preview = vec![
        "a.py: syntax: error a.py:1: expected an expression".to_owned(),
        "b.py: syntax: error b.py:9: unterminated string".to_owned(),
        "syntax: ok".to_owned(),
    ];
    let rows = cell.lines(80, &theme, TranscriptMode::Normal, 0);
    let spans: Vec<(String, Option<ratatui::style::Color>)> = rows
        .iter()
        .flat_map(|row| row.spans.iter())
        .map(|span| (span.content.to_string(), span.style.fg))
        .collect();
    for (text, fg) in &spans {
        if text.contains("syntax: error") {
            assert_eq!(*fg, Some(theme.error), "muted error row: {text}");
        }
    }
    assert!(
        spans.iter().any(|(text, _)| text.contains("b.py")),
        "second file's verdict row missing: {spans:?}"
    );
    Ok(())
}

/// Dies with a generic gear and the fallback hue on tools the registry has: the tables named
/// `ls`, `glob` and `fetch`, so `todo`, `answer` and `get_context` fell through to `_`. The
/// names come from the recorded sessions, so a tool a real run calls cannot go unlisted.
#[test]
fn every_tool_a_recorded_session_calls_has_its_own_glyph_and_hue() -> TestResult {
    let theme = theme();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sessions");
    let mut called = std::collections::BTreeSet::new();
    for entry in std::fs::read_dir(dir)? {
        for line in std::fs::read_to_string(entry?.path())?.lines() {
            let row: serde_json::Value = serde_json::from_str(line)?;
            if let Some(name) = row
                .pointer("/message/toolName")
                .and_then(|name| name.as_str())
            {
                called.insert(name.to_owned());
            }
        }
    }
    for registered in ["todo", "answer", "get_context"] {
        assert!(
            called.contains(registered),
            "no recorded session calls {registered}"
        );
    }
    for name in &called {
        let head = ToolCell::summary_of(name, "");
        assert!(
            !head.starts_with('⚙'),
            "{name} draws the generic gear: {head}"
        );
        assert_ne!(
            yi_tui::card::tool_hue(&theme, name),
            theme.warning,
            "{name} has no hue"
        );
    }
    Ok(())
}

#[test]
fn web_search_card_has_its_query_glyph_hue_and_citation_count() -> TestResult {
    let theme = theme();
    let summary = ToolCell::summary_of("web_search", "latest Rust");
    assert!(summary.starts_with('⌕'), "{summary}");
    assert!(summary.contains("latest Rust"));
    assert_eq!(yi_tui::card::tool_hue(&theme, "web_search"), theme.cyan);
    assert_eq!(yi_tui::cell::explore_verb("web_search"), Some("Search"));
    let digest = ToolCell::digest_of(
        "web_search",
        "answer",
        &json!({"citationCount": 5, "costUsd": 0.0075}),
        false,
    );
    assert_eq!(digest.as_deref(), Some("5 citations · $0.007"));
    let mut app = app();
    started(&mut app, "search", "web_search", "latest Rust");
    ended(&mut app, "search", "web_search", "answer");
    app.reduce_agent(AgentEvent::AgentEnd {
        messages: Vec::new(),
        reason: None,
    });
    let rows = flat(&app.take_commits()).join("\n");
    assert!(rows.contains("latest Rust"), "{rows}");
    Ok(())
}

/// The session `source` with every result's `[path#TAG]` header spread over two lines: the same
/// results worded differently, as a token-saving change to the model-facing text would.
fn reworded(source: &std::path::Path, into: &std::path::Path) -> TestResult {
    let mut out = String::new();
    for line in std::fs::read_to_string(source)?.lines() {
        let mut row: serde_json::Value = serde_json::from_str(line)?;
        if row.pointer("/message/role").and_then(|role| role.as_str()) == Some("toolResult")
            && let Some(text) = row.pointer_mut("/message/content/0/text")
            && let Some(old) = text.as_str().map(str::to_owned)
        {
            let header = |row: &str| {
                let inner = row.strip_prefix('[')?.strip_suffix(']')?;
                let (path, tag) = inner.split_once('#')?;
                Some(format!("file: {path}\ntag: {tag}"))
            };
            let rows: Vec<String> = old
                .lines()
                .map(|row| header(row).unwrap_or_else(|| row.to_owned()))
                .collect();
            *text = serde_json::Value::String(rows.join("\n"));
        }
        out.push_str(&row.to_string());
        out.push('\n');
    }
    std::fs::write(into, out)?;
    Ok(())
}

/// The first 24 characters of the first line of each bash result in a recorded session: what the
/// card's digest says of a command, taken from the file and not from anything Yi drew.
fn bash_first_lines(session: &std::path::Path) -> Result<Vec<String>, Box<dyn Error>> {
    let mut heads = Vec::new();
    for line in std::fs::read_to_string(session)?.lines() {
        let row: serde_json::Value = serde_json::from_str(line)?;
        if row
            .pointer("/message/toolName")
            .and_then(|name| name.as_str())
            == Some("bash")
            && let Some(text) = row
                .pointer("/message/content/0/text")
                .and_then(|t| t.as_str())
            && let Some(first) = text.lines().find(|line| !line.trim().is_empty())
        {
            heads.push(first.chars().take(24).collect());
        }
    }
    Ok(heads)
}

/// Dies with a digest read off the model's own words: a grep recorded from the real tool drew
/// `0 hits` for six, because its rows are `N:text` under a `[path#TAG]` header and not
/// `path:N:text`, and an edit's digest was whatever line two of its text became once the header
/// moved. Producer: `yi ask --faux` over a three-file tree (grep) and a five-line file (an
/// insert-after edit), the older sessions (edit, read, bash).
#[test]
fn digests_come_from_the_typed_details_and_survive_rewording_the_text() -> TestResult {
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sessions");
    let scratch = Scratch::new("yi-digest")?;
    let mut missing = Vec::new();
    let bash_heads = bash_first_lines(&fixtures.join("01a0d6e4.jsonl"))?;
    assert_eq!(bash_heads.len(), 4, "the session ran four commands");
    for (session, wanted) in [
        (
            "01a129b6",
            vec![
                "2 lines".to_owned(),
                "updated; first change at line 1".to_owned(),
            ],
        ),
        ("01a16f24", vec!["6 hits".to_owned()]),
        // An insert after line 2: the tool reports its anchor, 2, where the patch's first
        // changed row is 3.
        (
            "01a16f6f",
            vec![
                "5 lines".to_owned(),
                "updated; first change at line 2".to_owned(),
            ],
        ),
        ("01a0d6e4", bash_heads),
    ] {
        let original = fixtures.join(format!("{session}.jsonl"));
        let spread = scratch.join(format!("{session}.jsonl"));
        reworded(&original, &spread)?;
        for path in [&original, &spread] {
            let mut app = app();
            common::replay_stream(&mut app, path)?;
            let shown = app
                .reflowed(500)
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            let name = path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into());
            let reworded = if path == &original { "" } else { " reworded" };
            missing.extend(
                wanted
                    .iter()
                    .filter(|digest| !shown.contains(digest.as_str()))
                    .map(|digest| format!("{name}{reworded}: no `{digest}`")),
            );
        }
    }
    assert_eq!(missing, Vec::<String>::new());
    Ok(())
}

/// Dies on the old three-row preview: a 30-row screen budgets five result rows, so a sixth
/// shows whole (its cut would save nothing) and a seventh is cut to five and `… 2 more lines`;
/// a 20-row screen holds the four-row floor and a 90-row one the twelve-row ceiling.
#[test]
fn a_card_one_row_past_its_budget_shows_whole_and_two_past_is_cut() -> TestResult {
    for (screen, budget) in [(20, 4), (30, 5), (90, 12)] {
        card_at_its_budget(screen, budget)?;
    }
    Ok(())
}

fn card_at_its_budget(screen: u16, budget: usize) -> TestResult {
    for (total, kept, more) in [
        (budget + 1, budget + 1, None),
        (budget + 2, budget, Some(2)),
    ] {
        let mut app = app();
        app.set_rows(usize::from(screen));
        let text: Vec<String> = (1..=total).map(|n| format!("row {n}")).collect();
        started(&mut app, "b0", "bash", "seq");
        ended_with(
            &mut app,
            "b0",
            "bash",
            &text.join("\n"),
            false,
            json!({ "exitCode": 0 }),
        );
        let rows = common::rows_sized(&mut app, 80, screen)?;
        let head = (rows.iter().position(|row| row.starts_with("│ $ seq")))
            .ok_or_else(|| format!("no card: {rows:#?}"))?;
        let body: Vec<&str> = (rows.iter().skip(head + 1))
            .take_while(|row| row.starts_with('│'))
            .map(|row| row.trim_start_matches('│').trim())
            .collect();
        let mut want: Vec<String> = text.iter().take(kept).cloned().collect();
        want.extend(more.map(|n| format!("… {n} more lines")));
        assert_eq!(
            body, want,
            "{total} result rows on a {budget}-row budget: {rows:#?}"
        );
    }
    Ok(())
}

/// Dies on a cut that ran past the runtime's own elision row: a failed 40-line result keeps
/// twelve head rows, that row and six tail rows, and a 48-row screen's failed budget is sixteen,
/// so the card drew two counts, or hid the tail where a command's error is; it keeps both ends.
#[test]
fn a_failed_card_cut_past_the_runtime_elision_counts_once() -> TestResult {
    let mut app = app();
    app.set_rows(48);
    let text: Vec<String> = (1..=40).map(|n| format!("row {n}")).collect();
    started(&mut app, "b0", "bash", "seq");
    ended_with(
        &mut app,
        "b0",
        "bash",
        &text.join("\n"),
        true,
        json!({ "exitCode": 1 }),
    );
    let rows = common::rows_sized(&mut app, 80, 48)?;
    let counts: Vec<&String> = (rows.iter())
        .filter(|row| row.contains("more lines"))
        .collect();
    assert_eq!(counts.len(), 1, "{rows:#?}");
    let count = counts
        .first()
        .map_or("", |row| row.trim_start_matches('│').trim());
    assert_eq!(count, "… 25 more lines", "{rows:#?}");
    let shown: Vec<&str> = (rows.iter())
        .map(|row| row.trim_start_matches('│').trim())
        .filter(|row| row.starts_with("row "))
        .collect();
    let mut want: Vec<String> = (1..=12).map(|n| format!("row {n}")).collect();
    want.extend((38..=40).map(|n| format!("row {n}")));
    let body = shown.get(shown.len().saturating_sub(want.len())..);
    assert_eq!(
        body,
        Some(&want.iter().map(String::as_str).collect::<Vec<_>>()[..]),
        "{rows:#?}"
    );
    Ok(())
}

/// A recorded session's 47-line `wc` table and 28-line failed scan, replayed on a 36-row screen:
/// the table keeps its budget of rows and counts the rest, the failure keeps twice that.
#[test]
fn a_recorded_sessions_results_fill_their_budgets() -> TestResult {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/sessions/01a0d6e4.jsonl"
    );
    let results: Vec<(String, bool)> =
        common::session::session_entries(&std::fs::read_to_string(path)?)?
            .into_iter()
            .filter_map(|message| match message {
                yi_types::message::AgentMessage::ToolResult {
                    tool_name,
                    content,
                    is_error,
                    ..
                } if tool_name == "bash" => Some((text_of(&content), is_error)),
                _ => None,
            })
            .collect();
    let mut app = app();
    app.set_rows(36);
    common::replay_stream(&mut app, std::path::Path::new(path))?;
    let rows = flat(&app.take_commits());
    for (opens, failed) in [
        ("crates/ai/ src=4905", false),
        ("crates/ai/src/settle.rs:1", true),
    ] {
        let (text, _) = (results
            .iter()
            .find(|(text, error)| text.contains(opens) && *error == failed))
        .ok_or_else(|| format!("no {opens} result in the session"))?;
        let lines: Vec<&str> = text.lines().collect();
        let budget = yi_tui::budget::card_rows(36, failed);
        let second = (rows.iter().position(|row| row.contains(opens)))
            .ok_or_else(|| format!("no card opening {opens}: {rows:#?}"))?;
        let at = second.checked_sub(1).ok_or("the card has no first row")?;
        for (offset, line) in lines.iter().take(budget).enumerate() {
            let row = rows.get(at + offset).map_or("", String::as_str);
            assert!(
                row.contains(line.trim()),
                "row {offset} of {opens}: {row:?} for {line:?}"
            );
        }
        let more = format!("… {} more lines", lines.len() - budget);
        let elision = rows.get(at + budget).map_or("", String::as_str);
        assert!(
            elision.contains(&more),
            "{elision:?} after {budget} rows of {opens}"
        );
    }
    Ok(())
}

fn text_of(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn rows_matching(app: &App, needle: &str) -> usize {
    flat(&app.reflowed(100_000))
        .iter()
        .filter(|row| row.contains(needle))
        .count()
}

/// Two recorded turns each wrote the reply as prose and sent it again through `answer`; the
/// transcript carries each reply once and no answer card.
#[test]
fn a_streamed_answer_turn_shows_its_reply_once_and_no_answer_card() -> TestResult {
    let mut app = app();
    let session = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(ANSWERED);
    common::replay_stream(&mut app, &session)?;
    assert_eq!(rows_matching(&app, "Ranked by size label"), 1);
    assert_eq!(rows_matching(&app, "To prevent it:"), 1);
    assert_eq!(rows_matching(&app, "Answer delivered"), 0);
    Ok(())
}

/// A resumed session draws what the live turn drew: the reply once, no answer card.
#[test]
fn a_replayed_answer_turn_shows_its_reply_once_and_no_answer_card() -> TestResult {
    let mut app = app();
    let text =
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(ANSWERED))?;
    let entries: Vec<yi_types::entry::Entry> = text
        .lines()
        .filter(|line| line.contains("\"kind\":\"entry\""))
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    app.replay_entries(&entries);
    assert_eq!(rows_matching(&app, "Ranked by size label"), 1);
    assert_eq!(rows_matching(&app, "To prevent it:"), 1);
    assert_eq!(rows_matching(&app, "Answer delivered"), 0);
    Ok(())
}

/// A saved turn that stopped before its answer was appended keeps the prose the live turn showed.
#[test]
fn a_replayed_turn_cut_before_its_answer_keeps_the_prose() -> TestResult {
    let mut app = app();
    let text =
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(ANSWERED))?;
    let mut entries: Vec<yi_types::entry::Entry> = text
        .lines()
        .filter(|line| line.contains("\"kind\":\"entry\""))
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    let appended = entries.iter().rposition(|entry| {
        matches!(
            entry,
            yi_types::entry::Entry::Message {
                message: yi_types::message::AgentMessage::Assistant { .. },
                ..
            }
        )
    });
    entries.truncate(appended.ok_or("the fixture has no reply")?);
    app.replay_entries(&entries);
    assert_eq!(rows_matching(&app, "To prevent it:"), 1);
    Ok(())
}

/// A refused draft is the model's to fix; its refusal speaks to the model, never on screen.
#[test]
fn a_refused_answer_draws_no_row() -> TestResult {
    let mut app = app();
    started(&mut app, "a1", "answer", "");
    let refusal = "These numbers appear in no tool result or user message: 1895, 1894.\nThe answer is held, not shown.";
    ended_with(
        &mut app,
        "a1",
        "answer",
        refusal,
        true,
        serde_json::Value::Null,
    );
    assert_eq!(rows_matching(&app, "answer is held"), 0);
    assert!(
        common::live_rows(&mut app)?
            .iter()
            .all(|row| !row.contains("Answer"))
    );
    Ok(())
}
