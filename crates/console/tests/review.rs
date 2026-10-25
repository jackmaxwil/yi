//! The Review pane: what a lane's branch would land, and what a turn or a session changed.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use yi_console::model::{
    FileDiff, MAX_DIFF_FILES, PaneContent, ReviewScope, SessionDiff, SessionId, Zone,
};
use yi_tui::colors::{ColorTier, Theme};

fn text(lines: &[ratatui::text::Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

fn edit(path: &str, turn: u64, serving: Option<&str>) -> (String, FileDiff) {
    let patch = format!("--- a/{path}\n+++ b/{path}\n@@ -1 +1,2 @@\n line\n+added\n");
    let file = FileDiff {
        patch,
        added: 1,
        removed: 0,
        tracked: true,
        serving: serving.map(str::to_owned),
        turn,
    };
    (path.to_owned(), file)
}

/// Dies with one undivided session diff: a turn's edits could not be told from the session's,
/// and nothing said which todo a change served or what was only read.
#[test]
fn a_review_scopes_edits_by_turn_and_names_the_todo_each_served() {
    let theme = Theme::new(ColorTier::TrueColor, true);
    let mut diff = SessionDiff {
        turn: 2,
        ..SessionDiff::default()
    };
    diff.files
        .push(edit("src/a.rs", 1, Some("write the parser")));
    diff.files.push(edit("src/b.rs", 2, None));
    diff.reads.insert("src/c.rs".to_owned(), 3);
    let (title, lines) =
        yi_console::render::review_view(Some(&diff), ReviewScope::Turn, "", 80, &theme);
    assert_eq!(title, "Review · turn · 1 file +1 −0");
    let (title, lines_all) =
        yi_console::render::review_view(Some(&diff), ReviewScope::Session, "", 80, &theme);
    assert_eq!(title, "Review · session · 2 files +2 −0");
    let all = text(&lines_all);
    assert!(
        all.iter()
            .any(|line| line.contains("src/a.rs  +1 −0  · write the parser")),
        "{all:#?}"
    );
    assert!(
        all.iter().any(|line| line == "○ 1 file read only"),
        "{all:#?}"
    );
    assert!(text(&lines).iter().all(|line| !line.contains("src/a.rs")));
}

/// Dies with every branch file bare of its todo: an edit's patch names its file absolutely and
/// git's numstat from the repository root, so the lookup never matched; a relative read of an
/// edited file was counted as read only too.
#[test]
fn a_branch_review_names_the_todo_of_a_file_its_edit_named_absolutely() {
    let theme = Theme::new(ColorTier::TrueColor, true);
    let mut diff = SessionDiff::default();
    diff.files
        .push(edit("/repo/lane/src/a.rs", 0, Some("write the parser")));
    diff.reads.insert("src/a.rs".to_owned(), 2);
    diff.branch = Some(yi_types::lane::BranchDiff {
        base: "abc12345".to_owned(),
        files: vec![("src/a.rs".to_owned(), 1, 0)],
        patch: String::new(),
        untracked: 0,
    });
    let (_, lines) =
        yi_console::render::review_view(Some(&diff), ReviewScope::Branch, "", 80, &theme);
    let all = text(&lines);
    assert!(
        all.iter()
            .any(|line| line == "▸ src/a.rs  +1 −0  · write the parser"),
        "{all:#?}"
    );
    assert!(
        all.iter().all(|line| !line.contains("read only")),
        "{all:#?}"
    );
    let (_, lines) =
        yi_console::render::review_view(Some(&diff), ReviewScope::Session, "", 80, &theme);
    let all = text(&lines);
    assert!(
        all.iter().all(|line| !line.contains("read only")),
        "{all:#?}"
    );
}

/// Dies with no provenance: a hunk showed what changed and never why, though `yi why` could
/// walk its line to a commit, a todo and a goal.
#[test]
fn each_hunk_is_asked_why_at_its_first_changed_line_and_the_chain_shows_under_its_file() {
    let patch = "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,3 +1,4 @@\n one\n+two\n three\n@@ -10,2 +11,3 @@\n ten\n-old\n+new\n+newer\n";
    assert_eq!(yi_console::render::hunk_lines(patch), vec![2, 12]);
    let theme = Theme::new(ColorTier::TrueColor, true);
    let mut diff = SessionDiff::default();
    diff.files.push(edit("src/a.rs", 0, None));
    diff.files.push(edit("src/b.rs", 0, None));
    diff.selected = 1;
    diff.why.insert(
        "src/b.rs".to_owned(),
        vec!["  ↳ L2 1a2b3c4d5e6f Check evidence · todo t4 · goal tool ergonomics".to_owned()],
    );
    let (_, lines) =
        yi_console::render::review_view(Some(&diff), ReviewScope::Session, "", 80, &theme);
    let rows = text(&lines);
    let at = |needle: &str| rows.iter().position(|row| row.contains(needle));
    let (b, chain) = (at("▸ src/b.rs"), at("↳ L2 1a2b3c4d5e6f"));
    assert!(at("● src/a.rs").is_some(), "{rows:#?}");
    assert!(b.is_some() && chain == b.map(|b| b + 1), "{rows:#?}");
}

const MULTIFILE: &str = include_str!("fixtures/review-multifile.diff");

/// The fixture's six patches as the edits a session made, each counted from its own lines.
fn multifile_diff() -> SessionDiff {
    let mut diff = SessionDiff::default();
    for section in MULTIFILE.split("diff --git ").skip(1) {
        let path = section
            .lines()
            .find_map(|line| line.strip_prefix("+++ b/"))
            .unwrap_or_default();
        let count = |sign: char, header: &str| {
            let rows = section.lines().filter(|line| line.starts_with(sign));
            rows.filter(|line| !line.starts_with(header)).count() as u64
        };
        let file = FileDiff {
            patch: format!("diff --git {section}"),
            added: count('+', "+++"),
            removed: count('-', "---"),
            tracked: true,
            serving: None,
            turn: 0,
        };
        diff.files.push((path.to_owned(), file));
    }
    diff
}

fn console() -> (yi_console::app::App, yi_console::client::Outbound) {
    let (events, _received) = yi_console::client::events();
    let socket = std::env::temp_dir().join("yi-review-no-daemon.sock");
    let (outbound, _threads) = yi_console::client::spawn(socket, events);
    let app = yi_console::app::App::new("/repo".to_owned(), Theme::new(ColorTier::TrueColor, true));
    (app, outbound)
}

fn press(app: &mut yi_console::app::App, outbound: &yi_console::client::Outbound, code: KeyCode) {
    let key = KeyEvent::new(code, KeyModifiers::NONE);
    app.handle_event(outbound, ratatui::crossterm::event::Event::Key(key));
}

fn alt(app: &mut yi_console::app::App, outbound: &yi_console::client::Outbound, c: char) {
    let key = KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT);
    app.handle_event(outbound, ratatui::crossterm::event::Event::Key(key));
}

fn pane_rows(app: &mut yi_console::app::App, title: &str) -> Vec<String> {
    pane_rows_at(app, title, (100, 16))
}

/// The visible rows of the pane whose title starts with `title`, in a frame of `size`.
fn pane_rows_at(app: &mut yi_console::app::App, title: &str, size: (u16, u16)) -> Vec<String> {
    let area = ratatui::layout::Rect::new(0, 0, size.0, size.1);
    let theme = app.theme;
    let view = yi_console::render::compute_view(app, area, &theme);
    let pane = view.panes.iter().find(|pane| pane.title.starts_with(title));
    pane.map_or_else(Vec::new, |pane| text(&pane.lines))
}

/// Dies with a Review taller than its pane opening on its last rows: the file list, the `▸`
/// and the key hint sat above the window, and `↓` moved a selection nobody could see.
#[test]
fn a_review_taller_than_its_pane_opens_on_its_file_list() {
    let (mut app, outbound) = review_app();
    let rows = pane_rows(&mut app, "Review");
    let files = [
        "crates/console/src/app/chat.rs",
        "crates/console/src/app/diffs.rs",
        "crates/console/src/app/notebook.rs",
        "crates/console/src/model.rs",
        "crates/console/src/panes.rs",
        "docs/changes/2026-10-12-notebook-cells-persist.md",
    ];
    let row = |at: usize| rows.get(at).map_or("", String::as_str);
    let base = "no branch diff from git, so this session's edits";
    assert_eq!(row(0), base, "{rows:#?}");
    for (at, path) in files.iter().enumerate() {
        let mark = if at == 0 { "▸" } else { "●" };
        assert!(
            row(at + 1).starts_with(&format!("{mark} {path}  +")),
            "{rows:#?}"
        );
    }
    assert!(row(7).starts_with("s scope · ↑/↓ file"), "{rows:#?}");
    press(&mut app, &outbound, KeyCode::Down);
    let rows = pane_rows(&mut app, "Review");
    let row = |at: usize| rows.get(at).map_or("", String::as_str);
    assert!(
        row(1).starts_with("● crates/console/src/app/chat.rs"),
        "{rows:#?}"
    );
    assert!(
        row(2).starts_with("▸ crates/console/src/app/diffs.rs"),
        "{rows:#?}"
    );
    outbound.shutdown();
}

/// A console with `query` run in its palette, its pane focused.
fn palette_app(query: &str) -> (yi_console::app::App, yi_console::client::Outbound) {
    let (mut app, outbound) = console();
    app.state.zone = Zone::Panes;
    alt(&mut app, &outbound, '/');
    for c in query.chars() {
        press(&mut app, &outbound, KeyCode::Char(c));
    }
    press(&mut app, &outbound, KeyCode::Enter);
    (app, outbound)
}

const README: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../README.md");

/// Dies with a README opened through `md` showing its last page first; the first scroll down
/// from the top must move the view, not spend itself on the pin.
#[test]
fn a_markdown_viewer_opens_on_its_first_page() -> Result<(), Box<dyn std::error::Error>> {
    let (mut app, outbound) = palette_app(&format!("md {README}"));
    let rows = pane_rows(&mut app, "¶ README.md");
    let width = rows
        .iter()
        .map(|row| row.chars().count())
        .max()
        .unwrap_or(0);
    let source = std::fs::read_to_string(README)?;
    let all = text(&yi_tui::markdown::render(&source, width, &app.theme));
    assert_eq!(rows.first(), all.first(), "{rows:#?}");
    assert!(all.len() > rows.len(), "the README is taller than the pane");
    alt(&mut app, &outbound, 'j');
    let scrolled = pane_rows(&mut app, "¶ README.md");
    assert_eq!(
        scrolled.first(),
        rows.get(3),
        "the first scroll moves three rows down"
    );
    outbound.shutdown();
    Ok(())
}

/// Dies with a viewer's rows kept from the width it was first drawn at: after a split or a
/// resize the README and the patch drew wrapped for the old width.
#[test]
fn a_viewers_rows_follow_its_width() {
    let diff = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/review-multifile.diff"
    );
    for (query, title) in [
        (format!("md {README}"), "¶ README.md"),
        (format!("diff {diff}"), "Δ review-multifile.diff"),
    ] {
        let (mut warm, outbound) = palette_app(&query);
        for width in [100, 60, 100] {
            let (mut cold, cold_out) = palette_app(&query);
            let rows = pane_rows_at(&mut warm, title, (width, 40));
            assert!(!rows.is_empty(), "{title} drew nothing");
            assert_eq!(
                rows,
                pane_rows_at(&mut cold, title, (width, 40)),
                "{title} {width}"
            );
            cold_out.shutdown();
        }
        outbound.shutdown();
    }
}

/// A console showing the fixture's six edits in an open Review pane.
fn review_app() -> (yi_console::app::App, yi_console::client::Outbound) {
    let (mut app, outbound) = console();
    let session = SessionId("s-a".to_owned());
    if let Some(pane) = app.state.focused_pane_mut() {
        pane.content = PaneContent::Session {
            session: Some(session.clone()),
            chat: None,
        };
    }
    app.state.zone = Zone::Panes;
    app.state.diffs.insert(session, multifile_diff());
    alt(&mut app, &outbound, 'g');
    (app, outbound)
}

fn tool_frame(update: serde_json::Value) -> yi_console::client::ClientEvent {
    yi_console::client::ClientEvent::Frame(serde_json::json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {"sessionId": "s-a", "update": update},
    }))
}

/// Dies with a Review that kept rows drawn for another width or scope, or for the diff before
/// the agent's next edit or read landed: drawn once, its rows must still be the ones it shows.
#[test]
fn a_reviews_rows_follow_its_width_its_scope_and_each_new_edit() {
    let (mut warm, outbound) = review_app();
    let mut pressed = 0;
    for (width, presses) in [(100, 0), (60, 0), (60, 1), (100, 2), (60, 2)] {
        let (mut cold, cold_out) = review_app();
        for _ in 0..presses {
            press(&mut cold, &cold_out, KeyCode::Char('s'));
        }
        for _ in pressed..presses {
            press(&mut warm, &outbound, KeyCode::Char('s'));
        }
        pressed = presses;
        let rows = pane_rows_at(&mut warm, "Review", (width, 40));
        assert_eq!(
            rows,
            pane_rows_at(&mut cold, "Review", (width, 40)),
            "{width} {presses}"
        );
        cold_out.shutdown();
    }
    let edit = serde_json::json!({"sessionUpdate": "tool_call_update",
        "toolCallId": "e1", "title": "edit", "kind": "edit", "status": "completed",
        "rawOutput": {"patch": "--- a/src/new.rs\n+++ b/src/new.rs\n@@ -0,0 +1 @@\n+fn new() {}\n",
                      "added": 1, "removed": 0}});
    warm.reduce_client(&outbound, tool_frame(edit));
    let rows = pane_rows_at(&mut warm, "Review", (60, 40));
    assert!(
        rows.iter()
            .any(|row| row.starts_with("● src/new.rs  +1 −0")),
        "{rows:#?}"
    );
    let read = serde_json::json!({"sessionUpdate": "tool_call_update",
        "toolCallId": "r1", "title": "read", "kind": "read", "status": "completed",
        "rawInput": {"path": "src/only_read.rs"}});
    warm.reduce_client(&outbound, tool_frame(read));
    let rows = pane_rows_at(&mut warm, "Review", (60, 40));
    assert!(
        rows.iter().any(|row| row == "○ 1 file read only"),
        "{rows:#?}"
    );
    outbound.shutdown();
}

fn dropped_row_in(diff: &SessionDiff, scope: ReviewScope) -> Option<String> {
    let theme = Theme::new(ColorTier::TrueColor, true);
    let (_, lines) = yi_console::render::review_view(Some(diff), scope, "", 80, &theme);
    text(&lines).into_iter().find(|row| row.contains("[…]"))
}

fn dropped_row(diff: &SessionDiff) -> Option<String> {
    dropped_row_in(diff, ReviewScope::Session)
}

fn patch_of(path: &str, bytes: usize) -> (String, FileDiff) {
    let (path, mut file) = edit(path, 0, None);
    file.patch = "x".repeat(bytes);
    (path, file)
}

/// Dies with a review that lost its oldest edits without a word: file 513 pushed file 1 out of
/// the pane and the list read as every file the session touched.
#[test]
fn the_review_names_the_files_its_file_cap_dropped() {
    let mut diff = SessionDiff::default();
    for n in 0..MAX_DIFF_FILES {
        let (path, file) = edit(&format!("src/f{n}.rs"), 0, None);
        diff.insert(path, file);
    }
    assert_eq!(dropped_row(&diff), None, "512 files are all of them");
    let (path, file) = edit("src/f512.rs", 0, None);
    diff.insert(path, file);
    assert_eq!(
        dropped_row(&diff).as_deref(),
        Some("  […] 1 older file not shown · review keeps 512 files, 4 MiB · git diff lists them")
    );
}

/// The byte cap is the second way a file leaves: four MiB of patches fit, one byte more does not.
#[test]
fn the_review_names_the_files_its_byte_cap_dropped() {
    let mib = 1024 * 1024;
    let mut diff = SessionDiff::default();
    for n in 0..4 {
        let (path, file) = patch_of(&format!("big{n}.rs"), mib);
        diff.insert(path, file);
    }
    assert_eq!(dropped_row(&diff), None, "exactly 4 MiB fits");
    let (path, file) = patch_of("tiny.rs", 1);
    diff.insert(path, file);
    assert_eq!(
        dropped_row(&diff).as_deref(),
        Some("  […] 1 older file not shown · review keeps 512 files, 4 MiB · git diff lists them")
    );
}

/// An edit that brings a dropped file back un-drops it, so the count is files not shown.
#[test]
fn a_dropped_file_edited_again_is_no_longer_counted() {
    let mut diff = SessionDiff::default();
    for n in 0..=MAX_DIFF_FILES {
        let (path, file) = edit(&format!("src/f{n}.rs"), 0, None);
        diff.insert(path, file);
    }
    let (path, file) = edit("src/f0.rs", 0, None);
    diff.insert(path, file);
    assert!(
        diff.dropped_at_turn.contains_key("src/f1.rs")
            && !diff.dropped_at_turn.contains_key("src/f0.rs")
    );
    let row = dropped_row(&diff).unwrap_or_default();
    assert!(row.contains("1 older file not shown"), "{row}");
}

/// Dies with the turn view claiming files missing under "the last turn's edits" that an older
/// turn lost: the cap evicts the oldest files first, so those are almost never this turn's.
#[test]
fn a_turns_review_counts_only_the_files_its_own_turn_lost() {
    let mut diff = SessionDiff {
        turn: 2,
        ..SessionDiff::default()
    };
    for n in 0..=MAX_DIFF_FILES {
        let (path, file) = edit(&format!("src/old{n}.rs"), 1, None);
        diff.insert(path, file);
    }
    let (path, file) = edit("src/new.rs", 2, None);
    diff.insert(path, file);
    assert_eq!(
        dropped_row_in(&diff, ReviewScope::Turn),
        None,
        "two turn-1 files left the list and turn 2 lost none"
    );
    assert!(
        dropped_row(&diff).is_some_and(|row| row.contains("2 older files not shown")),
        "the session lost both"
    );
    for n in 0..MAX_DIFF_FILES {
        let (path, file) = edit(&format!("src/new{n}.rs"), 2, None);
        diff.insert(path, file);
    }
    assert_eq!(
        dropped_row_in(&diff, ReviewScope::Turn).as_deref(),
        Some("  […] 1 older file not shown · review keeps 512 files, 4 MiB · git diff lists them"),
        "only src/new.rs is turn 2's"
    );
    assert!(
        dropped_row(&diff).is_some_and(|row| row.contains("514 older files not shown")),
        "{:?}",
        dropped_row(&diff)
    );
}
