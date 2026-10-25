//! The Review pane: what a lane's branch would land, and what a turn or a session changed.

use yi_console::model::{FileDiff, MAX_DIFF_FILES, ReviewScope, SessionDiff};
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
