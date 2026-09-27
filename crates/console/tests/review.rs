//! The Review pane: what a lane's branch would land, and what a turn or a session changed.

use yi_console::model::{FileDiff, ReviewScope, SessionDiff};
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
            .any(|line| line.contains("● src/a.rs  +1 −0  · write the parser")),
        "{all:#?}"
    );
    assert!(
        all.iter().any(|line| line == "○ 1 files read only"),
        "{all:#?}"
    );
    assert!(text(&lines).iter().all(|line| !line.contains("src/a.rs")));
}
