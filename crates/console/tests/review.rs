//! The Review pane: what a lane's branch would land, and what a turn or a session changed.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use yi_console::model::{FileDiff, PaneContent, ReviewScope, SessionDiff, SessionId, Zone};
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

/// The visible rows of the pane whose title starts with `title`, at 100 by 16.
fn pane_rows(app: &mut yi_console::app::App, title: &str) -> Vec<String> {
    let area = ratatui::layout::Rect::new(0, 0, 100, 16);
    let theme = app.theme;
    let view = yi_console::render::compute_view(app, area, &theme);
    let pane = view.panes.iter().find(|pane| pane.title.starts_with(title));
    pane.map_or_else(Vec::new, |pane| text(&pane.lines))
}

/// Dies with a Review taller than its pane opening on its last rows: the file list, the `▸`
/// and the key hint sat above the window, and `↓` moved a selection nobody could see.
#[test]
fn a_review_taller_than_its_pane_opens_on_its_file_list() {
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

/// Dies with a README opened through `md` showing its last page first; the first scroll down
/// from the top must move the view, not spend itself on the pin.
#[test]
fn a_markdown_viewer_opens_on_its_first_page() {
    let (mut app, outbound) = console();
    let readme = concat!(env!("CARGO_MANIFEST_DIR"), "/../../README.md");
    app.state.zone = Zone::Panes;
    alt(&mut app, &outbound, '/');
    for c in format!("md {readme}").chars() {
        press(&mut app, &outbound, KeyCode::Char(c));
    }
    press(&mut app, &outbound, KeyCode::Enter);
    let rows = pane_rows(&mut app, "¶ README.md");
    let row = |at: usize| rows.get(at).map_or("", String::as_str);
    assert_eq!(row(0), "Yi", "{rows:#?}");
    assert!(
        row(3).starts_with("A coding agent in one Rust binary"),
        "{rows:#?}"
    );
    alt(&mut app, &outbound, 'j');
    let scrolled = pane_rows(&mut app, "¶ README.md");
    assert_eq!(
        scrolled.first(),
        rows.get(3),
        "the first scroll moves three rows down"
    );
    outbound.shutdown();
}
