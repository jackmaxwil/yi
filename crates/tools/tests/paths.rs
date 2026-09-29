//! A glob's literal head resolves like any path argument, walks honour the `.gitignore` files
//! above their root, and `find=` references follow definitions only. The tree is what
//! `cargo new --lib demo` writes: `fixtures/endings/lib.rs` is its `src/lib.rs`, byte for byte.
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use yi_tools::hashline::tool::{HashlineReadTool, shared_hashline_state};
use yi_tools::{GrepTool, Tool, ToolContext, ToolOutput};
use yi_types::message::Content;

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

fn text(output: &ToolOutput) -> String {
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

fn args(pairs: &[(&str, &str)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), json!(value)))
        .collect()
}

/// `demo/` as cargo leaves it: a repository with `src/lib.rs` and a `.gitignore`.
fn demo(scratch: &Scratch) -> Result<PathBuf, Box<dyn Error>> {
    let demo = scratch.join("demo");
    fs::create_dir_all(demo.join(".git"))?;
    fs::create_dir_all(demo.join("src"))?;
    let lib = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/endings/lib.rs");
    fs::copy(lib, demo.join("src/lib.rs"))?;
    fs::write(demo.join(".gitignore"), "/target\n")?;
    Ok(demo)
}

fn read(cwd: &Path, pairs: &[(&str, &str)]) -> String {
    let tool = HashlineReadTool::new(shared_hashline_state());
    text(&tool.execute(args(pairs), &ToolContext::new(cwd.to_path_buf())))
}

fn grep(context: &ToolContext, pairs: &[(&str, &str)]) -> String {
    text(&GrepTool { hashline: None }.execute(args(pairs), context))
}

#[test]
fn a_parent_or_absolute_glob_reads_the_files_it_names() -> TestResult {
    let scratch = Scratch::new("yi-paths-glob")?;
    let src = demo(&scratch)?.join("src");
    let elsewhere = scratch.join("elsewhere");
    fs::create_dir_all(&elsewhere)?;
    let absolute = format!("{}/*.rs", src.display());
    let cases = [
        (&src, "../src/*.rs"),
        (&src, absolute.as_str()),
        (&elsewhere, absolute.as_str()),
    ];
    for (cwd, pattern) in cases {
        let listed = read(cwd, &[("path", pattern)]);
        assert!(
            listed.contains("1 files match") && listed.contains("pub fn add"),
            "{pattern} from {} names src/lib.rs: {listed}",
            cwd.display()
        );
    }
    Ok(())
}

#[test]
fn a_grep_include_resolves_like_a_path() -> TestResult {
    let scratch = Scratch::new("yi-paths-include")?;
    let demo = demo(&scratch)?;
    let src = demo.join("src");
    let absolute = format!("{}/*.rs", src.display());
    let pattern = ("pattern", r"left \+ right");
    let cases: [(&Path, Vec<(&str, &str)>); 3] = [
        (&src, vec![pattern, ("include", absolute.as_str())]),
        (&src, vec![pattern, ("include", "../src/*.rs")]),
        (&demo, vec![pattern, ("path", "src"), ("include", "*.rs")]),
    ];
    for (cwd, call) in cases {
        let found = grep(&ToolContext::new(cwd.to_path_buf()), &call);
        assert!(
            found.contains("left + right"),
            "{call:?} matches lib.rs: {found}"
        );
    }
    let outside = grep(
        &ToolContext::new(demo.clone()),
        &[pattern, ("path", "src"), ("include", "src/*.rs")],
    );
    assert!(
        outside.contains("No matches"),
        "a relative include is relative to path, not the cwd: {outside}"
    );
    Ok(())
}

/// Guard: the walk starts at `src/`, below the `.gitignore` that hides `gen/`.
#[test]
fn a_walk_below_the_repository_root_honours_the_ignore_files_above_it() -> TestResult {
    let scratch = Scratch::new("yi-paths-ignore")?;
    let demo = demo(&scratch)?;
    fs::write(demo.join(".gitignore"), "/target\ngen/\n")?;
    fs::create_dir_all(demo.join("src/gen"))?;
    fs::write(demo.join("src/gen/x.rs"), "pub fn generated_marker() {}\n")?;
    let listed = read(&demo, &[("path", "src/**/*.rs")]);
    assert!(
        listed.contains("lib.rs") && !listed.contains("generated_marker"),
        "src/gen is ignored from demo/.gitignore: {listed}"
    );
    let found = grep(
        &ToolContext::new(demo.clone()),
        &[("pattern", "generated_marker"), ("path", "src")],
    );
    assert!(
        found.contains("No matches"),
        "grep path=src walks into gen/: {found}"
    );
    Ok(())
}

/// A `.git` above the repository must not bind it: a dotfiles repository at `~` ignoring `*`
/// blanked every walk. Between the root and its repository top, the inner file wins.
#[test]
fn only_the_nearest_repository_binds_a_walk_and_its_inner_rules_win() -> TestResult {
    let scratch = Scratch::new("yi-paths-top")?;
    fs::create_dir_all(scratch.join(".git"))?;
    fs::write(scratch.join(".gitignore"), "*\n!.gitignore\n")?;
    let demo = demo(&scratch)?;
    let listed = read(&demo, &[("path", "**/*.rs")]);
    assert!(
        listed.contains("pub fn add"),
        "the outer `*` hid demo: {listed}"
    );
    let found = grep(
        &ToolContext::new(demo.clone()),
        &[("pattern", r"left \+ right")],
    );
    assert!(
        found.contains("left + right"),
        "the outer `*` hid demo: {found}"
    );
    fs::write(demo.join(".gitignore"), "*.gen.rs\n")?;
    fs::write(demo.join("src/.gitignore"), "!keep.gen.rs\n")?;
    fs::create_dir_all(demo.join("src/sub"))?;
    fs::write(
        demo.join("src/sub/keep.gen.rs"),
        "pub fn kept_marker() {}\n",
    )?;
    fs::write(
        demo.join("src/sub/drop.gen.rs"),
        "pub fn dropped_marker() {}\n",
    )?;
    let listed = read(&demo, &[("path", "src/sub/*.rs")]);
    assert!(
        listed.contains("kept_marker") && !listed.contains("dropped_marker"),
        "src/.gitignore re-includes keep.gen.rs over demo/.gitignore: {listed}"
    );
    Ok(())
}

/// A glob from outside the home walks into it; its key store stays out of every walk, however
/// the walk's root is spelled: another letter case, a link, `/private`, or a firmlink.
#[cfg(unix)]
#[test]
fn a_glob_never_walks_into_a_key_store() -> TestResult {
    let scratch = Scratch::new("yi-paths-keys")?;
    let home = scratch.join("home");
    fs::create_dir_all(home.join(".ssh"))?;
    fs::write(home.join(".ssh/id_rsa"), "FAKE PRIVATE KEY MARKER\n")?;
    let elsewhere = scratch.join("elsewhere");
    fs::create_dir_all(&elsewhere)?;
    std::os::unix::fs::symlink(home.join(".ssh"), elsewhere.join("link"))?;
    std::os::unix::fs::symlink(&home, elsewhere.join("h"))?;
    // SAFETY: nextest runs each test in its own process; no other test reads HOME.
    unsafe { std::env::set_var("HOME", &home) };
    let name = scratch
        .file_name()
        .map(|name| name.to_string_lossy().to_uppercase());
    let upper = scratch.with_file_name(name.unwrap_or_default());
    let (home, upper) = (home.display().to_string(), upper.display());
    let mut patterns = vec![
        format!("{home}/.ss?/*"),
        format!("{home}/*/id_*"),
        format!("{home}/**"),
        "~/.ss?/*".to_owned(),
        "~/*/id_*".to_owned(),
        "link/*".to_owned(),
        "link/**".to_owned(),
        "h/.ss?/*".to_owned(),
        "h/**/id_*".to_owned(),
    ];
    if Path::new(&format!("{home}/.SSH")).exists() {
        patterns.extend([
            format!("{home}/.SSH/*"),
            "~/.SSH/*".to_owned(),
            format!("{upper}/*/.ssh/*"),
            format!("{upper}/HOME/.ss?/*"),
        ]);
    }
    for alias in ["/private", "/System/Volumes/Data/private"] {
        if Path::new(&format!("{alias}{home}")).exists() {
            patterns.push(format!("{alias}{home}/.ss?/*"));
        }
    }
    for pattern in &patterns {
        let listed = read(&elsewhere, &[("path", pattern)]);
        assert!(
            !listed.contains("KEY MARKER"),
            "read {pattern} leaks: {listed}"
        );
    }
    for path in ["../home", "link", "h"] {
        let context = ToolContext::new(elsewhere.clone());
        let found = grep(&context, &[("pattern", "KEY MARKER"), ("path", path)]);
        assert!(!found.contains("id_rsa"), "grep path={path} leaks: {found}");
    }
    Ok(())
}

#[test]
fn find_lists_references_for_a_definition_in_files_of_its_type() -> TestResult {
    let scratch = Scratch::new("yi-paths-refs")?;
    let demo = demo(&scratch)?;
    fs::write(demo.join("README.md"), "Call add(2, 2) to add numbers.\n")?;
    let local = read(&demo, &[("path", "src/lib.rs"), ("find", "let result")]);
    assert!(
        !local.contains("[refs:"),
        "a local binding lists no refs: {local}"
    );
    let defined = read(&demo, &[("path", "src/lib.rs"), ("find", "pub fn add")]);
    assert!(
        defined.contains("[refs: 1 of 1 for add]") && !defined.contains("README.md"),
        "only the .rs call site is a reference: {defined}"
    );
    fs::write(
        demo.join("src/lib.rs"),
        "// pub fn add is below\npub fn add() {}\n",
    )?;
    let comment = read(&demo, &[("path", "src/lib.rs"), ("find", "// pub fn add")]);
    assert!(
        !comment.contains("[refs:"),
        "a comment defines nothing: {comment}"
    );
    let calls = [
        (
            "run.py",
            "result = compute(1)\ncompute(2)\n",
            "result = compute",
        ),
        (
            "check.py",
            "assert compute(1)\ncompute(2)\n",
            "assert compute",
        ),
        (
            "app.ts",
            "export default connect(App)\nconnect(1)\n",
            "export default",
        ),
        (
            "m.cc",
            "int main() {\n    int y(3);\n    y;\n}\n",
            "int y(3)",
        ),
    ];
    for (file, text, needle) in calls {
        fs::write(demo.join(file), text)?;
        let call = read(&demo, &[("path", file), ("find", needle)]);
        assert!(!call.contains("[refs:"), "{needle} defines nothing: {call}");
    }
    let cases = [
        (
            "a.rs",
            "pub(crate) fn helper_x() {}",
            "helper_x",
            "b.rs",
            "helper_x();",
        ),
        (
            "a.rs",
            "pub(super) fn helper_y() {}",
            "helper_y",
            "b.rs",
            "helper_y();",
        ),
        (
            "a.ts",
            "export function greet() {}",
            "greet",
            "b.tsx",
            "greet();",
        ),
        (
            "a.ts",
            "export const shout = 1;",
            "shout",
            "b.tsx",
            "shout;",
        ),
        (
            "a.c",
            "static int add2(int a) {}",
            "add2",
            "b.c",
            "add2(1);",
        ),
        (
            "a.go",
            "func (s *Server) Handle(w int) {}",
            "Handle",
            "b.go",
            "s.Handle(1)",
        ),
        (
            "a.h",
            "struct point { int x; };",
            "point",
            "b.c",
            "struct point p;",
        ),
    ];
    for (file, line, name, user, call) in cases {
        let dir = scratch.join("langs").join(name);
        fs::create_dir_all(&dir)?;
        fs::write(dir.join(file), format!("{line}\n"))?;
        fs::write(dir.join(user), format!("{call}\n"))?;
        let found = read(&dir, &[("path", file), ("find", line)]);
        assert!(
            found.contains(&format!("[refs: 1 of 1 for {name}]")) && found.contains(user),
            "{line} lists its use in {user}: {found}"
        );
    }
    Ok(())
}

/// A symlink or another letter case reaches the walled tree by a name the deny list does not
/// spell; every walk must refuse it all the same.
#[cfg(unix)]
#[test]
fn a_walk_never_enters_a_walled_tree_by_another_name() -> TestResult {
    let scratch = Scratch::new("yi-paths-wall")?;
    let demo = demo(&scratch)?;
    fs::create_dir_all(demo.join("Secret"))?;
    fs::write(demo.join("Secret/k.rs"), "pub fn hidden_marker() {}\n")?;
    std::os::unix::fs::symlink(demo.join("Secret"), demo.join("alias"))?;
    let mut context = ToolContext::new(demo.clone());
    context.deny_read = vec![demo.join("Secret")];
    let mut roots = vec!["alias", "alias/.", "src/../alias"];
    if demo.join("SECRET").exists() {
        roots.push("SECRET");
    }
    for root in roots {
        let found = grep(&context, &[("pattern", "hidden_marker"), ("path", root)]);
        assert!(
            !found.contains("hidden_marker("),
            "grep path={root} leaks: {found}"
        );
        let tool = HashlineReadTool::new(shared_hashline_state());
        let glob = format!("{root}/*.rs");
        let listed = text(&tool.execute(args(&[("path", &glob)]), &context));
        assert!(
            !listed.contains("hidden_marker"),
            "read {glob} leaks: {listed}"
        );
    }
    context.deny_read = vec![demo.join("alias")];
    let found = grep(&context, &[("pattern", "hidden_marker")]);
    assert!(
        !found.contains("k.rs"),
        "a deny named by a link walls its target: {found}"
    );
    Ok(())
}
