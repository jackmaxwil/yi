//! Every fixture under `fixtures/endings/` is bytes as its producer wrote them: `pip freeze`
//! on Windows plus a Git Bash `echo >>`, Excel's "CSV (Macintosh)", `dotnet new sln`, a
//! Latin-1 editor, `cargo new --lib`. Each test copies one into a scratch directory, edits
//! one line through the tool, and expects the file back with only that line's bytes changed.
use std::error::Error;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use yi_tools::hashline::tool::{
    HashlineEditTool, HashlineReadTool, SharedHashline, shared_hashline_state,
};
use yi_tools::{GrepTool, Tool, ToolContext, ToolOutput};
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

fn fixture(name: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/endings")
        .join(name);
    Ok(fs::read(path)?)
}

/// The fixture with `from` swapped for `to`; `from` must occur once, so the expectation is
/// one line's bytes and nothing else.
fn with_one_line_changed(name: &str, from: &[u8], to: &[u8]) -> Result<Vec<u8>, Box<dyn Error>> {
    let bytes = fixture(name)?;
    let hits: Vec<usize> = bytes
        .windows(from.len())
        .enumerate()
        .filter(|(_, window)| *window == from)
        .map(|(index, _)| index)
        .collect();
    let [at] = hits[..] else {
        return Err(format!("{name}: {} occurrences of the edited text", hits.len()).into());
    };
    let mut out = bytes[..at].to_vec();
    out.extend_from_slice(to);
    out.extend_from_slice(&bytes[at + from.len()..]);
    Ok(out)
}

fn assert_bytes(actual: &[u8], expected: &[u8]) {
    assert!(
        actual == expected,
        "\n     got {}\nexpected {}",
        actual.escape_ascii(),
        expected.escape_ascii()
    );
}

/// The `TAG` of the first `[path#TAG]` in `text`.
fn tag_in(text: &str) -> Option<String> {
    let (_, tail) = text.split_once('#')?;
    let (tag, _) = tail.split_once(']')?;
    Some(tag.to_owned())
}

struct Lab {
    _dir: Scratch,
    context: ToolContext,
    state: SharedHashline,
}

impl Lab {
    fn with(tag: &str, name: &str) -> Result<Self, Box<dyn Error>> {
        let dir = Scratch::new(&format!("yi-endings-{tag}"))?;
        fs::write(dir.join(name), fixture(name)?)?;
        let context = ToolContext::new(dir.to_path_buf());
        Ok(Self {
            _dir: dir,
            context,
            state: shared_hashline_state(),
        })
    }

    fn bytes(&self, name: &str) -> Result<Vec<u8>, Box<dyn Error>> {
        Ok(fs::read(self.context.cwd.join(name))?)
    }

    fn tag_of(&self, name: &str) -> Result<String, Box<dyn Error>> {
        let read = HashlineReadTool::new(Arc::clone(&self.state))
            .execute(args(&[("path", json!(name))]), &self.context);
        tag_in(&output_text(&read)).ok_or_else(|| "no tag in the read header".into())
    }

    fn edit(&self, patch: &str) -> ToolOutput {
        HashlineEditTool {
            state: Arc::clone(&self.state),
            freeform_grammar: false,
        }
        .execute(args(&[("patch", json!(patch))]), &self.context)
    }

    fn replace(&self, pattern: &str, replacement: &str) -> ToolOutput {
        GrepTool {
            hashline: Some(Arc::clone(&self.state)),
        }
        .execute(
            args(&[
                ("pattern", json!(pattern)),
                ("replace", json!(replacement)),
                ("apply", json!(true)),
            ]),
            &self.context,
        )
    }
}

#[test]
fn a_pip_freeze_file_with_a_git_bash_line_keeps_every_lines_ending() -> TestResult {
    let lab = Lab::with("pip", "requirements.txt")?;
    let tag = lab.tag_of("requirements.txt")?;
    let edit = lab.edit(&format!(
        "[requirements.txt#{tag}]\nPUT 4.=4:\n+requests==2.32.4\n"
    ));
    assert!(!edit.is_error, "{}", output_text(&edit));
    assert_bytes(
        &lab.bytes("requirements.txt")?,
        &with_one_line_changed("requirements.txt", b"requests==2.32.3", b"requests==2.32.4")?,
    );
    Ok(())
}

#[test]
fn an_excel_macintosh_csv_keeps_its_lone_cr_endings() -> TestResult {
    let lab = Lab::with("csv", "prices.csv")?;
    let tag = lab.tag_of("prices.csv")?;
    let edit = lab.edit(&format!("[prices.csv#{tag}]\nPUT 3.=3:\n+Pear,0.99\n"));
    assert!(!edit.is_error, "{}", output_text(&edit));
    assert_bytes(
        &lab.bytes("prices.csv")?,
        &with_one_line_changed("prices.csv", b"Pear,0.95", b"Pear,0.99")?,
    );
    Ok(())
}

#[test]
fn a_dotnet_sln_keeps_its_bom_and_crlf() -> TestResult {
    let lab = Lab::with("sln", "demo.sln")?;
    let tag = lab.tag_of("demo.sln")?;
    let edit = lab.edit(&format!(
        "[demo.sln#{tag}]\nPUT 3.=3:\n+# Visual Studio Version 18\n"
    ));
    assert!(!edit.is_error, "{}", output_text(&edit));
    assert_bytes(
        &lab.bytes("demo.sln")?,
        &with_one_line_changed("demo.sln", b"Version 17", b"Version 18")?,
    );
    Ok(())
}

#[test]
fn grep_replace_keeps_every_lines_ending() -> TestResult {
    for (name, pattern, from, to) in [
        (
            "requirements.txt",
            r"requests==2\.32\.3",
            "requests==2.32.3",
            "requests==2.32.4",
        ),
        ("prices.csv", r"0\.95", "0.95", "0.99"),
        ("demo.sln", "Version 17", "Version 17", "Version 18"),
    ] {
        let lab = Lab::with("grep", name)?;
        let applied = lab.replace(pattern, to);
        let text = output_text(&applied);
        assert!(text.contains("applied to 1 of 1 files"), "{name}: {text}");
        assert_bytes(
            &lab.bytes(name)?,
            &with_one_line_changed(name, from.as_bytes(), to.as_bytes())?,
        );
    }
    Ok(())
}

#[test]
fn grep_replace_skips_a_latin_1_file_and_names_it() -> TestResult {
    let lab = Lab::with("latin1", "latin1.txt")?;
    let applied = lab.replace("old_name", "new_name");
    assert_bytes(&lab.bytes("latin1.txt")?, &fixture("latin1.txt")?);
    let text = output_text(&applied);
    assert!(text.contains("skipped (not UTF-8): latin1.txt"), "{text}");
    Ok(())
}

#[test]
fn a_never_read_bom_file_lands_on_the_retry_its_refusal_names() -> TestResult {
    let lab = Lab::with("bom-retry", "demo.sln")?;
    let patch = "PUT 3.=3:\n+# Visual Studio Version 18\n";
    let refused = lab.edit(&format!("[demo.sln]\n{patch}"));
    let text = output_text(&refused);
    assert!(
        refused.is_error && text.contains("then re-issue with this header"),
        "{text}"
    );
    let tag = tag_in(&text).ok_or("no minted tag in the refusal")?;
    let retry = lab.edit(&format!("[demo.sln#{tag}]\n{patch}"));
    let text = output_text(&retry);
    assert!(!retry.is_error && !text.contains("rebased"), "{text}");
    assert_bytes(
        &lab.bytes("demo.sln")?,
        &with_one_line_changed("demo.sln", b"Version 17", b"Version 18")?,
    );
    Ok(())
}

#[test]
fn a_block_put_over_another_hunks_line_is_refused() -> TestResult {
    let lab = Lab::with("block-put", "lib.rs")?;
    let tag = lab.tag_of("lib.rs")?;
    let edit = lab.edit(&format!(
        "[lib.rs#{tag}]\nPUT 1*:\n+pub fn add(left: u64, right: u64) -> u64 {{\n+    left - right\n+}}\nPUT 2.=2:\n+    left * right\n"
    ));
    let text = output_text(&edit);
    assert!(
        edit.is_error && text.contains("already targeted by another hunk"),
        "{text}"
    );
    assert_bytes(&lab.bytes("lib.rs")?, &fixture("lib.rs")?);
    Ok(())
}

#[test]
fn a_block_cut_over_another_hunks_line_is_refused() -> TestResult {
    let lab = Lab::with("block-cut", "lib.rs")?;
    let tag = lab.tag_of("lib.rs")?;
    let edit = lab.edit(&format!(
        "[lib.rs#{tag}]\nCUT 6*\nPUT 11.=11:\n+        let result = add(2, 3);\n"
    ));
    let text = output_text(&edit);
    assert!(
        edit.is_error && text.contains("already targeted by another hunk"),
        "{text}"
    );
    assert_bytes(&lab.bytes("lib.rs")?, &fixture("lib.rs")?);
    Ok(())
}

#[test]
fn a_block_put_over_the_same_range_keeps_the_last_hunk() -> TestResult {
    let lab = Lab::with("block-dup", "lib.rs")?;
    let tag = lab.tag_of("lib.rs")?;
    let edit = lab.edit(&format!(
        "[lib.rs#{tag}]\nPUT 1*:\n+pub fn add(left: u64, right: u64) -> u64 {{\n+    left - right\n+}}\nPUT 1.=3:\n+pub fn add(left: u64, right: u64) -> u64 {{\n+    left * right\n+}}\n"
    ));
    let text = output_text(&edit);
    assert!(
        !edit.is_error && text.contains("kept only the last"),
        "{text}"
    );
    assert_bytes(
        &lab.bytes("lib.rs")?,
        &with_one_line_changed("lib.rs", b"left + right", b"left * right")?,
    );
    Ok(())
}
