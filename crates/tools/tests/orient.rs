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
        packet.contains("[skeletons truncated: 40 of 62 files at the 40-file cap;"),
        "a clamped layer must name what it cut: {packet}"
    );
    Ok(())
}

/// The 4 KB layer clamp cut the skeleton layer mid-entry and dropped its own footer, the one
/// row that says how to see the rest.
#[test]
fn a_byte_capped_skeleton_layer_ends_on_whole_rows_with_its_footer() -> TestResult {
    let dir = fixture("byte-capped")?;
    let body: String = (0..12)
        .map(|n| format!("pub fn a_rather_long_generated_function_name_{n:02}() {{}}\n"))
        .collect();
    for index in 0..30 {
        fs::write(dir.join(format!("wide{index:02}.rs")), &body)?;
    }
    let packet = run(&dir, Map::new());
    let layer = skeleton_layer(&packet);
    assert!(!layer.contains("truncated at 4000 bytes"), "{layer}");
    let last = layer.trim_end().lines().last().unwrap_or_default();
    assert!(
        last.starts_with("[skeletons truncated: ")
            && last.contains(" of 32 files at the 4000-byte layer budget;"),
        "{layer}"
    );
    assert!(layer.len() <= 4_000, "{}", layer.len());
    for row in layer
        .lines()
        .filter(|row| row.contains("generated_function_name"))
    {
        assert!(row.ends_with("() {}"), "a row cut mid-entry: {row:?}");
    }
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

fn skeleton_layer(packet: &str) -> &str {
    packet
        .split("\n## ")
        .find_map(|section| section.strip_prefix("file skeletons\n"))
        .unwrap_or_default()
}

/// Forty-five files that sort ahead of the one that matters, so a name-ordered
/// walk spends the whole layer before reaching it.
fn crowded(tag: &str) -> Result<Scratch, Box<dyn Error>> {
    let dir = fixture(tag)?;
    for index in 0..45 {
        fs::write(
            dir.join(format!("gen{index:03}.rs")),
            "pub fn generated() {}\n",
        )?;
    }
    Ok(dir)
}

#[test]
fn skeletons_rank_the_file_defining_the_symbol_first() -> TestResult {
    let dir = crowded("symbol-rank")?;
    let heads: String = (0..12).map(|n| format!("pub fn f{n}() {{}}\n")).collect();
    fs::write(
        dir.join("zz_needle.rs"),
        format!("pub fn needle_target() -> u8 {{\n    7\n}}\n{heads}"),
    )?;
    let mut input = Map::new();
    input.insert("symbol".to_owned(), json!("needle_target"));
    let packet = run(&dir, input);
    let layer = skeleton_layer(&packet);
    assert!(
        layer.contains("zz_needle.rs\n  pub fn needle_target() -> u8"),
        "the only file defining the symbol must keep its skeleton: {layer}"
    );
    assert!(
        layer.contains(
            "  pub fn f10() {}\n  [12 of 13 heads, cap 12 per file — grep def=true for all]"
        ),
        "a file cut at its head cap must say so: {layer}"
    );
    assert!(
        layer.contains(
            "[order: 1 defining `needle_target`, 0 mentioning it, 2 with change heat, then name]"
        ),
        "the layer must say how it ordered the files: {layer}"
    );
    assert!(
        layer.contains("[skeletons truncated: 40 of 48 files at the 40-file cap;"),
        "the cut must name kept/total and the cap: {layer}"
    );
    Ok(())
}

#[test]
fn skeletons_rank_hot_files_ahead_of_the_alphabet() -> TestResult {
    let dir = crowded("heat-rank")?;
    fs::write(dir.join("zz_hot.rs"), "pub fn hot() {}\n")?;
    git(&dir, &["add", "zz_hot.rs"])?;
    git(&dir, &["commit", "-m", "three"])?;
    let packet = run(&dir, Map::new());
    let layer = skeleton_layer(&packet);
    assert!(
        layer.contains("zz_hot.rs\n  pub fn hot()"),
        "a file git changed must outrank one it never touched: {layer}"
    );
    Ok(())
}

/// The two grid layers wait on another process each, so the packet waits for one of them, not
/// both. The rerun owns a PATH of a fake grid that takes 1 s to answer.
#[cfg(unix)]
#[test]
fn the_grid_layers_are_asked_at_once() -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    const NAME: &str = "the_grid_layers_are_asked_at_once";
    let Some(dir) = std::env::var_os("YI_FAKE_GRID") else {
        let dir = Scratch::new("yi-orient-overlap")?;
        fs::create_dir_all(dir.join(".grid"))?;
        let grid = dir.join("grid");
        fs::write(&grid, "#!/bin/sh\n/bin/sleep 1\necho \"$1 answered\"\n")?;
        fs::set_permissions(&grid, fs::Permissions::from_mode(0o755))?;
        let rerun = yi_tools::command(std::env::current_exe()?)
            .args(["--exact", NAME])
            .env("PATH", &*dir)
            .env("YI_FAKE_GRID", &*dir)
            .output()?;
        let stdout = String::from_utf8_lossy(&rerun.stdout);
        assert!(
            rerun.status.success() && stdout.contains("1 passed"),
            "{stdout}"
        );
        return Ok(());
    };
    let mut input = Map::new();
    input.insert("symbol".to_owned(), json!("alpha"));
    let started = std::time::Instant::now();
    let packet = run(Path::new(&dir), input);
    let took = started.elapsed();
    assert!(packet.contains("roots answered"), "{packet}");
    assert!(packet.contains("scope answered"), "{packet}");
    assert!(took < std::time::Duration::from_millis(1800), "{took:?}");
    Ok(())
}

/// Dogfood 2026-09-27: `symbol=HashlineEditTool` said nothing is called that, because grid
/// matches every dot-separated segment, and the miss kept only its first line. The fake answers
/// `**.alpha` and `**.tool.alpha`, and anything else with grid's real miss, closest names and all;
/// it leaves a `ran` marker, which an uncharted tree must never see.
#[cfg(unix)]
#[test]
fn a_bare_symbol_reaches_grid_under_any_path_and_a_miss_keeps_its_names() -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    const NAME: &str = "a_bare_symbol_reaches_grid_under_any_path_and_a_miss_keeps_its_names";
    let Some(dir) = std::env::var_os("YI_FAKE_GRID") else {
        let dir = Scratch::new("yi-orient-bare")?;
        let miss = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/grid/scope-miss.txt");
        let grid = dir.join("grid");
        fs::write(
            &grid,
            format!(
                "#!/bin/sh\n: > ran\ncase \"$1 $2\" in\n\
                 'scope **.alpha'|'scope **.tool.alpha') echo \"scope $2 answered\";;\n\
                 scope*) /bin/cat '{}' >&2; exit 2;;\nesac\n",
                miss.display()
            ),
        )?;
        fs::set_permissions(&grid, fs::Permissions::from_mode(0o755))?;
        let rerun = yi_tools::command(std::env::current_exe()?)
            .args(["--exact", NAME])
            .env("PATH", &*dir)
            .env("YI_FAKE_GRID", &*dir)
            .output()?;
        let stdout = String::from_utf8_lossy(&rerun.stdout);
        assert!(
            rerun.status.success() && stdout.contains("1 passed"),
            "{stdout}"
        );
        return Ok(());
    };
    let ask = |symbol: &str| {
        let mut input = Map::new();
        input.insert("symbol".to_owned(), json!(symbol));
        run(Path::new(&dir), input)
    };
    // grid would survey an uncharted tree and write `.grid/` into it.
    let uncharted = ask("alpha");
    assert!(!Path::new(&dir).join("ran").exists(), "{uncharted}");
    assert!(uncharted.contains("absent: no .grid chart"), "{uncharted}");
    fs::create_dir_all(Path::new(&dir).join(".grid"))?;
    let bare = ask("alpha");
    assert!(bare.contains("scope **.alpha answered"), "{bare}");
    let pathed = ask("tool::alpha");
    assert!(pathed.contains("scope **.tool.alpha answered"), "{pathed}");
    let missed = ask("scale");
    assert!(missed.contains("  drift.user.total"), "{missed}");
    Ok(())
}

/// `justfile` and `Justfile` are one file on APFS and two on Linux; either way one gate.
#[test]
fn a_gate_command_is_listed_once() -> TestResult {
    let dir = fixture("one-gate")?;
    fs::write(dir.join("justfile"), "check:\n")?;
    fs::write(dir.join("Justfile"), "check:\n")?;
    let packet = run(&dir, Map::new());
    assert_eq!(packet.matches("just check  (").count(), 1, "{packet}");
    Ok(())
}
