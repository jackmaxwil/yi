use std::error::Error;
use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::{Map, Value, json};
use yi_tools::{GetContextTool, Tool, ToolContext};
use yi_types::message::Content;

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

#[expect(
    clippy::disallowed_methods,
    reason = "the fixture repo is built by git itself; run_captured is the tool's path, not the test's"
)]
fn git(root: &Path, args: &[&str]) -> TestResult {
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    assert!(status.success(), "git {args:?} failed");
    Ok(())
}

/// A fixture repo with two commits, a gate manifest, and a mining store, so
/// every non-grid layer has something to report.
fn fixture(tag: &str) -> Result<Scratch, Box<dyn Error>> {
    let dir = Scratch::new(&format!("yi-orient-{tag}"))?;
    let root = &dir;
    fs::write(root.join("Cargo.toml"), "[package]\nname = \"fixture\"\n")?;
    fs::write(
        root.join("alpha.rs"),
        "pub fn alpha() {\n    let x = 1;\n}\n",
    )?;
    git(root, &["init", "--initial-branch=main"])?;
    git(root, &["config", "user.email", "fixture@example.com"])?;
    git(root, &["config", "user.name", "Fixture"])?;
    git(root, &["add", "Cargo.toml", "alpha.rs"])?;
    git(root, &["commit", "-m", "one"])?;
    fs::write(root.join("beta.rs"), "pub struct Beta;\n")?;
    fs::write(
        root.join("alpha.rs"),
        "pub fn alpha() {\n    let x = 2;\n}\n",
    )?;
    git(root, &["add", "beta.rs", "alpha.rs"])?;
    // Ten paths tied at one touch each: without a deterministic tie-break the
    // heat layer reorders between runs.
    for index in 0..10 {
        let name = format!("tie{index:02}.txt");
        fs::write(root.join(&name), "tie\n")?;
        git(root, &["add", &name])?;
    }
    git(root, &["commit", "-m", "two"])?;
    fs::create_dir_all(root.join(".yi/mining"))?;
    fs::write(
        root.join(".yi/mining/issues.jsonl"),
        "{\"fingerprint\":\"fp-1\",\"title\":\"stale baseline\"}\n",
    )?;
    Ok(dir)
}

fn run(root: &Path, input: Map<String, Value>) -> String {
    let output = GetContextTool.execute(input, &ToolContext::new(root.to_path_buf()));
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

#[test]
fn packet_is_byte_stable_across_two_runs_on_one_tree() -> TestResult {
    let dir = fixture("stable")?;
    let first = run(&dir, Map::new());
    let second = run(&dir, Map::new());
    assert_eq!(
        first, second,
        "orientation packet must not drift between runs on a fixed tree"
    );
    assert!(
        first.contains("   2  alpha.rs"),
        "change heat must count the twice-touched file: {first}"
    );
    assert!(
        first.contains("cargo test  (Cargo.toml)"),
        "gate layer must name the detected manifest: {first}"
    );
    assert!(
        first.contains("fp-1  stale baseline"),
        "mining layer must carry the planted fingerprint: {first}"
    );
    Ok(())
}

/// Sections as (name, first body line). The header count is checked against
/// this, so the assertion holds whether or not the machine has grid.
fn sections(packet: &str) -> Vec<(String, String)> {
    packet
        .split("\n## ")
        .skip(1)
        .map(|section| {
            let (name, body) = section.split_once('\n').unwrap_or((section, ""));
            (
                name.to_owned(),
                body.lines().next().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

#[test]
fn every_layer_is_named_and_the_header_count_matches() -> TestResult {
    let dir = fixture("honest-header")?;
    let packet = run(&dir, Map::new());
    let sections = sections(&packet);
    assert_eq!(sections.len(), 6, "six named layers: {packet}");
    for (name, first) in &sections {
        assert!(
            !first.is_empty(),
            "layer {name} must say something, absence included: {packet}"
        );
    }
    let present = sections
        .iter()
        .filter(|(_, first)| !first.starts_with("absent: "))
        .count();
    let header = packet.lines().nth(1).unwrap_or_default();
    let expected = if present == 6 {
        "COMPLETE".to_owned()
    } else {
        format!("PARTIAL - {present} of 6 layers")
    };
    assert_eq!(header, expected, "header must count layers honestly");
    assert!(
        packet.contains("## symbol neighborhood\nabsent: no symbol argument was given"),
        "a layer with no argument names the reason: {packet}"
    );
    Ok(())
}

#[test]
fn oversized_skeleton_layer_names_its_truncation() -> TestResult {
    let dir = fixture("clamped")?;
    for index in 0..60 {
        fs::write(
            dir.join(format!("gen{index:03}.rs")),
            "pub fn generated() {}\n",
        )?;
    }
    let packet = run(&dir, Map::new());
    assert!(
        packet.contains("[skeletons truncated: 40 of 62 files]"),
        "a clamped layer must name what it cut: {packet}"
    );
    Ok(())
}

#[test]
fn symbol_argument_reaches_the_neighborhood_layer() -> TestResult {
    let dir = fixture("symbol")?;
    let mut input = Map::new();
    input.insert("symbol".to_owned(), json!("yi_tools.orient"));
    let packet = run(&dir, input);
    assert!(
        !packet.contains("absent: no symbol argument was given"),
        "a named symbol must not report itself missing: {packet}"
    );
    Ok(())
}
