use std::error::Error;
use std::fs;
use std::path::PathBuf;

use serde_json::{Map, Value, json};
use yi_tools::hashline::tool::{
    HashlineEditTool, HashlineReadTool, SharedHashline, shared_hashline_state,
};
use yi_tools::{Tool, ToolContext, ToolOutput, WriteTool};
use yi_types::message::Content;

type TestResult = Result<(), Box<dyn Error>>;

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp_dir(tag: &str) -> Result<TempDir, Box<dyn Error>> {
    let dir = std::env::temp_dir().join(format!("yi-hashline-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    Ok(TempDir(dir))
}

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
    _dir: TempDir,
    context: ToolContext,
    state: SharedHashline,
}

impl Fixture {
    fn new(tag: &str) -> Result<Self, Box<dyn Error>> {
        let dir = temp_dir(tag)?;
        let context = ToolContext::new(dir.0.clone());
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
        HashlineReadTool {
            state: std::sync::Arc::clone(&self.state),
        }
        .execute(args(&[("path", json!(path))]), &self.context)
    }

    fn edit(&self, patch: &str) -> ToolOutput {
        HashlineEditTool {
            state: std::sync::Arc::clone(&self.state),
            freeform_grammar: false,
        }
        .execute(args(&[("patch", json!(patch))]), &self.context)
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
fn stale_tag_rejects_with_the_omp_mismatch_header() -> TestResult {
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
        text.contains("Missing hashline snapshot tag for a.txt"),
        "{text}"
    );
    Ok(())
}

#[test]
fn seen_lines_guard_reveals_unseen_lines_then_allows_retry() -> TestResult {
    let fixture = Fixture::new("seen")?;
    let body: String = (1..=50).map(|n| format!("line {n}\n")).collect();
    fixture.write("big.txt", &body)?;
    let read = HashlineReadTool {
        state: std::sync::Arc::clone(&fixture.state),
    }
    .execute(
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
