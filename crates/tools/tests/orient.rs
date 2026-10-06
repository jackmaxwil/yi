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
/// every layer but ripwire's has something to report.
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
/// this, so the assertion holds whether or not the machine has ripwire.
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

/// Reruns `name` with PATH holding only a fake `ripwire` that answers each verb with the real
/// ripwire 0.6.5 answer recorded under tests/fixtures/ripwire; `slow` makes each answer take 1 s.
#[cfg(unix)]
fn with_fake_ripwire(name: &str, slow: bool, answers: &[(&str, &str)]) -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    let dir = Scratch::new(&format!("yi-orient-{name}"))?;
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ripwire");
    let mut cases = String::new();
    for (verb, fixture) in answers {
        let path = fixtures.join(fixture);
        let (stream, code) = match fixture.ends_with(".txt") {
            true => (">&2", 1),
            false => ("", 0),
        };
        cases.push_str(&format!(
            "  {verb}*) /bin/cat '{}' {stream}; exit {code};;\n",
            path.display()
        ));
    }
    let pause = if slow { "/bin/sleep 1\n" } else { "" };
    let ripwire = dir.join("ripwire");
    fs::write(
        &ripwire,
        format!(
            "#!/bin/sh\n[ -n \"$YI_WARM\" ] && exit 0\necho \"$2\" >> \"$(/usr/bin/dirname \"$0\")/args\"\n\
             {pause}case \"$2\" in\n{cases}esac\nexit 2\n"
        ),
    )?;
    fs::set_permissions(&ripwire, fs::Permissions::from_mode(0o755))?;
    // Incident: a fresh script's first exec stalled past a second under load; run it once here.
    yi_tools::command(&ripwire).env("YI_WARM", "1").output()?;
    let rerun = yi_tools::command(std::env::current_exe()?)
        .args(["--exact", name])
        .env("PATH", &*dir)
        .env("YI_FAKE_RIPWIRE", &*dir)
        .output()?;
    let stdout = String::from_utf8_lossy(&rerun.stdout);
    assert!(
        rerun.status.success() && stdout.contains("1 passed"),
        "{stdout}"
    );
    Ok(())
}

fn ask(dir: &Path, key: &str, value: &str) -> String {
    let mut input = Map::new();
    input.insert(key.to_owned(), json!(value));
    run(dir, input)
}

/// The task map and the neighborhood's callers and callees wait on ripwire, so the packet waits
/// for one answer, not three in a row: each fake answer takes 1 s, three in a row 3 s.
#[cfg(unix)]
#[test]
fn the_ripwire_layers_are_asked_at_once() -> TestResult {
    const NAME: &str = "the_ripwire_layers_are_asked_at_once";
    let Some(dir) = std::env::var_os("YI_FAKE_RIPWIRE") else {
        let answers = [
            ("--for=", "for.json"),
            ("--callers=", "callers.json"),
            ("--callees=", "callees.json"),
        ];
        return with_fake_ripwire(NAME, true, &answers);
    };
    let mut input = Map::new();
    input.insert("task".to_owned(), json!("change what alpha returns"));
    input.insert("symbol".to_owned(), json!("alpha"));
    let started = std::time::Instant::now();
    let packet = run(Path::new(&dir), input);
    let took = started.elapsed();
    assert!(
        packet.contains("src/lib.rs:5  pub fn caller() -> u32"),
        "{packet}"
    );
    assert!(
        packet.contains("callers of alpha: 2, matched by name"),
        "{packet}"
    );
    assert!(packet.contains("  other src/beta.rs:2"), "{packet}");
    assert!(packet.contains("callees of alpha: 1"), "{packet}");
    assert!(took < std::time::Duration::from_millis(2500), "{took:?}");
    Ok(())
}

/// Ripwire's own cuts reach the model at the cut: its byte budget names the call that widens it,
/// and its relevance floor says how many symbols scored, down to none.
#[cfg(unix)]
#[test]
fn the_task_map_names_ripwires_own_cuts() -> TestResult {
    const NAME: &str = "the_task_map_names_ripwires_own_cuts";
    let Some(dir) = std::env::var_os("YI_FAKE_RIPWIRE") else {
        let answers = [
            ("--for=capped", "for-capped.json"),
            ("--for=nothing", "for-nothing-scored.json"),
            ("--for=", "for.json"),
        ];
        return with_fake_ripwire(NAME, false, &answers);
    };
    let dir = Path::new(&dir);
    let capped = ask(dir, "task", "capped streaming bold split");
    assert!(
        capped.contains("[task map: 30 of 40 signatures, cut at ripwire's byte budget — bash: ripwire . --for='capped streaming bold split' --json --token-budget=7000]"),
        "{capped}"
    );
    assert!(
        capped.contains("crates/tui/src/app/stream.rs:466  pub(super) fn commit_prose"),
        "{capped}"
    );
    // The suggested command is pasted into a shell, so a task with a quote must survive sh.
    let quoted = ask(dir, "task", "capped: a session's lane");
    let pasted = quoted
        .split("bash: ripwire . --for=")
        .nth(1)
        .and_then(|rest| rest.split(" --json").next())
        .ok_or(quoted.clone())?;
    let echoed = yi_tools::command("/bin/sh")
        .args(["-c", &format!("printf %s {pasted}")])
        .output()?;
    assert_eq!(
        String::from_utf8_lossy(&echoed.stdout),
        "capped: a session's lane"
    );
    let floor = ask(dir, "task", "change what alpha returns");
    assert!(
        floor.contains(
            "[task map — relevance floor: kept 3 of 40 - the other 37 scored zero on this query"
        ),
        "{floor}"
    );
    let none = ask(dir, "task", "nothing like this here");
    assert!(
        none.contains("[task map — relevance floor: kept 0 of 40"),
        "{none}"
    );
    let untasked = run(dir, Map::new());
    assert!(
        untasked.contains("absent: no task argument was given"),
        "{untasked}"
    );
    Ok(())
}

/// A symbol ripwire cannot find keeps its did-you-mean, so the next call is the right name.
#[cfg(unix)]
#[test]
fn a_missed_symbol_keeps_ripwires_suggestion() -> TestResult {
    const NAME: &str = "a_missed_symbol_keeps_ripwires_suggestion";
    let Some(dir) = std::env::var_os("YI_FAKE_RIPWIRE") else {
        let answers = [
            ("--callers=", "callers-miss.txt"),
            ("--callees=", "callers-miss.txt"),
        ];
        return with_fake_ripwire(NAME, false, &answers);
    };
    let missed = ask(Path::new(&dir), "symbol", "alpha2");
    assert!(missed.contains("(did you mean 'alpha'?)"), "{missed}");
    assert!(missed.contains("PARTIAL"), "{missed}");
    Ok(())
}

/// Incident: `crate::app::stream::commit_prose` missed though `commit_prose` is found; a path is
/// cut to the name, or to `Type::name` where a type owns it.
#[cfg(unix)]
#[test]
fn a_path_qualified_symbol_asks_ripwire_by_its_last_names() -> TestResult {
    const NAME: &str = "a_path_qualified_symbol_asks_ripwire_by_its_last_names";
    let Some(dir) = std::env::var_os("YI_FAKE_RIPWIRE") else {
        let answers = [
            ("--callers=", "callers.json"),
            ("--callees=", "callees.json"),
        ];
        return with_fake_ripwire(NAME, false, &answers);
    };
    let dir = Path::new(&dir);
    ask(dir, "symbol", "crate::app::stream::commit_prose");
    ask(
        dir,
        "symbol",
        "yi_tools::hashline::tool::HashlineEditTool::execute",
    );
    let asked = fs::read_to_string(dir.join("args"))?;
    assert!(asked.contains("--callers=commit_prose\n"), "{asked}");
    assert!(
        asked.contains("--callers=HashlineEditTool::execute\n"),
        "{asked}"
    );
    Ok(())
}

/// Twenty rows a side are shown: `draft` has exactly twenty callers and shows them all;
/// `journal_path` has twenty-one, and the cut names the call that lists every one.
#[cfg(unix)]
#[test]
fn the_neighborhood_names_its_row_cap_at_the_limit_plus_one() -> TestResult {
    const NAME: &str = "the_neighborhood_names_its_row_cap_at_the_limit_plus_one";
    let Some(dir) = std::env::var_os("YI_FAKE_RIPWIRE") else {
        let answers = [
            ("--callers=draft", "callers-at-cap.json"),
            ("--callees=draft", "callees-at-cap.json"),
            ("--callers=journal_path", "callers-past-cap.json"),
            ("--callees=journal_path", "callees-past-cap.json"),
        ];
        return with_fake_ripwire(NAME, false, &answers);
    };
    let dir = Path::new(&dir);
    let at = ask(dir, "symbol", "draft");
    assert!(!at.contains("rows, cap 20"), "{at}");
    assert!(at.contains("callees of draft: 18"), "{at}");
    let past = ask(dir, "symbol", "journal_path");
    assert!(
        past.contains("[callers: 20 of 21 rows, cap 20 — bash: ripwire . --callers='journal_path' --json --limit=21]"),
        "{past}"
    );
    assert!(past.contains("callees of journal_path: 2"), "{past}");
    let section = past
        .split("callers of journal_path")
        .nth(1)
        .unwrap_or_default();
    let shown = section
        .lines()
        .take_while(|line| !line.starts_with('['))
        .skip(1);
    assert_eq!(
        shown.filter(|line| line.starts_with("  ")).count(),
        20,
        "{past}"
    );
    Ok(())
}

/// Ripwire's real answer for a type is no call either way, which says nothing about its uses.
#[cfg(unix)]
#[test]
fn a_type_with_no_calls_points_at_grep() -> TestResult {
    const NAME: &str = "a_type_with_no_calls_points_at_grep";
    let Some(dir) = std::env::var_os("YI_FAKE_RIPWIRE") else {
        let answers = [
            ("--callers=", "callers-type.json"),
            ("--callees=", "callees-type.json"),
        ];
        return with_fake_ripwire(NAME, false, &answers);
    };
    let near = ask(Path::new(&dir), "symbol", "HashlineEditTool");
    assert!(
        near.contains("[HashlineEditTool: no call found either way; ripwire matches calls by name and does not track uses of a type — grep -rn 'HashlineEditTool' for those]"),
        "{near}"
    );
    Ok(())
}

/// An exit-0 answer the seam cannot read is named absent per layer, never zero rows: shape drift
/// is not proof that no call exists.
#[cfg(unix)]
#[test]
fn an_unreadable_neighborhood_answer_is_absent_not_clean() -> TestResult {
    const NAME: &str = "an_unreadable_neighborhood_answer_is_absent_not_clean";
    let Some(dir) = std::env::var_os("YI_FAKE_RIPWIRE") else {
        let answers = [
            ("--callers=", "callers-shape.json"),
            ("--callees=", "callers-shape.json"),
        ];
        return with_fake_ripwire(NAME, false, &answers);
    };
    let near = ask(Path::new(&dir), "symbol", "alpha");
    assert!(
        near.contains("absent: the answer named no callers rows"),
        "{near}"
    );
    assert!(!near.contains("no call found either way"), "{near}");
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
