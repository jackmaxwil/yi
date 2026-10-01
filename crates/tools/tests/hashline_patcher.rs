use std::error::Error;
use std::fs;

use serde_json::{Map, Value, json};
use yi_tools::hashline::tool::{
    HashlineEditTool, HashlineReadTool, SharedHashline, shared_hashline_state,
};
use yi_tools::{Tool, ToolContext, ToolOutput, WriteTool};
use yi_types::message::Content;

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

fn output_text(output: &ToolOutput) -> String {
    output
        .result
        .content
        .iter()
        .map(|content| match content {
            Content::Text { text, .. } => text.clone(),
            _ => String::new(),
        })
        .collect()
}

fn args(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect()
}

struct Fixture {
    _dir: Scratch,
    context: ToolContext,
    state: SharedHashline,
}

impl Fixture {
    fn new(tag: &str) -> Result<Self, Box<dyn Error>> {
        let dir = Scratch::new(&format!("yi-hashline-{tag}"))?;
        let context = ToolContext::new(dir.to_path_buf());
        Ok(Self {
            _dir: dir,
            context,
            state: shared_hashline_state(),
        })
    }

    fn write(&self, path: &str, content: &str) -> Result<(), Box<dyn Error>> {
        fs::write(self.context.cwd.join(path), content)?;
        Ok(())
    }

    fn read_tool(&self, path: &str) -> ToolOutput {
        HashlineReadTool::new(std::sync::Arc::clone(&self.state))
            .execute(args(&[("path", json!(path))]), &self.context)
    }

    /// The runtime previews every call for the permission ask before it executes it; the
    /// fixture does the same, so a preview's side effects cannot hide from the suite.
    fn edit(&self, patch: &str) -> ToolOutput {
        let tool = HashlineEditTool {
            state: std::sync::Arc::clone(&self.state),
            freeform_grammar: false,
        };
        let _ = tool.preview(&args(&[("patch", json!(patch))]), &self.context.cwd);
        tool.execute(args(&[("patch", json!(patch))]), &self.context)
    }

    fn tag_of(&self, path: &str) -> Result<String, Box<dyn Error>> {
        let read = self.read_tool(path);
        let text = output_text(&read);
        let header = text.lines().next().ok_or("empty read output")?;
        let tag = header
            .rsplit_once('#')
            .map(|(_, tail)| tail.trim_end_matches(']').to_owned())
            .ok_or("no tag in header")?;
        Ok(tag)
    }

    fn content(&self, path: &str) -> Result<String, Box<dyn Error>> {
        Ok(fs::read_to_string(self.context.cwd.join(path))?)
    }

    fn preview(&self, patch: &str) -> Option<String> {
        HashlineEditTool {
            state: std::sync::Arc::clone(&self.state),
            freeform_grammar: false,
        }
        .preview(&args(&[("patch", json!(patch))]), &self.context.cwd)
    }
}

#[test]
fn edit_preview_shows_the_diff_without_touching_disk_or_the_snapshot_store() -> TestResult {
    let fixture = Fixture::new("preview")?;
    fixture.write("a.txt", "one\ntwo\nthree\n")?;
    let tag = fixture.tag_of("a.txt")?;
    let patch = format!("[a.txt#{tag}]\nPUT 2.=2:\n+TWO\n");

    let preview = fixture.preview(&patch).ok_or("no preview")?;
    assert!(preview.contains("-two"), "{preview}");
    assert!(preview.contains("+TWO"), "{preview}");

    // The whole point: a previewed edit the user then denies changes nothing.
    assert_eq!(fixture.content("a.txt")?, "one\ntwo\nthree\n");

    // And the snapshot store is unspent, so the same tag still applies. If the
    // preview had recorded or invalidated a snapshot, this would fail stale.
    let edit = fixture.edit(&patch);
    assert!(!edit.is_error, "{}", output_text(&edit));
    assert_eq!(fixture.content("a.txt")?, "one\nTWO\nthree\n");
    Ok(())
}

/// The preview's patch reaches the permission ask; only the *result's* patch
/// reaches the transcript, so an applied edit with no `details.patch` renders
/// as a bare digest with no diff body.
#[test]
fn an_applied_edit_carries_its_patch_on_the_result() -> TestResult {
    let fixture = Fixture::new("result-patch")?;
    fixture.write("a.txt", "one\ntwo\nthree\n")?;
    let tag = fixture.tag_of("a.txt")?;
    let edit = fixture.edit(&format!("[a.txt#{tag}]\nPUT 2.=2:\n+TWO\n"));

    let patch = edit.result.details["patch"]
        .as_str()
        .ok_or("edit result carried no patch")?;
    assert!(patch.contains("-two"), "{patch}");
    assert!(patch.contains("+TWO"), "{patch}");
    assert_eq!(edit.result.details["added"], json!(1));
    assert_eq!(edit.result.details["removed"], json!(1));

    // A no-op edit has nothing to draw, so it must not claim a patch.
    let tag = fixture.tag_of("a.txt")?;
    let noop = fixture.edit(&format!("[a.txt#{tag}]\nPUT 2.=2:\n+TWO\n"));
    assert!(noop.result.details["patch"].is_null());
    Ok(())
}

#[test]
fn edit_preview_declines_a_noop_and_an_unparseable_patch() -> TestResult {
    let fixture = Fixture::new("preview-noop")?;
    fixture.write("a.txt", "one\ntwo\n")?;
    let tag = fixture.tag_of("a.txt")?;

    assert!(fixture.preview("not a patch at all").is_none());
    assert!(
        fixture
            .preview(&format!("[a.txt#{tag}]\nPUT 2.=2:\n+two\n"))
            .is_none()
    );
    Ok(())
}

#[test]
fn read_edit_round_trip_replaces_lines_and_mints_a_new_tag() -> TestResult {
    let fixture = Fixture::new("round-trip")?;
    fixture.write(
        "greet.py",
        "def greet(name):\n    msg = \"Hello, \" + name\n    print(msg)\ngreet(\"world\")\n",
    )?;
    let tag = fixture.tag_of("greet.py")?;

    let edit = fixture.edit(&format!(
        "[greet.py#{tag}]\nPUT 2.=3:\n+    print(f\"Hi, {{name}}\")\n"
    ));
    assert!(!edit.is_error, "{}", output_text(&edit));
    let text = output_text(&edit);
    assert!(text.contains("updated; first change at line 2"), "{text}");
    assert!(
        text.lines()
            .next()
            .is_some_and(|line| line.starts_with("[greet.py#")),
        "{text}"
    );

    let content = fixture.content("greet.py")?;
    assert_eq!(
        content,
        "def greet(name):\n    print(f\"Hi, {name}\")\ngreet(\"world\")\n"
    );
    let new_tag = fixture.tag_of("greet.py")?;
    assert_ne!(new_tag, tag);
    Ok(())
}

#[test]
fn stale_tag_rejects_with_the_ported_mismatch_header() -> TestResult {
    let fixture = Fixture::new("stale")?;
    fixture.write("a.txt", "one\ntwo\nthree\n")?;
    let tag = fixture.tag_of("a.txt")?;
    fixture.write("a.txt", "one\nTWO CHANGED\nthree\n")?;

    let edit = fixture.edit(&format!("[a.txt#{tag}]\nPUT 2.=2:\n+replacement\n"));
    assert!(edit.is_error);
    let text = output_text(&edit);
    assert!(
        text.contains("Edit rejected for a.txt: file changed between read and edit."),
        "{text}"
    );
    assert!(
        text.contains(&format!("Section is bound to #{tag}")),
        "{text}"
    );
    assert!(text.contains("*2:TWO CHANGED"), "{text}");
    Ok(())
}

#[test]
fn fabricated_tag_rejects_as_not_from_this_session() -> TestResult {
    let fixture = Fixture::new("fabricated")?;
    fixture.write("a.txt", "alpha\nbeta\n")?;
    let _ = fixture.tag_of("a.txt")?;

    let edit = fixture.edit("[a.txt#0000]\nPUT 1.=1:\n+new alpha\n");
    assert!(edit.is_error);
    let text = output_text(&edit);
    assert!(
        text.contains("hash #0000 is not from this session"),
        "{text}"
    );
    assert!(text.contains("never invent the tag"), "{text}");
    Ok(())
}

#[test]
fn missing_tag_rejects_with_teaching_text() -> TestResult {
    let fixture = Fixture::new("no-tag")?;
    fixture.write("a.txt", "alpha\n")?;
    let edit = fixture.edit("[a.txt]\nPUT 1.=1:\n+new\n");
    assert!(edit.is_error);
    let text = output_text(&edit);
    assert!(
        text.contains("No version of a.txt is on record for this session"),
        "{text}"
    );
    assert!(
        text.contains("[a.txt#") && text.contains("*1:alpha"),
        "{text}"
    );
    let header = text
        .split("[a.txt#")
        .nth(1)
        .and_then(|rest| rest.split(']').next())
        .ok_or("minted tag")?;
    let retry = fixture.edit(&format!("[a.txt#{header}]\nPUT 1.=1:\n+new\n"));
    assert!(!retry.is_error, "{}", output_text(&retry));
    assert_eq!(
        fs::read_to_string(fixture.context.cwd.join("a.txt"))?,
        "new\n"
    );
    Ok(())
}

/// The loop classifies a rejection by the message prefix; the pin keeps the two agreeing.
#[test]
fn a_rejected_edit_classifies_as_stale_tag() -> TestResult {
    let fixture = Fixture::new("stale-kind")?;
    fixture.write("a.txt", "alpha\n")?;
    let _ = fixture.tag_of("a.txt")?;
    let edit = fixture.edit("[a.txt#0000]\nPUT 1.=1:\n+new\n");
    assert!(edit.is_error);
    assert_eq!(
        edit.result.details["errorKind"], "stale_tag",
        "{:?}",
        edit.result.details
    );
    Ok(())
}

#[test]
fn seen_lines_guard_reveals_unseen_lines_then_allows_retry() -> TestResult {
    let fixture = Fixture::new("seen")?;
    let body: String = (1..=50).map(|n| format!("line {n}\n")).collect();
    fixture.write("big.txt", &body)?;
    let read = HashlineReadTool::new(std::sync::Arc::clone(&fixture.state)).execute(
        args(&[
            ("path", json!("big.txt")),
            ("offset", json!(1)),
            ("limit", json!(10)),
        ]),
        &fixture.context,
    );
    let text = output_text(&read);
    let tag = text
        .lines()
        .next()
        .and_then(|line| line.rsplit_once('#'))
        .map(|(_, tail)| tail.trim_end_matches(']').to_owned())
        .ok_or("no tag")?;

    let blind = fixture.edit(&format!("[big.txt#{tag}]\nPUT 30.=30:\n+changed line 30\n"));
    assert!(blind.is_error);
    let message = output_text(&blind);
    assert!(message.contains("never displayed"), "{message}");
    assert!(message.contains("30:line 30"), "{message}");

    let retry = fixture.edit(&format!("[big.txt#{tag}]\nPUT 30.=30:\n+changed line 30\n"));
    assert!(!retry.is_error, "{}", output_text(&retry));
    assert!(fixture.content("big.txt")?.contains("changed line 30"));
    Ok(())
}

#[test]
fn noop_edits_escalate_after_three_identical_repeats() -> TestResult {
    let fixture = Fixture::new("noop")?;
    fixture.write("a.txt", "same\n")?;
    let tag = fixture.tag_of("a.txt")?;
    let patch = format!("[a.txt#{tag}]\nPUT 1.=1:\n+same\n");

    let first = fixture.edit(&patch);
    assert!(!first.is_error);
    assert!(
        output_text(&first).contains("no changes"),
        "{}",
        output_text(&first)
    );
    let second = fixture.edit(&patch);
    assert!(!second.is_error);
    let third = fixture.edit(&patch);
    assert!(third.is_error);
    assert!(
        output_text(&third).contains("byte-identical no-op"),
        "{}",
        output_text(&third)
    );
    Ok(())
}

#[test]
fn cut_and_anonymous_paste_move_lines_within_a_file() -> TestResult {
    let fixture = Fixture::new("move-lines")?;
    fixture.write("list.txt", "a\nb\nc\nd\n")?;
    let tag = fixture.tag_of("list.txt")?;

    let edit = fixture.edit(&format!("[list.txt#{tag}]\nCUT 1.=1\nPUT >4\n"));
    assert!(!edit.is_error, "{}", output_text(&edit));
    assert_eq!(fixture.content("list.txt")?, "b\nc\nd\na\n");
    Ok(())
}

#[test]
fn named_register_moves_content_across_files() -> TestResult {
    let fixture = Fixture::new("cross-file")?;
    fixture.write("source.py", "def helper():\n    return 1\nkeep = True\n")?;
    fixture.write("dest.py", "existing = 0\n")?;
    let source_tag = fixture.tag_of("source.py")?;
    let dest_tag = fixture.tag_of("dest.py")?;

    let edit = fixture.edit(&format!(
        "[source.py#{source_tag}]\nCUT 1.=2 @fn\n[dest.py#{dest_tag}]\nPUT <1 @fn\n"
    ));
    assert!(!edit.is_error, "{}", output_text(&edit));
    assert_eq!(fixture.content("source.py")?, "keep = True\n");
    assert_eq!(
        fixture.content("dest.py")?,
        "def helper():\n    return 1\nexisting = 0\n"
    );
    Ok(())
}

/// A CUT into a named register answered only "updated": the model never saw what it captured,
/// and a later empty-register warning listed names with no sizes.
#[test]
fn a_named_cut_echoes_what_it_captured() -> TestResult {
    let fixture = Fixture::new("cut-echo")?;
    fixture.write("list.txt", "a\nbb\nc\nd\ne\n")?;
    let tag = fixture.tag_of("list.txt")?;
    let cut = fixture.edit(&format!("[list.txt#{tag}]\nCUT 2.=4 @x\n"));
    let text = output_text(&cut);
    assert!(!cut.is_error, "{text}");
    assert!(
        text.contains("`CUT 2.=4 @x` captured lines 2-4 (3 lines, 7 bytes)"),
        "{text}"
    );
    let tag = fixture.tag_of("list.txt")?;
    let paste = fixture.edit(&format!("[list.txt#{tag}]\nPUT >2 @nope\n"));
    let text = output_text(&paste);
    assert!(
        text.contains("Available registers: `@x` (3 lines)."),
        "{text}"
    );
    Ok(())
}

#[test]
fn block_replace_resolves_braces_without_tree_sitter() -> TestResult {
    let fixture = Fixture::new("block")?;
    fixture.write(
        "lib.rs",
        "fn alpha() {\n    old_one();\n    old_two();\n}\nfn beta() {}\n",
    )?;
    let tag = fixture.tag_of("lib.rs")?;

    let edit = fixture.edit(&format!(
        "[lib.rs#{tag}]\nPUT 1*:\n+fn alpha() {{\n+    fresh();\n+}}\n"
    ));
    assert!(!edit.is_error, "{}", output_text(&edit));
    assert_eq!(
        fixture.content("lib.rs")?,
        "fn alpha() {\n    fresh();\n}\nfn beta() {}\n"
    );
    Ok(())
}

#[test]
fn mv_renames_and_keeps_tags_valid_at_destination() -> TestResult {
    let fixture = Fixture::new("mv")?;
    fixture.write("old.txt", "content line\n")?;
    let tag = fixture.tag_of("old.txt")?;

    let edit = fixture.edit(&format!("[old.txt#{tag}]\nMV new.txt\n"));
    assert!(!edit.is_error, "{}", output_text(&edit));
    assert!(output_text(&edit).contains("moved to new.txt"));
    assert!(!fixture.context.cwd.join("old.txt").exists());
    assert_eq!(fixture.content("new.txt")?, "content line\n");
    Ok(())
}

#[test]
fn rem_deletes_the_file() -> TestResult {
    let fixture = Fixture::new("rem")?;
    fixture.write("doomed.txt", "bye\n")?;
    let tag = fixture.tag_of("doomed.txt")?;
    let edit = fixture.edit(&format!("[doomed.txt#{tag}]\nREM\n"));
    assert!(!edit.is_error, "{}", output_text(&edit));
    assert!(!fixture.context.cwd.join("doomed.txt").exists());
    Ok(())
}

#[test]
fn crlf_files_round_trip_their_line_endings() -> TestResult {
    let fixture = Fixture::new("crlf")?;
    fixture.write("dos.txt", "one\r\ntwo\r\nthree\r\n")?;
    let tag = fixture.tag_of("dos.txt")?;
    let edit = fixture.edit(&format!("[dos.txt#{tag}]\nPUT 2.=2:\n+TWO\n"));
    assert!(!edit.is_error, "{}", output_text(&edit));
    assert_eq!(fixture.content("dos.txt")?, "one\r\nTWO\r\nthree\r\n");
    Ok(())
}

#[test]
fn write_tool_records_a_snapshot_so_edit_follows_immediately() -> TestResult {
    let fixture = Fixture::new("write-then-edit")?;
    let write = WriteTool {
        hashline: Some(std::sync::Arc::clone(&fixture.state)),
    }
    .execute(
        args(&[
            ("path", json!("fresh.txt")),
            ("content", json!("alpha\nbeta\n")),
        ]),
        &fixture.context,
    );
    assert!(!write.is_error);
    let tag = fixture.tag_of("fresh.txt")?;
    let edit = fixture.edit(&format!("[fresh.txt#{tag}]\nPUT 1.=1:\n+ALPHA\n"));
    assert!(!edit.is_error, "{}", output_text(&edit));
    assert_eq!(fixture.content("fresh.txt")?, "ALPHA\nbeta\n");
    Ok(())
}

#[test]
fn wrong_directory_path_recovers_via_filename_and_tag() -> TestResult {
    let fixture = Fixture::new("path-recovery")?;
    fs::create_dir_all(fixture.context.cwd.join("src"))?;
    fixture.write("src/util.rs", "pub fn one() {}\n")?;
    let tag = fixture.tag_of("src/util.rs")?;

    let edit = fixture.edit(&format!("[util.rs#{tag}]\nPUT 1.=1:\n+pub fn two() {{}}\n"));
    assert!(!edit.is_error, "{}", output_text(&edit));
    let text = output_text(&edit);
    assert!(
        text.contains("does not exist; matched its filename and snapshot tag"),
        "{text}"
    );
    assert_eq!(fixture.content("src/util.rs")?, "pub fn two() {}\n");
    Ok(())
}

/// Recovery rebinds to a path the broker never judged, so the gates judge it here: a snapshot
/// of the workspace `.git` taken before a link swap must not become an edit target (D323).
#[test]
fn path_recovery_never_rebinds_to_a_guarded_path() -> TestResult {
    let fixture = Fixture::new("path-recovery-gate")?;
    fs::create_dir_all(fixture.context.cwd.join(".git"))?;
    fixture.write(".git/config", "[core]\n")?;
    // The read tool opens no `.git` file (#890), so the view a session holds is seeded here.
    let config = fixture.context.cwd.join(".git/config");
    let tag = yi_tools::hashline::tool::record_write_snapshot(&fixture.state, &config, "[core]\n");

    let edit = fixture.edit(&format!("[config#{tag}]\nPUT 1.=1:\n+hooked\n"));
    assert!(edit.is_error, "{}", output_text(&edit));
    assert_eq!(fixture.content(".git/config")?, "[core]\n");
    Ok(())
}

#[test]
fn symlink_targets_are_refused_at_commit() -> TestResult {
    let fixture = Fixture::new("symlink")?;
    fixture.write("real.txt", "content\n")?;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            fixture.context.cwd.join("real.txt"),
            fixture.context.cwd.join("link.txt"),
        )?;
        let tag = fixture.tag_of("link.txt")?;
        let edit = fixture.edit(&format!("[link.txt#{tag}]\nPUT 1.=1:\n+replaced\n"));
        assert!(edit.is_error);
        assert!(
            output_text(&edit).contains("symlink"),
            "{}",
            output_text(&edit)
        );
        assert_eq!(fixture.content("real.txt")?, "content\n");
    }
    Ok(())
}

#[test]
fn a_multi_hunk_edit_anchors_every_hunk_without_a_re_read() -> TestResult {
    let fixture = Fixture::new("multi-hunk")?;
    let body: String = (1..=12).map(|n| format!("line {n}\n")).collect();
    fixture.write("a.txt", &body)?;
    let tag = fixture.tag_of("a.txt")?;

    let edit = fixture.edit(&format!(
        "[a.txt#{tag}]\nPUT 2.=2:\n+SECOND\nPUT 10.=10:\n+TENTH\n"
    ));
    assert!(!edit.is_error, "{}", output_text(&edit));
    let text = output_text(&edit);

    // Both hunks get a window. Before this, only the first did.
    assert!(text.contains("2:SECOND"), "{text}");
    assert!(text.contains("10:TENTH"), "{text}");
    assert!(text.contains("13:"), "{text}");

    // The contract a consumer sees: the second hunk is anchorable straight
    // away. Without its window the seen-lines guard rejects this and the model
    // has to re-read the whole file.
    let new_tag = text
        .lines()
        .next()
        .and_then(|header| header.rsplit_once('#'))
        .map(|(_, tail)| tail.trim_end_matches(']').to_owned())
        .ok_or("no tag in edit header")?;
    let follow_up = fixture.edit(&format!("[a.txt#{new_tag}]\nPUT 10.=10:\n+TENTH AGAIN\n"));
    assert!(!follow_up.is_error, "{}", output_text(&follow_up));
    assert!(fixture.content("a.txt")?.contains("TENTH AGAIN"));
    Ok(())
}

#[test]
fn numbers_from_an_older_read_land_after_an_insert_above_them() -> TestResult {
    let fixture = Fixture::new("rebase-moved")?;
    fixture.write("a.txt", "one\ntwo\nthree\nfour\n")?;
    let old_tag = fixture.tag_of("a.txt")?;
    let first = fixture.edit(&format!("[a.txt#{old_tag}]\nPUT <1:\n+zero\n"));
    assert!(!first.is_error, "{}", output_text(&first));

    let second = fixture.edit(&format!("[a.txt#{old_tag}]\nPUT 3.=3:\n+THREE\n"));
    let text = output_text(&second);
    assert!(!second.is_error, "{text}");
    assert!(text.contains("rebased #"), "{text}");
    assert_eq!(fixture.content("a.txt")?, "zero\none\ntwo\nTHREE\nfour\n");
    Ok(())
}

#[test]
fn a_cited_line_that_changed_since_the_read_is_rejected_and_nothing_is_written() -> TestResult {
    let fixture = Fixture::new("rebase-changed")?;
    fixture.write("a.txt", "one\ntwo\nthree\nfour\n")?;
    let old_tag = fixture.tag_of("a.txt")?;
    let first = fixture.edit(&format!("[a.txt#{old_tag}]\nPUT 3.=3:\n+drei\n"));
    assert!(!first.is_error, "{}", output_text(&first));

    let second = fixture.edit(&format!("[a.txt#{old_tag}]\nPUT 3.=3:\n+THREE\n"));
    let text = output_text(&second);
    assert!(second.is_error, "{text}");
    assert!(
        text.contains("drei"),
        "the rejection shows the current line: {text}"
    );
    assert_eq!(fixture.content("a.txt")?, "one\ntwo\ndrei\nfour\n");
    Ok(())
}

#[test]
fn a_header_without_a_tag_means_the_version_last_shown() -> TestResult {
    let fixture = Fixture::new("rebase-notag")?;
    fixture.write("a.txt", "one\ntwo\nthree\n")?;
    let _ = fixture.tag_of("a.txt")?;
    let edit = fixture.edit("[a.txt]\nPUT 2.=2:\n+TWO\n");
    assert!(!edit.is_error, "{}", output_text(&edit));
    assert_eq!(fixture.content("a.txt")?, "one\nTWO\nthree\n");

    fixture.write("b.txt", "never shown\n")?;
    let blind = fixture.edit("[b.txt]\nPUT 1.=1:\n+x\n");
    assert!(blind.is_error, "{}", output_text(&blind));
    Ok(())
}

#[test]
fn a_block_op_rebases_only_when_the_whole_old_block_is_unchanged() -> TestResult {
    let fixture = Fixture::new("rebase-block")?;
    fixture.write("a.rs", "fn a() {\n    1\n}\nfn b() {\n    2\n}\n")?;
    let old_tag = fixture.tag_of("a.rs")?;
    let first = fixture.edit(&format!("[a.rs#{old_tag}]\nPUT <1:\n+// head\n"));
    assert!(!first.is_error, "{}", output_text(&first));

    let moved = fixture.edit(&format!(
        "[a.rs#{old_tag}]\nPUT 4*:\n+fn b() {{\n+    3\n+}}\n"
    ));
    assert!(!moved.is_error, "{}", output_text(&moved));
    assert_eq!(
        fixture.content("a.rs")?,
        "// head\nfn a() {\n    1\n}\nfn b() {\n    3\n}\n"
    );

    let stale = fixture.edit(&format!(
        "[a.rs#{old_tag}]\nPUT 4*:\n+fn b() {{\n+    4\n+}}\n"
    ));
    assert!(stale.is_error, "{}", output_text(&stale));
    Ok(())
}

#[test]
fn a_block_op_on_a_python_file_replaces_the_indented_body() -> TestResult {
    let fixture = Fixture::new("indent-block")?;
    fixture.write(
        "a.py",
        "def a():\n    return 1\n\n\ndef b():\n    return 2\n",
    )?;
    let tag = fixture.tag_of("a.py")?;
    let edit = fixture.edit(&format!(
        "[a.py#{tag}]\nPUT 1*:\n+def a():\n+    return 10\n"
    ));
    assert!(!edit.is_error, "{}", output_text(&edit));
    assert_eq!(
        fixture.content("a.py")?,
        "def a():\n    return 10\n\n\ndef b():\n    return 2\n"
    );
    Ok(())
}

#[test]
fn an_edit_to_a_charted_file_carries_a_named_grid_layer() -> TestResult {
    let fixture = Fixture::new("grid-layer")?;
    fs::create_dir_all(fixture.context.cwd.join(".grid"))?;
    fixture.write("a.rs", "fn a() {}\n")?;
    let tag = fixture.tag_of("a.rs")?;
    let edit = fixture.edit(&format!("[a.rs#{tag}]\nPUT 1.=1:\n+fn a() {{ 1 }}\n"));
    let text = output_text(&edit);
    assert!(!edit.is_error, "{text}");
    assert!(
        text.contains("[grid check"),
        "the layer is named either way: {text}"
    );
    assert!(
        edit.result.details["grid"].is_string(),
        "{}",
        edit.result.details
    );

    fixture.write("b.txt", "one\n")?;
    let tag = fixture.tag_of("b.txt")?;
    let plain = fixture.edit(&format!("[b.txt#{tag}]\nPUT 1.=1:\n+two\n"));
    assert!(!output_text(&plain).contains("[grid check"));
    assert_eq!(plain.result.details["grid"], json!("skipped"));
    Ok(())
}

/// The header from the #481 confirmation run: one stray `:` between the range halves.
#[test]
fn a_one_mark_near_miss_header_applies_and_names_the_repair() -> TestResult {
    let fixture = Fixture::new("near-miss")?;
    let body: String = (1..=45).map(|n| format!("row {n}\n")).collect();
    fixture.write("rows.txt", &body)?;
    let tag = fixture.tag_of("rows.txt")?;
    let edit = fixture.edit(&format!("[rows.txt#{tag}]\nPUT 40.:=40:\n+row forty\n"));
    assert!(!edit.is_error, "{}", output_text(&edit));
    let text = output_text(&edit);
    assert!(
        text.contains("read the hunk header `PUT 40.:=40:` as `PUT 40.=40:`"),
        "{text}"
    );
    let content = fixture.content("rows.txt")?;
    assert!(content.contains("row 39\nrow forty\nrow 41\n"), "{content}");
    Ok(())
}

/// Dropping one `.` of `PUT 4.5.:` reads lines 4-5, dropping the other reads line 45.
#[test]
fn an_ambiguous_near_miss_header_is_still_refused() -> TestResult {
    let fixture = Fixture::new("near-miss-ambiguous")?;
    fixture.write("rows.txt", "a\nb\nc\nd\ne\nf\n")?;
    let tag = fixture.tag_of("rows.txt")?;
    let edit = fixture.edit(&format!("[rows.txt#{tag}]\nPUT 4.5.:\n+x\n"));
    assert!(edit.is_error, "{}", output_text(&edit));
    assert!(
        output_text(&edit).contains("no preceding hunk header"),
        "{}",
        output_text(&edit)
    );
    assert_eq!(fixture.content("rows.txt")?, "a\nb\nc\nd\ne\nf\n");
    Ok(())
}

/// All four `never displayed` refusals in the confirmation corpora cited lines past the end of
/// a file the model had just written in full; the refusal must name the length instead.
#[test]
fn an_anchor_past_the_end_is_not_called_unseen() -> TestResult {
    let fixture = Fixture::new("past-eof")?;
    let content: String = (1..=135).map(|n| format!("line {n}\n")).collect();
    let write = WriteTool {
        hashline: Some(std::sync::Arc::clone(&fixture.state)),
    }
    .execute(
        args(&[("path", json!("rot.py")), ("content", json!(content))]),
        &fixture.context,
    );
    assert!(!write.is_error, "{}", output_text(&write));
    let tag = output_text(&write)
        .lines()
        .next()
        .and_then(|line| line.rsplit_once('#'))
        .map(|(_, tail)| tail.trim_end_matches(']').to_owned())
        .ok_or("no tag on the write result")?;
    let edit = fixture.edit(&format!("[rot.py#{tag}]\nPUT 134.=148:\n+x\n"));
    assert!(edit.is_error);
    let text = output_text(&edit);
    assert!(!text.contains("never displayed"), "{text}");
    assert!(text.contains("136 lines"), "{text}");
    Ok(())
}

/// A bare body row `REM.` in a batch file is one mark from `REM`, which deletes the file:
/// the refusal names the `+` form and never reads the row as an op.
#[test]
fn a_near_miss_never_reads_as_a_file_op() -> TestResult {
    let fixture = Fixture::new("near-miss-rem")?;
    fixture.write("run.bat", "@echo off\nexit\n")?;
    let tag = fixture.tag_of("run.bat")?;
    let edit = fixture.edit(&format!("[run.bat#{tag}]\nPUT 1:\n+@echo off\nREM.\n"));
    let text = output_text(&edit);
    assert!(edit.is_error, "{text}");
    assert!(
        text.contains("line 3:") && text.contains("`+TEXT`"),
        "{text}"
    );
    assert_eq!(fixture.content("run.bat")?, "@echo off\nexit\n");
    Ok(())
}

/// `PUT >N*:` on a bare inner line landed after that line with a warning; the docs call it
/// WRONG. The closer case lowers to the form the docs call RIGHT and keeps its warning.
#[test]
fn an_insert_after_block_on_a_non_opener_is_refused() -> TestResult {
    let fixture = Fixture::new("after-block-non-opener")?;
    fixture.write("a.rs", "fn a() {\n    1\n}\nfn b() {\n    2\n}\n")?;
    let tag = fixture.tag_of("a.rs")?;
    let inner = fixture.edit(&format!("[a.rs#{tag}]\nPUT >2*:\n+// x\n"));
    let text = output_text(&inner);
    assert!(inner.is_error, "{text}");
    assert!(text.contains("OPENS"), "{text}");
    assert_eq!(
        fixture.content("a.rs")?,
        "fn a() {\n    1\n}\nfn b() {\n    2\n}\n"
    );

    let closer = fixture.edit(&format!("[a.rs#{tag}]\nPUT >3*:\n+// x\n"));
    let text = output_text(&closer);
    assert!(!closer.is_error, "{text}");
    assert!(text.contains("applied as plain `PUT >3:`"), "{text}");
    assert_eq!(
        fixture.content("a.rs")?,
        "fn a() {\n    1\n}\n// x\nfn b() {\n    2\n}\n"
    );
    Ok(())
}

fn ten_lines() -> String {
    (1..=10).map(|n| format!("line {n}\n")).collect()
}

/// An unknown-tag rejection displayed lines 1-3 and recorded the live file with no seen set,
/// so the next untagged edit could change line 7 blind.
#[test]
fn an_unknown_tag_rejection_displays_only_its_anchored_lines() -> TestResult {
    let fixture = Fixture::new("mismatch-seen")?;
    fixture.write("a.txt", &ten_lines())?;
    let rejected = fixture.edit("[a.txt#0000]\nPUT 1.=1:\n+one\n");
    assert!(rejected.is_error, "{}", output_text(&rejected));

    let blind = fixture.edit("[a.txt]\nPUT 7.=7:\n+seven\n");
    let text = output_text(&blind);
    assert!(blind.is_error, "{text}");
    assert!(text.contains("never displayed"), "{text}");
    assert!(text.contains("7:line 7"), "{text}");
    assert_eq!(fixture.content("a.txt")?, ten_lines());
    Ok(())
}

/// A read that shows no rows recorded an empty seen set, which the guard read as unrestricted.
#[test]
fn a_read_that_shows_no_rows_displays_nothing() -> TestResult {
    let fixture = Fixture::new("empty-view-seen")?;
    fixture.write("a.txt", &ten_lines())?;
    let read = HashlineReadTool::new(std::sync::Arc::clone(&fixture.state)).execute(
        args(&[("path", json!("a.txt")), ("limit", json!(0))]),
        &fixture.context,
    );
    assert!(!read.is_error, "{}", output_text(&read));

    let blind = fixture.edit("[a.txt]\nPUT 7.=7:\n+seven\n");
    let text = output_text(&blind);
    assert!(blind.is_error, "{text}");
    assert!(text.contains("never displayed"), "{text}");
    assert_eq!(fixture.content("a.txt")?, ten_lines());
    Ok(())
}

/// A grep replace with `apply` shows a diff of the changed lines, yet marked every line seen.
#[test]
fn a_grep_apply_displays_only_the_lines_its_diff_shows() -> TestResult {
    let fixture = Fixture::new("grep-apply-seen")?;
    fixture.write("a.txt", &ten_lines())?;
    let applied = yi_tools::GrepTool {
        hashline: Some(std::sync::Arc::clone(&fixture.state)),
    }
    .execute(
        args(&[
            ("pattern", json!("line 2")),
            ("replace", json!("LINE 2")),
            ("apply", json!(true)),
        ]),
        &fixture.context,
    );
    assert!(!applied.is_error, "{}", output_text(&applied));

    let near = fixture.edit("[a.txt]\nPUT 4.=4:\n+four\n");
    assert!(!near.is_error, "{}", output_text(&near));
    let blind = fixture.edit("[a.txt]\nPUT 9.=9:\n+nine\n");
    let text = output_text(&blind);
    assert!(blind.is_error, "{text}");
    assert!(text.contains("never displayed"), "{text}");
    Ok(())
}

/// The header a refusal or a read minted for `path`: the first `[path#TAG]` in its text.
fn minted_tag(text: &str, path: &str) -> Result<String, Box<dyn Error>> {
    Ok(text
        .split(&format!("[{path}#"))
        .nth(1)
        .and_then(|rest| rest.split(']').next())
        .ok_or("no minted header in the text")?
        .to_owned())
}

/// Rows of the `N:TEXT` shape a refusal prints, whatever their marker.
fn numbered_rows(text: &str) -> usize {
    text.lines()
        .filter(|line| {
            line.get(1..)
                .and_then(|rest| rest.split(':').next())
                .is_some_and(|number| {
                    !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
                })
        })
        .count()
}

/// `.gitignore` as `cargo new` writes it, never shown this session. A tail insert anchors on
/// no line, so a tag nobody minted used to land it with only a drift warning.
#[test]
fn a_tail_insert_under_a_tag_never_minted_is_refused_then_lands_with_the_minted_one() -> TestResult
{
    let fixture = Fixture::new("gate-tail")?;
    fixture.write(".gitignore", "/target\n")?;
    let blind = fixture.edit("[.gitignore#0000]\nPUT >$:\n+/dist\n");
    let text = output_text(&blind);
    assert!(blind.is_error, "{text}");
    assert!(
        text.contains("hash #0000 is not from this session"),
        "{text}"
    );
    assert!(text.contains("1:/target"), "{text}");
    assert!(!text.lines().any(|line| line == " 2:"), "{text}");
    assert_eq!(fixture.content(".gitignore")?, "/target\n");

    let tag = minted_tag(&text, ".gitignore")?;
    let retry = fixture.edit(&format!("[.gitignore#{tag}]\nPUT >$:\n+/dist\n"));
    assert!(!retry.is_error, "{}", output_text(&retry));
    assert_eq!(fixture.content(".gitignore")?, "/target\n/dist\n");
    Ok(())
}

/// `Cargo.toml` as `cargo new --lib` (cargo 1.94) writes it.
const CARGO_TOML: &str = "[package]\nname = \"probe-crate\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\n";

/// Read through one state, edited through a fresh one with that tag, as a resumed session
/// does: the tag matches the file on disk, but nothing this session showed it.
#[test]
fn a_tag_from_a_prior_session_is_refused_until_this_one_shows_the_lines() -> TestResult {
    let fixture = Fixture::new("gate-resumed")?;
    fixture.write("Cargo.toml", CARGO_TOML)?;
    let tag = fixture.tag_of("Cargo.toml")?;
    let resumed = Fixture {
        state: shared_hashline_state(),
        ..fixture
    };
    let patch = format!("[Cargo.toml#{tag}]\nPUT 3.=3:\n+version = \"0.2.0\"\n");
    let blind = resumed.edit(&patch);
    let text = output_text(&blind);
    assert!(blind.is_error, "{text}");
    assert!(
        text.contains("No version of Cargo.toml is on record for this session"),
        "{text}"
    );
    assert!(text.contains("*3:version = \"0.1.0\""), "{text}");
    assert_eq!(resumed.content("Cargo.toml")?, CARGO_TOML);

    let retry = resumed.edit(&patch);
    assert!(!retry.is_error, "{}", output_text(&retry));
    assert!(
        resumed
            .content("Cargo.toml")?
            .contains("version = \"0.2.0\"")
    );
    Ok(())
}

/// `LICENSE-APACHE` as serde 1.0.219 ships it (176 lines), never read. `PUT 1.=100:` names
/// more lines than one refusal shows; the reveal used to print all 102 and unlock the retry.
#[test]
fn a_reveal_past_forty_rows_unlocks_nothing_and_names_the_ranged_read() -> TestResult {
    let fixture = Fixture::new("gate-cap")?;
    let license = include_str!("fixtures/gate/LICENSE-APACHE");
    fixture.write("LICENSE-APACHE", license)?;
    let patch = |tag: &str| format!("[LICENSE-APACHE{tag}]\nPUT 1.=100:\n+stub\n");
    let blind = fixture.edit(&patch(""));
    let text = output_text(&blind);
    assert!(blind.is_error, "{text}");
    assert_eq!(numbered_rows(&text), 40, "{text}");
    let tag = minted_tag(&text, "LICENSE-APACHE")?;

    let retry = fixture.edit(&patch(&format!("#{tag}")));
    let text = output_text(&retry);
    assert!(retry.is_error, "{text}");
    // The remedy is one `read` accepts: `ranges=`, never the `path:selector` form.
    assert!(text.contains("ranges=[[1,100]]"), "{text}");
    assert!(!text.contains("LICENSE-APACHE:1"), "{text}");
    assert_eq!(fixture.content("LICENSE-APACHE")?, license);

    let read = HashlineReadTool::new(std::sync::Arc::clone(&fixture.state)).execute(
        args(&[
            ("path", json!("LICENSE-APACHE")),
            ("ranges", json!([[1, 100]])),
        ]),
        &fixture.context,
    );
    let tag = minted_tag(&output_text(&read), "LICENSE-APACHE")?;
    let landed = fixture.edit(&patch(&format!("#{tag}")));
    assert!(!landed.is_error, "{}", output_text(&landed));
    Ok(())
}

/// The runtime previews an edit before it executes it. A preview that recorded its refusal's
/// reveal as displayed let the first execute land blind, with no refusal ever shown.
#[test]
fn a_preview_of_a_never_read_file_unlocks_nothing_for_the_execute() -> TestResult {
    let fixture = Fixture::new("preview-never-read")?;
    fixture.write("a.txt", &ten_lines())?;
    let patch = "[a.txt]\nPUT 7.=7:\n+seven\n";
    assert!(fixture.preview(patch).is_none());
    let edit = fixture.edit(patch);
    let text = output_text(&edit);
    assert!(edit.is_error, "{text}");
    assert!(text.contains("is on record for this session"), "{text}");
    assert_eq!(fixture.content("a.txt")?, ten_lines());
    Ok(())
}

#[test]
fn a_preview_under_a_prior_sessions_tag_unlocks_nothing_for_the_execute() -> TestResult {
    let fixture = Fixture::new("preview-resumed")?;
    fixture.write("Cargo.toml", CARGO_TOML)?;
    let tag = fixture.tag_of("Cargo.toml")?;
    let resumed = Fixture {
        state: shared_hashline_state(),
        ..fixture
    };
    let patch = format!("[Cargo.toml#{tag}]\nPUT 3.=3:\n+version = \"0.2.0\"\n");
    assert!(resumed.preview(&patch).is_none());
    let edit = resumed.edit(&patch);
    assert!(edit.is_error, "{}", output_text(&edit));
    assert_eq!(resumed.content("Cargo.toml")?, CARGO_TOML);
    Ok(())
}

#[test]
fn a_preview_of_an_unseen_line_unlocks_nothing_for_the_execute() -> TestResult {
    let fixture = Fixture::new("preview-unseen")?;
    let body: String = (1..=50).map(|n| format!("line {n}\n")).collect();
    fixture.write("big.txt", &body)?;
    let read = HashlineReadTool::new(std::sync::Arc::clone(&fixture.state)).execute(
        args(&[
            ("path", json!("big.txt")),
            ("offset", json!(1)),
            ("limit", json!(10)),
        ]),
        &fixture.context,
    );
    let tag = minted_tag(&output_text(&read), "big.txt")?;
    let patch = format!("[big.txt#{tag}]\nPUT 30.=30:\n+changed\n");
    assert!(fixture.preview(&patch).is_none());
    let edit = fixture.edit(&patch);
    let text = output_text(&edit);
    assert!(edit.is_error, "{text}");
    assert!(text.contains("never displayed"), "{text}");
    assert_eq!(fixture.content("big.txt")?, body);
    Ok(())
}

/// A line wider than 512 columns is clipped by `read` and by every refusal, so neither marks
/// it seen; the refusal names a remedy scoped to this file, not the read that would loop.
#[test]
fn a_clipped_anchor_names_a_remedy_scoped_to_the_file() -> TestResult {
    let fixture = Fixture::new("gate-wide")?;
    let wide = format!("{}TOKEN{}", "x".repeat(300), "y".repeat(300));
    let body = format!("one\ntwo\n{wide}\nfour\n");
    fixture.write("wide.txt", &body)?;
    // Siblings the pattern also matches: an unscoped remedy would rewrite them too.
    fixture.write("other.txt", "keep TOKEN here\n")?;
    fixture.write("wide.txt.bak", &body)?;
    let read = HashlineReadTool::new(std::sync::Arc::clone(&fixture.state)).execute(
        args(&[("path", json!("wide.txt")), ("limit", json!(1))]),
        &fixture.context,
    );
    let tag = minted_tag(&output_text(&read), "wide.txt")?;
    let on_wide = fixture.edit(&format!("[wide.txt#{tag}]\nPUT 3.=3:\n+three\n"));
    let text = output_text(&on_wide);
    assert!(on_wide.is_error, "{text}");
    assert!(
        text.contains("Line(s) 3 exceed 512 columns") && text.contains("path=wide.txt"),
        "{text}"
    );
    assert!(!text.contains("ranges="), "{text}");
    // The clipped row blocked only itself: line 4, shown whole beside it, now anchors.
    let beside = fixture.edit(&format!("[wide.txt#{tag}]\nPUT 4.=4:\n+FOUR\n"));
    assert!(!beside.is_error, "{}", output_text(&beside));
    // The named remedy, run as worded: `replace` on this file shows the rewrite and writes
    // nothing, then `apply` writes it, and only wide.txt changes.
    let grep = |apply: bool| {
        yi_tools::GrepTool {
            hashline: Some(std::sync::Arc::clone(&fixture.state)),
        }
        .execute(
            args(&[
                ("pattern", json!("TOKEN")),
                ("path", json!("wide.txt")),
                ("replace", json!("token")),
                ("apply", json!(apply)),
            ]),
            &fixture.context,
        )
    };
    let previewed = grep(false);
    assert!(!previewed.is_error, "{}", output_text(&previewed));
    assert!(fixture.content("wide.txt")?.contains("TOKEN"));
    let applied = grep(true);
    assert!(!applied.is_error, "{}", output_text(&applied));
    assert_eq!(
        fixture.content("wide.txt")?,
        body.replace("TOKEN", "token").replace("four", "FOUR")
    );
    assert_eq!(fixture.content("other.txt")?, "keep TOKEN here\n");
    assert_eq!(fixture.content("wide.txt.bak")?, body);
    Ok(())
}

/// A file op names no line, so its refusal has no rows to show; it still carries the minted
/// header, and never the past-the-end sentence with an empty range.
#[test]
fn a_file_op_on_a_never_read_file_is_refused_with_the_header_then_lands() -> TestResult {
    let fixture = Fixture::new("gate-file-op")?;
    fixture.write("a.txt", "alpha\nbeta\n")?;
    let moved = fixture.edit("[a.txt]\nMV b.txt\n");
    let text = output_text(&moved);
    assert!(moved.is_error, "{text}");
    assert!(!text.contains("past the end"), "{text}");
    assert!(text.contains("then re-issue with this header"), "{text}");
    let tag = minted_tag(&text, "a.txt")?;
    let retry = fixture.edit(&format!("[a.txt#{tag}]\nMV b.txt\n"));
    assert!(!retry.is_error, "{}", output_text(&retry));
    assert_eq!(fixture.content("b.txt")?, "alpha\nbeta\n");

    fixture.write("c.txt", "gamma\n")?;
    let removed = fixture.edit("[c.txt#0000]\nREM\n");
    let text = output_text(&removed);
    assert!(removed.is_error, "{text}");
    assert!(text.contains("not from this session"), "{text}");
    assert!(!text.contains("past the end"), "{text}");
    let tag = minted_tag(&text, "c.txt")?;
    let retry = fixture.edit(&format!("[c.txt#{tag}]\nREM\n"));
    assert!(!retry.is_error, "{}", output_text(&retry));
    assert!(!fixture.context.cwd.join("c.txt").exists());
    Ok(())
}

/// An anchor past the end shows nothing; the refusal says so instead of promising a retry.
#[test]
fn an_anchor_past_the_end_of_a_never_read_file_names_the_length() -> TestResult {
    let fixture = Fixture::new("gate-past-eof")?;
    fixture.write("a.txt", &ten_lines())?;
    let edit = fixture.edit("[a.txt]\nPUT 500.=500:\n+x\n");
    let text = output_text(&edit);
    assert!(edit.is_error, "{text}");
    assert!(
        text.contains("Lines 500 are past the end (the file has 10 lines)"),
        "{text}"
    );
    assert!(!text.contains("re-issue with this header"), "{text}");
    Ok(())
}
