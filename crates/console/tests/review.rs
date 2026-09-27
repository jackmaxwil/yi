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
            .any(|line| line == "● src/a.rs  +1 −0  · write the parser"),
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

/// Dies with no picture of the time: where a session's minutes went, and its turns,
/// checkpoints and failures, were nowhere on screen to scrub.
#[test]
fn a_tape_draws_model_and_tool_time_and_points_at_the_chosen_mark() {
    use yi_types::tape::{Mark, MarkKind, Tape};
    let theme = Theme::new(ColorTier::TrueColor, true);
    let mark = |at: u64, kind: MarkKind, label: &str| Mark {
        at,
        kind,
        entry: format!("e{at}"),
        label: label.to_owned(),
    };
    let tape = Tape {
        start: 0,
        end: 10_000,
        model: vec![[0, 7_000]],
        tools: vec![[7_000, 9_000]],
        marks: vec![
            mark(0, MarkKind::User, "fix the gauge"),
            mark(8_000, MarkKind::Failed, "bash failed"),
            mark(9_000, MarkKind::Checkpoint, "checkpoint"),
        ],
    };
    let (title, lines) = yi_console::render::tape_view(Some(&tape), 1, 48, &theme);
    assert_eq!(title, "Tape · 10s · model 70% · tools 20%");
    let rows = text(&lines);
    assert!(rows[2].contains('┃'), "{rows:#?}");
    assert!(rows[3].contains('✗') && rows[3].contains('◆'), "{rows:#?}");
    assert!(
        rows.iter().any(|row| row == "+8s · bash failed"),
        "{rows:#?}"
    );
}
