use std::error::Error;

use yi_tools::hashline::format::{
    compute_file_hash, format_hashline_header, format_numbered_lines, split_addressable_file_lines,
};
use yi_tools::hashline::messages::{
    BARE_BODY_AUTO_PIPED_WARNING, BARE_RANGE_AUTO_PUT_WARNING, DIFF_OLD_ROWS_IGNORED_WARNING,
    EMPTY_PUT_AUTO_CUT_WARNING, MINUS_BULLET_AUTO_PIPED_WARNING, REPLACE_PAIR_COALESCED_WARNING,
    SNAPSHOT_ROWS_AUTO_PUT_WARNING,
};
use yi_tools::hashline::parser::{Executor, ParsedSection};
use yi_tools::hashline::tokenizer::{tokenize_all, try_parse_header};
use yi_tools::hashline::types::{Cursor, Edit, FileOp, PasteTarget};

type TestResult = Result<(), Box<dyn Error>>;

fn parse(patch: &str) -> Result<ParsedSection, String> {
    let mut executor = Executor::new();
    for token in tokenize_all(patch) {
        executor.feed(&token)?;
    }
    executor.end()
}

fn edit_kinds(section: &ParsedSection) -> Vec<&'static str> {
    section
        .edits
        .iter()
        .map(|edit| match edit {
            Edit::Insert { .. } => "insert",
            Edit::Delete { .. } => "delete",
            Edit::Cut { .. } => "cut",
            Edit::Paste { .. } => "paste",
            Edit::Block { .. } => "block",
        })
        .collect()
}

#[test]
fn tag_and_header_round_trip_through_the_tokenizer() -> TestResult {
    let tag = compute_file_hash("fn main() {}\n");
    let header = format_hashline_header("src/main.rs", tag);
    let parsed = try_parse_header(&header).ok_or("header did not parse")?;
    assert_eq!(parsed.path, "src/main.rs");
    assert_eq!(parsed.file_hash.as_deref(), Some(tag.to_string().as_str()));
    Ok(())
}

#[test]
fn tag_survives_trailing_whitespace_and_crlf() -> TestResult {
    let base = compute_file_hash("alpha\nbeta\n");
    assert_eq!(compute_file_hash("alpha  \nbeta\t\n"), base);
    assert_eq!(compute_file_hash("alpha\r\nbeta\r\n"), base);
    assert_ne!(compute_file_hash("alpha\nbetb\n"), base);
    Ok(())
}

#[test]
fn replace_hunk_lowers_to_inserts_plus_deletes() -> TestResult {
    let section = parse("PUT 2.=3:\n+new two\n+new three\n").map_err(|e| e.to_string())?;
    assert_eq!(
        edit_kinds(&section),
        vec!["insert", "insert", "delete", "delete"]
    );
    let Edit::Insert {
        cursor,
        text,
        replacement,
        ..
    } = &section.edits[0]
    else {
        return Err("expected insert".into());
    };
    assert_eq!(
        *cursor,
        Cursor::BeforeAnchor {
            anchor: yi_tools::hashline::types::Anchor { line: 2 }
        }
    );
    assert_eq!(text, "new two");
    assert!(replacement);
    Ok(())
}

#[test]
fn gap_inserts_and_eof_anchor_parse() -> TestResult {
    let section = parse("PUT <1:\n+head\nPUT >$:\n+tail\nPUT >4:\n+after four\n")
        .map_err(|e| e.to_string())?;
    let cursors: Vec<&Cursor> = section
        .edits
        .iter()
        .filter_map(|edit| match edit {
            Edit::Insert { cursor, .. } => Some(cursor),
            _ => None,
        })
        .collect();
    assert_eq!(cursors.len(), 3);
    assert_eq!(*cursors[0], Cursor::Bof);
    assert_eq!(*cursors[1], Cursor::Eof);
    assert!(matches!(cursors[2], Cursor::AfterAnchor { anchor } if anchor.line == 4));
    Ok(())
}

#[test]
fn cut_captures_then_anonymous_paste() -> TestResult {
    let section = parse("CUT 5.=6\nPUT >10\n").map_err(|e| e.to_string())?;
    assert_eq!(
        edit_kinds(&section),
        vec!["cut", "delete", "delete", "paste"]
    );
    let Edit::Paste {
        at: PasteTarget::Gap { cursor },
        register,
        ..
    } = &section.edits[3]
    else {
        return Err("expected gap paste".into());
    };
    assert!(matches!(cursor, Cursor::AfterAnchor { anchor } if anchor.line == 10));
    assert!(register.is_none());
    Ok(())
}

#[test]
fn rem_and_move_are_file_ops() -> TestResult {
    let rem = parse("REM\n").map_err(|e| e.to_string())?;
    assert_eq!(rem.file_op, Some(FileOp::Rem));
    let moved = parse("MV src/renamed.rs\n").map_err(|e| e.to_string())?;
    assert_eq!(
        moved.file_op,
        Some(FileOp::Move {
            dest: "src/renamed.rs".to_owned()
        })
    );
    let both = parse("PUT 1.=1:\n+x\nREM\n");
    assert!(both.is_err());
    Ok(())
}

#[test]
fn lenient_range_separators_recover() -> TestResult {
    for header in [
        "PUT 2-3:",
        "PUT 2=3:",
        "PUT 2..3:",
        "PUT 2 3:",
        "PUT 2\u{2026}3:",
    ] {
        let section = parse(&format!("{header}\n+x\n")).map_err(|e| format!("{header}: {e}"))?;
        let deletes = section
            .edits
            .iter()
            .filter(|edit| matches!(edit, Edit::Delete { .. }))
            .count();
        assert_eq!(deletes, 2, "{header}");
    }
    Ok(())
}

#[test]
fn dangling_separator_collapses_to_single_line() -> TestResult {
    let section = parse("PUT 244.=:\n+x\n").map_err(|e| e.to_string())?;
    let deletes = section
        .edits
        .iter()
        .filter(|edit| matches!(edit, Edit::Delete { anchor, .. } if anchor.line == 244))
        .count();
    assert_eq!(deletes, 1);
    Ok(())
}

#[test]
fn inverted_range_is_rejected_with_teaching_text() -> TestResult {
    let error = parse("PUT 10.=3:\n+x\n").err().ok_or("expected error")?;
    assert!(
        error.contains("Invalid absolute range: start 10, end 3"),
        "{error}"
    );
    assert!(error.contains("not a line count"), "{error}");
    Ok(())
}

#[test]
fn oversized_range_is_bounded() -> TestResult {
    let error = parse("CUT 1.=2000000\n").err().ok_or("expected error")?;
    assert!(error.contains("the maximum is 100000"), "{error}");
    Ok(())
}

#[test]
fn bare_body_rows_auto_pipe_with_warning() -> TestResult {
    let section = parse("PUT 5.=5:\nbare content\n").map_err(|e| e.to_string())?;
    assert!(
        section
            .warnings
            .iter()
            .any(|w| w == BARE_BODY_AUTO_PIPED_WARNING)
    );
    let Edit::Insert { text, .. } = &section.edits[0] else {
        return Err("expected insert".into());
    };
    assert_eq!(text, "bare content");
    Ok(())
}

#[test]
fn bare_range_header_recovers_as_put() -> TestResult {
    let section = parse("3.=4:\n+x\n+y\n").map_err(|e| e.to_string())?;
    assert!(
        section
            .warnings
            .iter()
            .any(|w| w == BARE_RANGE_AUTO_PUT_WARNING)
    );
    assert_eq!(
        edit_kinds(&section),
        vec!["insert", "insert", "delete", "delete"]
    );
    Ok(())
}

#[test]
fn snapshot_rows_recover_as_single_line_replacements() -> TestResult {
    let section = parse("7:updated seven\n").map_err(|e| e.to_string())?;
    assert!(
        section
            .warnings
            .iter()
            .any(|w| w == SNAPSHOT_ROWS_AUTO_PUT_WARNING)
    );
    assert_eq!(edit_kinds(&section), vec!["insert", "delete"]);
    let repeated = parse("7:one\n7:two\n");
    assert!(repeated.is_err());
    Ok(())
}

#[test]
fn empty_put_body_becomes_delete_with_warning() -> TestResult {
    let section = parse("PUT 4.=6:\n").map_err(|e| e.to_string())?;
    assert!(
        section
            .warnings
            .iter()
            .any(|w| w == EMPTY_PUT_AUTO_CUT_WARNING)
    );
    assert_eq!(edit_kinds(&section), vec!["delete", "delete", "delete"]);
    Ok(())
}

#[test]
fn unified_diff_contamination_is_rejected() -> TestResult {
    let error = parse("@@ -1,2 +1,2 @@\n").err().ok_or("expected error")?;
    assert!(error.contains("unified-diff hunk header"), "{error}");
    let sentinel = parse("*** Update File: src/x.rs\n")
        .err()
        .ok_or("expected error")?;
    assert!(sentinel.contains("apply_patch sentinel"), "{sentinel}");
    Ok(())
}

#[test]
fn minus_rows_with_explicit_plus_rows_are_dropped_as_diff_old() -> TestResult {
    let section = parse("PUT 2.=2:\n-old line()\n+new line()\n").map_err(|e| e.to_string())?;
    assert!(
        section
            .warnings
            .iter()
            .any(|w| w == DIFF_OLD_ROWS_IGNORED_WARNING)
    );
    let inserts: Vec<&str> = section
        .edits
        .iter()
        .filter_map(|edit| match edit {
            Edit::Insert { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(inserts, vec!["new line()"]);
    Ok(())
}

#[test]
fn markdown_bullets_are_kept_as_literal_content() -> TestResult {
    let section = parse("PUT 2.=2:\n- first item\n- second item\n").map_err(|e| e.to_string())?;
    assert!(
        section
            .warnings
            .iter()
            .any(|w| w == MINUS_BULLET_AUTO_PIPED_WARNING)
    );
    let inserts = section
        .edits
        .iter()
        .filter(|edit| matches!(edit, Edit::Insert { .. }))
        .count();
    assert_eq!(inserts, 2);
    Ok(())
}

#[test]
fn exact_duplicate_ranges_coalesce_to_the_last_hunk() -> TestResult {
    let section = parse("PUT 2.=3:\n+first\nPUT 2.=3:\n+second\n").map_err(|e| e.to_string())?;
    assert!(
        section
            .warnings
            .iter()
            .any(|w| w == REPLACE_PAIR_COALESCED_WARNING)
    );
    let inserts: Vec<&str> = section
        .edits
        .iter()
        .filter_map(|edit| match edit {
            Edit::Insert { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(inserts, vec!["second"]);
    Ok(())
}

#[test]
fn partial_overlap_is_rejected() -> TestResult {
    let error = parse("PUT 2.=4:\n+a\nPUT 4.=6:\n+b\n")
        .err()
        .ok_or("expected error")?;
    assert!(
        error.contains("already targeted by another hunk"),
        "{error}"
    );
    Ok(())
}

#[test]
fn colon_on_register_put_is_rejected() -> TestResult {
    let error = parse("PUT >4 @saved:\n").err().ok_or("expected error")?;
    assert!(error.contains("never takes `:`"), "{error}");
    Ok(())
}

#[test]
fn envelope_markers_bracket_and_terminate_parsing() -> TestResult {
    let section = parse("*** Begin Patch\nPUT 1.=1:\n+x\n*** End Patch\nPUT 9.=9:\n+ignored\n")
        .map_err(|e| e.to_string())?;
    assert_eq!(edit_kinds(&section), vec!["insert", "delete"]);
    Ok(())
}

#[test]
fn block_targets_parse_into_block_edits() -> TestResult {
    let section =
        parse("PUT 5*:\n+body\nCUT 9*\nPUT >12*:\n+after\n").map_err(|e| e.to_string())?;
    assert_eq!(edit_kinds(&section), vec!["block", "block", "block"]);
    Ok(())
}

#[test]
fn numbered_lines_format_for_display() -> TestResult {
    let numbered = format_numbered_lines("a\nb", 10);
    assert_eq!(numbered, "10:a\n11:b");
    assert_eq!(split_addressable_file_lines("a\nb\n"), vec!["a", "b"]);
    Ok(())
}

#[test]
fn apply_patch_noise_with_multibyte_lowercase_does_not_panic() -> TestResult {
    // 'İ' (U+0130) lowercases to "i\u{307}" (2 -> 3 bytes), so byte offsets
    // scanned over the original header must never index the lowered copy.
    let patch =
        yi_tools::hashline::input::Patch::parse("[update\u{130}file: x]\nPUT 1:\n+hi\n", None)
            .map_err(|e| e.to_string())?;
    assert_eq!(patch.sections[0].path, "x");
    Ok(())
}
